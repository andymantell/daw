// SPDX-License-Identifier: GPL-3.0-or-later
//! The update notice (roadmap 1.8; proposal in
//! docs/research/PLATFORM_DISTRIBUTION.md). Off until the user turns it
//! on: the app asks first, because checking lets the site hosting the
//! releases see the computer's internet address
//! (docs/research/PRIVACY_CHILD_DATA.md). When on, it looks at most once
//! a day for a newer release and shows a note with a link to its page.
//! It never downloads or installs anything.
//!
//! Each release carries `latest.txt`, a short list of its version and
//! files, and `latest.txt.minisig`, a signature over it made with a key
//! only the release workflow holds (`.github/workflows/release.yml`). The
//! app checks the signature against the public key built into it, so a
//! changed release page cannot announce a version that was never built
//! by the workflow. Nothing about the user or the computer is sent: the
//! request names only the app and its version.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use eframe::egui;

use crate::APP_NAME;

/// Where releases are published.
pub(crate) const RELEASES: &str = "https://github.com/andymantell/daw/releases";
/// The release workflow's public key, in the signing tool's format. The
/// file holds no key until the owner makes one (docs/REVIEW_QUEUE.md);
/// until then this build cannot check for updates.
const PUBLIC_KEY: &str = include_str!("../../../packaging/release-key.pub");
/// How often to look, when turned on.
const EVERY: Duration = Duration::from_secs(24 * 60 * 60);
/// The list of files is a few lines; anything bigger is not it.
const MAX_BYTES: u64 = 64 * 1024;

/// The settings, kept in `updates.txt` beside the device settings.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Settings {
    /// The user turned the daily check on.
    pub(crate) daily: bool,
    /// The user has answered the question at start-up.
    pub(crate) asked: bool,
    /// When the last check finished, in seconds since 1970.
    pub(crate) last: u64,
    /// A version whose note the user closed, so it is not shown again.
    pub(crate) dismissed: Option<String>,
}

impl Settings {
    pub(crate) fn from_text(text: &str) -> Settings {
        let mut s = Settings::default();
        for line in text.lines() {
            let (key, value) = line.split_once(' ').unwrap_or((line, ""));
            let value = value.trim();
            match key {
                "daily" => s.daily = value == "yes",
                "asked" => s.asked = value == "yes",
                "last" => s.last = value.parse().unwrap_or(0),
                "dismissed" if !value.is_empty() => s.dismissed = Some(value.into()),
                _ => {}
            }
        }
        s
    }

    pub(crate) fn to_text(&self) -> String {
        let yes = |b| if b { "yes" } else { "no" };
        let mut t = format!(
            "daily {}\nasked {}\nlast {}\n",
            yes(self.daily),
            yes(self.asked),
            self.last
        );
        if let Some(v) = &self.dismissed {
            t.push_str(&format!("dismissed {v}\n"));
        }
        t
    }
}

/// A version number: `1.2.3`, perhaps with `-beta.1` after it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Version {
    numbers: [u64; 3],
    pre: Option<String>,
}

impl Version {
    pub(crate) fn parse(text: &str) -> Option<Version> {
        let text = text.trim().trim_start_matches('v');
        let (core, pre) = match text.split_once('-') {
            Some((c, p)) => (c, Some(p.to_string())),
            None => (text, None),
        };
        let mut numbers = [0; 3];
        let mut parts = core.split('.');
        for n in &mut numbers {
            *n = parts.next()?.parse().ok()?;
        }
        if parts.next().is_some() {
            return None;
        }
        Some(Version { numbers, pre })
    }

    pub(crate) fn current() -> Version {
        Version::parse(env!("CARGO_PKG_VERSION")).expect("the crate's version")
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    /// A test release (`1.0.0-beta`) comes before the release itself.
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.numbers
            .cmp(&other.numbers)
            .then_with(|| match (&self.pre, &other.pre) {
                (None, None) => std::cmp::Ordering::Equal,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (Some(_), None) => std::cmp::Ordering::Less,
                (Some(a), Some(b)) => a.cmp(b),
            })
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let [a, b, c] = self.numbers;
        write!(f, "{a}.{b}.{c}")?;
        if let Some(p) = &self.pre {
            write!(f, "-{p}")?;
        }
        Ok(())
    }
}

