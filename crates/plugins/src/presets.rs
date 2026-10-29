// SPDX-License-Identifier: GPL-3.0-or-later
//! Plugin presets (roadmap 2.4): the user's own, saved by the app, and
//! the plugin's own, found where plugins and other hosts keep them.
//!
//! Both are `.vstpreset` files, the VST 3 preset file layout, so a
//! preset saved here loads in other hosts and theirs load here. The
//! layout: a header (`VST3`, a version, the plugin's class id as 32 hex
//! digits, and where the chunk list is), the chunks themselves, then a
//! `List` of chunk ids with their offsets and sizes. The app writes the
//! component state (`Comp`) and the controller state (`Cont`); other
//! chunks are kept as they are when a preset is only renamed.
//!
//! The user's own presets live under one folder, one sub-folder per
//! plugin class. The plugin's own are looked for in the standard preset
//! folders, laid out `<Vendor>/<Plugin>/<name>.vstpreset`, and matched
//! by the class id in the file, so folder names do not have to agree.

use std::path::{Path, PathBuf};

use vst3::Steinberg::TUID;

use crate::module::Result;
use crate::plugin::PluginState;

pub const EXT: &str = "vstpreset";

const MAGIC: &[u8; 4] = b"VST3";
const VERSION: u32 = 1;
const LIST: &[u8; 4] = b"List";
const COMPONENT: &[u8; 4] = b"Comp";
const CONTROLLER: &[u8; 4] = b"Cont";
const HEADER: usize = 4 + 4 + 32 + 8;

/// A class id as the 32 hex digits preset files and other hosts use.
/// The VST 3 bindings keep a class id in the platform's GUID byte
/// order, which on Windows differs from the digits as written; this
/// undoes that.
pub fn canonical_hex(cid: &TUID) -> String {
    let b: Vec<u8> = cid.iter().map(|&c| c as u8).collect();
    let ordered: Vec<u8> = if cfg!(windows) {
        let mut o = Vec::with_capacity(16);
        o.extend(b[0..4].iter().rev());
        o.extend([b[5], b[4], b[7], b[6]]);
        o.extend(&b[8..]);
        o
    } else {
        b
    };
    ordered.iter().map(|x| format!("{x:02X}")).collect()
}

/// What a preset file holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PresetFile {
    /// The plugin's class id, as [`canonical_hex`].
    pub class: String,
    pub state: PluginState,
}

impl PresetFile {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&VERSION.to_le_bytes());
        let mut class = self.class.as_bytes().to_vec();
        class.resize(32, b'0');
        out.extend_from_slice(&class);
        let list_offset_at = out.len();
        out.extend_from_slice(&0i64.to_le_bytes());
        let mut entries: Vec<([u8; 4], i64, i64)> = Vec::new();
        let start = out.len() as i64;
        out.extend_from_slice(&self.state.component);
        entries.push((*COMPONENT, start, self.state.component.len() as i64));
        if let Some(c) = &self.state.controller {
            let start = out.len() as i64;
            out.extend_from_slice(c);
            entries.push((*CONTROLLER, start, c.len() as i64));
        }
        let list_offset = out.len() as i64;
        out.extend_from_slice(LIST);
        out.extend_from_slice(&(entries.len() as i32).to_le_bytes());
        for (id, offset, size) in entries {
            out.extend_from_slice(&id);
            out.extend_from_slice(&offset.to_le_bytes());
            out.extend_from_slice(&size.to_le_bytes());
        }
        out[list_offset_at..list_offset_at + 8].copy_from_slice(&list_offset.to_le_bytes());
        out
    }

    /// Reads the header only: the class id, or why this is not a preset.
    pub fn class_of(bytes: &[u8]) -> Result<String> {
        if bytes.len() < HEADER || &bytes[..4] != MAGIC {
            return Err("not a VST 3 preset file".into());
        }
        let class = &bytes[8..40];
        Ok(std::str::from_utf8(class)
            .map_err(|_| "damaged class id")?
            .trim_end_matches('\0')
            .to_ascii_uppercase())
    }

    pub fn decode(bytes: &[u8]) -> Result<PresetFile> {
        let class = PresetFile::class_of(bytes)?;
        let i64_at = |at: usize| -> Result<i64> {
            let b = bytes.get(at..at + 8).ok_or("preset file cut short")?;
            Ok(i64::from_le_bytes(b.try_into().unwrap()))
        };
        let list = i64_at(40)?;
        let list = usize::try_from(list).map_err(|_| "damaged chunk list")?;
        if bytes.get(list..list + 4) != Some(LIST) {
            return Err("damaged chunk list".into());
        }
        let count = bytes
            .get(list + 4..list + 8)
            .map(|b| i32::from_le_bytes(b.try_into().unwrap()))
            .ok_or("preset file cut short")?;
        let count = usize::try_from(count).map_err(|_| "damaged chunk list")?;
        let mut component = None;
        let mut controller = None;
        for i in 0..count {
            let at = list + 8 + i * 20;
            let id = bytes.get(at..at + 4).ok_or("preset file cut short")?;
            let offset = usize::try_from(i64_at(at + 4)?).map_err(|_| "damaged chunk")?;
            let size = usize::try_from(i64_at(at + 12)?).map_err(|_| "damaged chunk")?;
            let data = bytes
                .get(offset..offset.checked_add(size).ok_or("damaged chunk")?)
                .ok_or("preset file cut short")?;
            if id == COMPONENT {
                component = Some(data.to_vec());
            } else if id == CONTROLLER {
                controller = Some(data.to_vec());
            }
        }
        Ok(PresetFile {
            class,
            state: PluginState {
                component: component.ok_or("the preset has no plugin state in it")?,
                controller,
            },
        })
    }
}

