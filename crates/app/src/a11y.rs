// SPDX-License-Identifier: GPL-3.0-or-later
//! Keyboard and screen-reader access (roadmap 1.9), tested through the
//! accessibility tree that screen readers read.

use eframe::egui::{self, Key};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT as _, Queryable as _};

use daw_engine::graph::Input;

use crate::{App, Track};

fn harness() -> Harness<'static, App> {
    Harness::builder()
        .with_size([1400.0, 900.0])
        .allow_missing_glyphs()
        .build_eframe(|_cc| App::new(Vec::new()))
}

/// Every control's name, as a screen reader would announce it.
fn names(h: &Harness<'_, App>) -> Vec<String> {
    h.root()
        .children_recursive()
        .filter(|n| egui::accessibility::is_input(n.accesskit_node().role()))
        .filter_map(|n| n.accesskit_node().label())
        .collect()
}

/// A name made only of symbols (an icon, a "+") tells a listener nothing.
fn unspoken(h: &Harness<'_, App>) -> Vec<String> {
    names(h)
        .into_iter()
        .filter(|n| !n.chars().any(char::is_alphanumeric))
        .collect()
}

/// The app with two tracks, the first with an input so it can be armed.
fn with_tracks() -> Harness<'static, App> {
    let mut h = harness();
    let app = h.state_mut();
    app.tracks.push(Track::new("Guitar".into()));
    app.tracks.push(Track::new("Bass".into()));
    app.tracks[0].input = Some(Input::mono(0));
    app.rebuild();
    h.run();
    h
}

/// Waits for the (silent) audio thread to catch up with the transport.
fn wait_for(h: &mut Harness<'_, App>, what: impl Fn(&App) -> bool) -> bool {
    for _ in 0..100 {
        h.step();
        if what(h.state()) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    false
}

fn playing(app: &App) -> bool {
    app.ctl.status().playing()
}

#[test]
fn every_control_has_a_spoken_name() {
    // The harness checks that each control has a name whenever the
    // window settles; this also rules out names that are only symbols.
    let mut h = harness();
    h.run();
    assert_eq!(unspoken(&h), Vec::<String>::new());

    let mut h = with_tracks();
    assert_eq!(unspoken(&h), Vec::<String>::new());
    for name in [
        "Back to the start",
        "Arm Guitar for recording",
        "Bass volume",
        "Bass pan",
        "Practice mute Guitar",
        "Effects on Bass",
        "Zoom in",
        "Undo",
    ] {
        assert!(
            names(&h).iter().any(|n| n == name),
            "no control named {name}"
        );
    }

    // And in every window.
    let app = h.state_mut();
    app.show_devices = true;
    app.effects.show_manager = true;
    app.effects.open_track = Some(0);
    app.tuner.open = true;
    app.perf.open = true;
    app.export.open = true;
    app.notes_open = true;
    app.shortcuts_open = true;
    h.run();
    assert_eq!(unspoken(&h), Vec::<String>::new());
    for name in ["Driver", "Rescan", "Add effect"] {
        assert!(
            names(&h).iter().any(|n| n == name),
            "no control named {name}"
        );
    }
}

#[test]
fn the_transport_works_from_the_keyboard() {
    let mut h = with_tracks();
    h.key_press(Key::Space);
    assert!(wait_for(&mut h, playing), "Space plays");
    h.key_press(Key::Space);
    assert!(wait_for(&mut h, |a| !playing(a)), "Space stops");
    assert!(wait_for(&mut h, |a| a.ctl.status().position().0 > 0));
    h.key_press(Key::Home);
    assert!(
        wait_for(&mut h, |a| a.ctl.status().position().0 == 0),
        "Home"
    );

    h.key_press(Key::R);
    h.run();
    assert!(h.state().recording, "R arms the transport for recording");
    h.key_press(Key::L);
    h.run();
    assert!(h.state().looping, "L loops");
    h.key_press(Key::K);
    h.run();
    assert!(h.state().metronome.enabled, "K turns the metronome on");
    h.key_press(Key::C);
    h.run();
    assert!(h.state().count_in, "C turns the count-in on");
    h.key_press(Key::K);
    h.key_press(Key::R);
    h.run();
    assert!(!h.state().metronome.enabled && !h.state().recording);
}

#[test]
fn keys_type_into_text_fields_instead() {
    let mut h = with_tracks();
    h.state_mut().notes_open = true;
    h.run();
    h.get_by_role(egui::accesskit::Role::MultilineTextInput)
        .focus();
    h.run();
    h.key_press(Key::Space);
    h.key_press(Key::R);
    h.run();
    assert!(!h.state().recording);
    assert!(!wait_for(&mut h, playing));
}

/// Tab moves through the controls and Enter presses the focused one, so
/// a track can be armed and recording started without a mouse.
#[test]
fn tab_and_enter_reach_and_press_controls() {
    fn tab_to(h: &mut Harness<'_, App>, name: &str) {
        for _ in 0..200 {
            h.key_press(Key::Tab);
            h.run();
            if h.query_by_label(name).is_some_and(|n| n.is_focused()) {
                return;
            }
        }
        panic!("Tab never reached {name}");
    }
    let mut h = with_tracks();
    tab_to(&mut h, "Arm Guitar for recording");
    h.key_press(Key::Enter);
    h.run();
    assert!(h.state().tracks[0].arm, "Enter arms the track");

    tab_to(&mut h, "⏺ Record");
    h.key_press(Key::Enter);
    h.run();
    assert!(h.state().recording, "Enter presses Record");
}