/// The key built into this build, if it has one.
fn public_key(text: &str) -> Option<minisign_verify::PublicKey> {
    minisign_verify::PublicKey::decode(text.trim()).ok()
}

/// Checks the list of files against its signature and reads the
/// version from it.
fn verify(
    key: &minisign_verify::PublicKey,
    list: &[u8],
    signature: &str,
) -> Result<Version, String> {
    let sig = minisign_verify::Signature::decode(signature)
        .map_err(|_| "the release's signature could not be read".to_string())?;
    key.verify(list, &sig, false).map_err(|_| {
        "the release's signature does not match: it was not made by the release workflow"
            .to_string()
    })?;
    let list = std::str::from_utf8(list).map_err(|_| "the release's file list is not text")?;
    list.lines()
        .find_map(|l| l.strip_prefix("version "))
        .and_then(Version::parse)
        .ok_or_else(|| "the release's file list has no version".into())
}

/// What a check found.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Found {
    /// This is the newest version.
    UpToDate,
    /// A newer version, with its release page.
    Newer(Version, String),
}

/// Asks the release page for the newest release's file list and checks
/// it. Runs on its own thread.
fn check(key: &minisign_verify::PublicKey) -> Result<Found, String> {
    let tls = ureq::tls::TlsConfig::builder()
        .root_certs(ureq::tls::RootCerts::PlatformVerifier)
        .unversioned_rustls_crypto_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .build();
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .tls_config(tls)
        .timeout_global(Some(Duration::from_secs(20)))
        // Only the app and its version: nothing about the user.
        .user_agent(format!("{APP_NAME}/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .into();
    let get = |name: &str| -> Result<Vec<u8>, String> {
        let url = format!("{RELEASES}/latest/download/{name}");
        match agent.get(&url).call() {
            Ok(mut r) => r
                .body_mut()
                .with_config()
                .limit(MAX_BYTES)
                .read_to_vec()
                .map_err(|e| format!("could not read the release page: {e}")),
            Err(ureq::Error::StatusCode(404)) => {
                Err("no release with a signed file list has been published yet".into())
            }
            Err(e) => Err(format!("could not reach the release page: {e}")),
        }
    };
    let list = get("latest.txt")?;
    let sig = get("latest.txt.minisig")?;
    let sig = String::from_utf8(sig).map_err(|_| "the release's signature is not text")?;
    let newest = verify(key, &list, &sig)?;
    Ok(if newest > Version::current() {
        let page = format!("{RELEASES}/tag/v{newest}");
        Found::Newer(newest, page)
    } else {
        Found::UpToDate
    })
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The update notice, its question at start-up and the Help → Check for
/// updates window.
pub(crate) struct Updates {
    path: PathBuf,
    pub(crate) settings: Settings,
    key: Option<minisign_verify::PublicKey>,
    running: Option<mpsc::Receiver<Result<Found, String>>>,
    /// The last check's result this run.
    pub(crate) result: Option<Result<Found, String>>,
    pub(crate) open: bool,
    /// When the daily check was last considered.
    looked: Option<Instant>,
}

impl Updates {
    /// `path` is the settings file; tests pass `None` to keep none.
    pub(crate) fn new(path: Option<PathBuf>) -> Updates {
        let settings = path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|t| Settings::from_text(&t))
            .unwrap_or_default();
        Updates {
            path: path.unwrap_or_default(),
            settings,
            key: public_key(PUBLIC_KEY),
            running: None,
            result: None,
            open: false,
            looked: None,
        }
    }

    fn save(&self) {
        if self.path.as_os_str().is_empty() {
            return;
        }
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = self.path.with_extension("tmp");
        if std::fs::write(&tmp, self.settings.to_text()).is_ok() {
            let _ = std::fs::rename(tmp, &self.path);
        }
    }

    pub(crate) fn checking(&self) -> bool {
        self.running.is_some()
    }

    /// Starts a check now. Only ever called because the user turned
    /// checking on or pressed "Check now".
    pub(crate) fn start(&mut self) {
        let Some(key) = self.key.clone() else {
            return;
        };
        if self.running.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("update check".into())
            .spawn(move || {
                let _ = tx.send(check(&key));
            });
        if spawned.is_ok() {
            self.running = Some(rx);
        }
    }

    /// Picks up a finished check, and starts the daily one when due.
    pub(crate) fn poll(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.running {
            match rx.try_recv() {
                Ok(r) => {
                    self.running = None;
                    self.result = Some(r);
                    self.settings.last = now_secs();
                    self.save();
                }
                Err(mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(Duration::from_millis(250));
                }
                Err(mpsc::TryRecvError::Disconnected) => self.running = None,
            }
        }
        // Considered once a minute; the app may stay open for days.
        if self
            .looked
            .is_some_and(|t| t.elapsed() < Duration::from_secs(60))
        {
            return;
        }
        self.looked = Some(Instant::now());
        let due = now_secs().saturating_sub(self.settings.last) >= EVERY.as_secs();
        if self.settings.daily && due {
            self.start();
        }
    }

    /// The newer version to tell the user about, unless they closed its
    /// note.
    pub(crate) fn newer(&self) -> Option<(&Version, &str)> {
        match &self.result {
            Some(Ok(Found::Newer(v, page)))
                if self.settings.dismissed.as_deref() != Some(&v.to_string()) =>
            {
                Some((v, page))
            }
            _ => None,
        }
    }

    /// The question at start-up: shown once, and only in builds that can
    /// check.
    pub(crate) fn should_ask(&self) -> bool {
        self.key.is_some() && !self.settings.asked
    }

    pub(crate) fn answer(&mut self, daily: bool) {
        self.settings.asked = true;
        self.set_daily(daily);
    }

    fn set_daily(&mut self, daily: bool) {
        self.settings.daily = daily;
        self.save();
        if daily {
            self.looked = None;
        }
    }

    /// The note about a newer version, in the bottom bar.
    pub(crate) fn note(&mut self, ui: &mut egui::Ui) {
        let Some((v, page)) = self.newer() else {
            return;
        };
        let (v, page) = (v.to_string(), page.to_string());
        ui.horizontal_wrapped(|ui| {
            ui.label(format!(
                "{APP_NAME} {v} is available (you have {}).",
                Version::current()
            ));
            ui.hyperlink_to("Open the release page", &page);
            if ui.button("Not now").clicked() {
                self.settings.dismissed = Some(v);
                self.save();
            }
        });
    }

    /// The text of the question, also shown in the window.
    const WHY: &str = "Once a day, the app can ask GitHub, where new versions are \
        published, whether there is a newer one. GitHub then sees this computer's \
        internet address, as it would for any web page. Nothing else is sent, and \
        nothing is downloaded or installed: you get a note with a link.";

    /// The question at start-up.
    pub(crate) fn ask_window(&mut self, ctx: &egui::Context) {
        if !self.should_ask() {
            return;
        }
        egui::Window::new("Check for new versions?")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.set_max_width(420.0);
                ui.label(Self::WHY);
                ui.label("You can change this later in Help → Check for updates.");
                ui.horizontal(|ui| {
                    if ui.button("Yes, check once a day").clicked() {
                        self.answer(true);
                    }
                    if ui.button("No thanks").clicked() {
                        self.answer(false);
                    }
                });
            });
    }

    /// Help → Check for updates.
    pub(crate) fn window(&mut self, ctx: &egui::Context) {
        if !self.open {
            return;
        }
        let mut open = self.open;
        egui::Window::new("Check for updates")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.set_max_width(440.0);
                ui.label(format!("This is version {}.", Version::current()));
                if self.key.is_none() {
                    ui.label(
                        "This build cannot check for updates: it was not built by the \
                         release workflow, so it has no key to check releases with.",
                    );
                    ui.hyperlink_to("See all releases", RELEASES);
                    return;
                }
                ui.label(Self::WHY);
                let mut daily = self.settings.daily;
                if ui.checkbox(&mut daily, "Check once a day").changed() {
                    self.settings.asked = true;
                    self.set_daily(daily);
                }
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(!self.checking(), egui::Button::new("Check now"))
                        .clicked()
                    {
                        self.start();
                    }
                    if self.checking() {
                        ui.spinner();
                        ui.label("Checking…");
                    }
                });
                match &self.result {
                    Some(Ok(Found::UpToDate)) => {
                        ui.label("You have the newest version.");
                    }
                    Some(Ok(Found::Newer(v, page))) => {
                        ui.label(format!("Version {v} is available."));
                        ui.hyperlink_to("Open the release page", page);
                    }
                    Some(Err(e)) => {
                        ui.label(format!("The check did not work: {e}."));
                    }
                    None => {}
                }
            });
        self.open = open;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A key pair made for these tests only, and a signed list.
    const TEST_KEY: &str = "untrusted comment: minisign public key E24DA3565A9042C6