/// A preset the user can pick.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Preset {
    pub name: String,
    pub path: PathBuf,
    /// Saved by this app (and so can be deleted here).
    pub own: bool,
}

/// Where the user's own presets for `cid` go, under `root`.
pub fn own_dir(root: &Path, cid: &TUID) -> PathBuf {
    root.join(canonical_hex(cid))
}

/// The standard preset folders for this platform.
pub fn native_dirs() -> Vec<PathBuf> {
    let env = |k: &str| std::env::var_os(k).map(PathBuf::from);
    let mut dirs = Vec::new();
    if cfg!(windows) {
        if let Some(p) = env("USERPROFILE") {
            dirs.push(p.join("Documents").join("VST3 Presets"));
        }
        if let Some(p) = env("PUBLIC") {
            dirs.push(p.join("Documents").join("VST3 Presets"));
        }
    } else if cfg!(target_os = "macos") {
        if let Some(h) = env("HOME") {
            dirs.push(h.join("Library/Audio/Presets"));
        }
        dirs.push("/Library/Audio/Presets".into());
    } else {
        if let Some(h) = env("HOME") {
            dirs.push(h.join(".vst3/presets"));
        }
        dirs.push("/usr/share/vst3/presets".into());
        dirs.push("/usr/local/share/vst3/presets".into());
    }
    dirs
}

fn name_of(path: &Path) -> String {
    path.file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

/// Every `.vstpreset` for `cid` under `dir`, down to `depth` folders.
fn find(dir: &Path, cid: &str, depth: u32, own: bool, out: &mut Vec<Preset>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let path = e.path();
        if path.is_dir() {
            if depth > 0 {
                find(&path, cid, depth - 1, own, out);
            }
            continue;
        }
        if !path
            .extension()
            .is_some_and(|x| x.eq_ignore_ascii_case(EXT))
        {
            continue;
        }
        let Ok(bytes) = read_head(&path) else {
            continue;
        };
        if PresetFile::class_of(&bytes).is_ok_and(|c| c == cid) {
            out.push(Preset {
                name: name_of(&path),
                path,
                own,
            });
        }
    }
}

fn read_head(path: &Path) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut buf = vec![0; HEADER];
    let n = f.read(&mut buf)?;
    buf.truncate(n);
    Ok(buf)
}

/// The user's own presets for `cid` (under `root`), then the plugin's
/// own from the standard folders, each sorted by name.
pub fn list(root: &Path, cid: &TUID) -> Vec<Preset> {
    let hex = canonical_hex(cid);
    let mut own = Vec::new();
    find(&own_dir(root, cid), &hex, 0, true, &mut own);
    own.sort_by_key(|a| a.name.to_lowercase());
    let mut native = Vec::new();
    for d in native_dirs() {
        find(&d, &hex, 3, false, &mut native);
    }
    native.sort_by_key(|a| a.name.to_lowercase());
    own.extend(native);
    own
}