RWTGQpBaVqNN4uYH/+0ba3zgFy/C7+14Rey5vkVxy+XZJzPnajmAiPdf
";
    const TEST_LIST: &[u8] = b"version 1.2.3\n";
    const TEST_SIG: &str = "untrusted comment: signature from minisign secret key
RUTGQpBaVqNN4o+BMBI6bMzAPEguRMuFGOOY4bcXcApWEQrNYb5xQW1dQnBxzq5ID/KEqLwuBQEiY905nsTzYWy/nqyUjUoeeAU=
trusted comment: DAW 1.2.3
JOuMLeFWm9V3qRy9qpgOLc0r0NSF2pDs+qy2udMrpFRoVNg5joN83zj6K0bqEpfZJtP+Xb4i0LZxF6xA8Z49Dg==
";

    #[test]
    fn a_signed_list_gives_its_version() {
        let key = public_key(TEST_KEY).unwrap();
        assert_eq!(
            verify(&key, TEST_LIST, TEST_SIG),
            Ok(Version::parse("1.2.3").unwrap())
        );
    }

    #[test]
    fn a_changed_list_is_refused() {
        let key = public_key(TEST_KEY).unwrap();
        let e = verify(&key, b"version 9.9.9\n", TEST_SIG).unwrap_err();
        assert!(e.contains("does not match"), "{e}");
        assert!(verify(&key, TEST_LIST, "not a signature").is_err());
    }

    #[test]
    fn a_build_without_a_key_does_not_check_or_ask() {
        assert!(public_key("untrusted comment: no release key yet\n").is_none());
        let mut u = Updates::new(None);
        u.key = None;
        assert!(!u.should_ask());
        u.settings.daily = true;
        u.start();
        assert!(!u.checking());
    }

    #[test]
    fn nothing_is_checked_until_turned_on() {
        let mut u = Updates::new(None);
        u.key = public_key(TEST_KEY);
        assert!(u.should_ask());
        let ctx = egui::Context::default();
        u.poll(&ctx);
        assert!(!u.checking());
        u.answer(false);
        assert!(!u.should_ask());
        u.poll(&ctx);
        assert!(!u.checking());
    }

    #[test]
    fn versions() {
        let v = |s| Version::parse(s).unwrap();
        assert!(v("0.2.0") > v("0.1.9"));
        assert!(v("1.10.0") > v("1.9.0"));
        assert!(v("1.0.0") > v("1.0.0-beta.2"));
        assert!(v("1.0.0-beta.2") > v("1.0.0-beta.1"));
        assert_eq!(v("v0.3.1"), v("0.3.1"));
        assert_eq!(v("1.0.0-rc.1").to_string(), "1.0.0-rc.1");
        assert!(Version::parse("1.2").is_none());
        assert!(Version::parse("1.2.3.4").is_none());
        assert!(Version::parse("one").is_none());
    }

    #[test]
    fn settings_round_trip() {
        let s = Settings {
            daily: true,
            asked: true,
            last: 1_793_221_721,
            dismissed: Some("0.2.0".into()),
        };
        assert_eq!(Settings::from_text(&s.to_text()), s);
        assert_eq!(Settings::from_text(""), Settings::default());
    }

    #[test]
    fn a_closed_note_stays_closed_for_that_version() {
        let mut u = Updates::new(None);
        let v = Version::parse("9.0.0").unwrap();
        u.result = Some(Ok(Found::Newer(v.clone(), "page".into())));
        assert!(u.newer().is_some());
        u.settings.dismissed = Some(v.to_string());
        assert!(u.newer().is_none());
        u.result = Some(Ok(Found::Newer(
            Version::parse("9.0.1").unwrap(),
            "page".into(),
        )));
        assert!(u.newer().is_some());
    }
}