/// A file name from a preset name the user typed.
fn safe_name(name: &str) -> String {
    let s: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_control() || "\\/:*?\"<>|".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    s.trim_matches('.').to_string()
}

/// Saves `state` as the user's preset `name` for `cid`, replacing one
/// of that name.
pub fn save(root: &Path, cid: &TUID, name: &str, state: &PluginState) -> Result<Preset> {
    let name = safe_name(name);
    if name.is_empty() {
        return Err("give the preset a name".into());
    }
    let dir = own_dir(root, cid);
    std::fs::create_dir_all(&dir).map_err(|e| format!("could not make the presets folder: {e}"))?;
    let path = dir.join(&name).with_extension(EXT);
    let file = PresetFile {
        class: canonical_hex(cid),
        state: state.clone(),
    };
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, file.encode())
        .and_then(|()| std::fs::rename(&tmp, &path))
        .map_err(|e| format!("could not save the preset: {e}"))?;
    Ok(Preset {
        name,
        path,
        own: true,
    })
}

/// Reads a preset, checking it is for `cid`.
pub fn load(path: &Path, cid: &TUID) -> Result<PluginState> {
    let bytes = std::fs::read(path).map_err(|e| format!("could not read the preset: {e}"))?;
    let file = PresetFile::decode(&bytes)?;
    if file.class != canonical_hex(cid) {
        return Err("this preset is for a different plugin".into());
    }
    Ok(file.state)
}

pub fn delete(preset: &Preset) -> Result<()> {
    if !preset.own {
        return Err("only presets saved here can be deleted here".into());
    }
    std::fs::remove_file(&preset.path).map_err(|e| format!("could not delete the preset: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("daw-presets-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    const CID: TUID = vst3::uid(0x0102_0304, 0x0506_0708, 0x090A_0B0C, 0x0D0E_0F10);

    #[test]
    fn the_class_id_reads_the_same_on_every_platform() {
        assert_eq!(canonical_hex(&CID), "0102030405060708090A0B0C0D0E0F10");
    }

    #[test]
    fn a_preset_file_comes_back_as_written() {
        let file = PresetFile {
            class: canonical_hex(&CID),
            state: PluginState {
                component: vec![1, 2, 3, 4],
                controller: Some(vec![5]),
            },
        };
        let bytes = file.encode();
        assert_eq!(&bytes[..4], b"VST3");
        assert_eq!(PresetFile::decode(&bytes).unwrap(), file);
        let no_controller = PresetFile {
            state: PluginState {
                component: vec![9],
                controller: None,
            },
            ..file
        };
        assert_eq!(
            PresetFile::decode(&no_controller.encode()).unwrap(),
            no_controller
        );
    }

    #[test]
    fn damaged_preset_files_are_errors() {
        assert!(PresetFile::decode(b"nope").is_err());
        let mut bytes = PresetFile {
            class: canonical_hex(&CID),
            state: PluginState {
                component: vec![1, 2, 3],
                controller: None,
            },
        }
        .encode();
        bytes.truncate(60);
        assert!(PresetFile::decode(&bytes).is_err());
        let mut bad_list = bytes.clone();
        bad_list[40..48].copy_from_slice(&i64::MAX.to_le_bytes());
        assert!(PresetFile::decode(&bad_list).is_err());
    }

    #[test]
    fn own_presets_are_saved_listed_loaded_and_deleted() {
        let root = scratch("own");
        let state = PluginState {
            component: vec![7, 7],
            controller: None,
        };
        assert!(save(&root, &CID, "  ", &state).is_err());
        let p = save(&root, &CID, "Clean: bright?", &state).unwrap();
        assert_eq!(p.name, "Clean_ bright_");
        assert!(p.own);
        save(&root, &CID, "Amp", &state).unwrap();
        let listed = list(&root, &CID);
        let names: Vec<&str> = listed.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["Amp", "Clean_ bright_"]);
        assert_eq!(load(&p.path, &CID).unwrap(), state);
        let other = vst3::uid(1, 2, 3, 4);
        assert!(load(&p.path, &other).is_err());
        assert!(list(&root, &other).is_empty());
        delete(&p).unwrap();
        assert_eq!(list(&root, &CID).len(), 1);
    }
}
