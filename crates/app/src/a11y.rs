// SPDX-License-Identifier: GPL-3.0-or-later
//! Keyboard and screen-reader access (roadmap 1.9), tested through the
//! accessibility tree that screen readers read.

use eframe::egui::{self, Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT as _, Queryable as _};

use daw_engine::graph::Input;

use crate::commands::Cmd;
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
    app.notes_ui.open = true;
    app.comment_ui.open = true;
    app.comments.push(daw_project::Comment {
        id: "c1".into(),
        author: "me".into(),
        parent: None,
        track: None,
        start: 0,
        end: None,
        text: "Rushing here".into(),
        resolved: false,
        created: 0,
    });
    let ctx = h.ctx.clone();
    h.state_mut().run(&ctx, Cmd::KeyCommands);
    h.state_mut().run(&ctx, Cmd::FindCommand);
    h.run();
    assert_eq!(unspoken(&h), Vec::<String>::new());
    for name in [
        "Driver",
        "Rescan",
        "Add effect",
        "New comment",
        "Delete comment: Rushing here",
        "Lesson notes",
        "Add tab block",
        "Section for the new tab block",
        "Type a command's name",
        "Find a command to change",
        "Add a key for Loop on or off",
        "Remove Space from Play or stop",
        "Reset the keys for Loop on or off",
    ] {
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
    h.state_mut().notes_ui.open = true;
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

/// Two tracks, a one-bar clip at the start of Guitar (120 BPM, 4/4: a
/// beat is 24,000 frames, a bar 96,000) and a marker at bar 3.
fn with_a_clip() -> Harness<'static, App> {
    let mut h = with_tracks();
    let app = h.state_mut();
    let mut c = crate::timeline::tests::clip(0, 96_000);
    c.label = "backing".into();
    app.tracks[0].clips.push(c);
    crate::timeline::add_marker(&mut app.markers, 3);
    app.rebuild();
    h.run();
    h
}

fn clip_of(app: &App, track: usize) -> &crate::timeline::Clip {
    &app.tracks[track].clips[0]
}

/// Focuses the control whose name contains `label` and settles for a
/// frame, so the arrow keys then stay with it.
fn focus(h: &mut Harness<'_, App>, label: &str) {
    h.get_by_label_contains(label).focus();
    h.run();
    h.run();
    assert!(
        h.get_by_label_contains(label).is_focused(),
        "{label} focused"
    );
}

#[test]
fn clips_and_markers_are_read_and_edited_from_the_keyboard() {
    let mut h = with_a_clip();
    let spoken = names(&h);
    assert!(
        spoken
            .iter()
            .any(|n| n == "backing on Guitar, bar 1 to bar 2"),
        "{spoken:?}"
    );
    assert!(spoken.iter().any(|n| n == "Section A, marker at bar 3"));

    focus(&mut h, "backing on Guitar");
    h.key_press(Key::ArrowRight);
    h.run();
    assert_eq!(clip_of(h.state(), 0).start, 24_000, "Right: a beat later");
    h.key_press_modifiers(Modifiers::SHIFT, Key::ArrowRight);
    h.run();
    assert_eq!(clip_of(h.state(), 0).start, 120_000, "Shift+Right: a bar");
    assert!(
        h.query_by_label("backing on Guitar, bar 2 beat 2 to bar 3 beat 2")
            .is_some(),
        "the name follows the clip: {:?}",
        names(&h)
    );
    h.key_press_modifiers(Modifiers::COMMAND, Key::ArrowRight);
    h.run();
    let c = clip_of(h.state(), 0);
    assert_eq!(
        (c.start, c.offset, c.len),
        (144_000, 24_000, 72_000),
        "Ctrl+Right trims the start"
    );
    h.key_press_modifiers(Modifiers::ALT, Key::ArrowLeft);
    h.run();
    assert_eq!(clip_of(h.state(), 0).len, 48_000, "Alt+Left trims the end");
    h.key_press(Key::ArrowLeft);
    h.run();
    assert_eq!(clip_of(h.state(), 0).start, 120_000, "Left: a beat earlier");

    h.key_press(Key::Enter);
    h.run();
    let id = clip_of(h.state(), 0).id;
    assert!(h.state().timeline.selected.contains(&id), "Enter selects");
    assert!(
        wait_for(&mut h, |a| a.ctl.status().position().0 == 120_000),
        "and goes to the clip's start"
    );
    assert_eq!(
        h.get_by_label_contains("backing on Guitar")
            .accesskit_node()
            .toggled(),
        Some(egui::accesskit::Toggled::True),
        "a screen reader hears that it is selected"
    );
    h.key_press(Key::Space);
    assert!(
        wait_for(&mut h, playing),
        "Space still plays with a clip focused"
    );
    h.key_press(Key::Space);
    assert!(wait_for(&mut h, |a| !playing(a)));

    h.key_press_modifiers(Modifiers::COMMAND, Key::ArrowDown);
    h.run();
    assert!(
        h.state().tracks[0].clips.is_empty(),
        "Ctrl+Down moves it to Bass"
    );
    assert_eq!(clip_of(h.state(), 1).start, 120_000);
    assert!(h.query_by_label_contains("backing on Bass").is_some());
    h.key_press_modifiers(Modifiers::COMMAND, Key::ArrowDown);
    h.run();
    assert_eq!(h.state().tracks[1].clips.len(), 1, "no track below Bass");

    h.key_press(Key::Delete);
    h.run();
    assert!(h.state().tracks[1].clips.is_empty(), "Delete removes it");
    assert!(h.query_by_label_contains("backing on").is_none());
}

#[test]
fn up_and_down_move_between_tracks_and_a_take_moved_becomes_a_clip() {
    let mut h = with_a_clip();
    let app = h.state_mut();
    let take = crate::timeline::tests::clip(48_000, 48_000);
    crate::timeline::add_take(&mut app.tracks[1].clips, take);
    app.rebuild();
    h.run();
    assert!(
        h.query_by_label("Take 1 on Bass, bar 1 beat 3 to bar 2")
            .is_some()
    );

    focus(&mut h, "backing on Guitar");
    h.key_press(Key::ArrowDown);
    h.run();
    h.run();
    assert!(
        h.get_by_label_contains("Take 1 on Bass").is_focused(),
        "Down reaches the nearest clip on Bass"
    );
    h.key_press(Key::ArrowUp);
    h.run();
    h.run();
    assert!(h.get_by_label_contains("backing on Guitar").is_focused());

    focus(&mut h, "Take 1 on Bass");
    h.key_press_modifiers(Modifiers::COMMAND, Key::ArrowUp);
    h.run();
    let guitar = &h.state().tracks[0].clips;
    assert_eq!(guitar.len(), 2);
    let moved = guitar.iter().find(|c| c.start == 48_000).unwrap();
    assert_eq!(
        (moved.take, moved.name()),
        (None, "Take 1 from Bass".into())
    );
}

#[test]
fn markers_are_moved_renamed_looped_and_deleted_from_the_keyboard() {
    let mut h = with_a_clip();
    focus(&mut h, "Section A, marker");
    h.key_press(Key::ArrowRight);
    h.run();
    assert_eq!(h.state().markers[0].bar, 4, "Right: a bar later");
    h.key_press(Key::ArrowLeft);
    h.key_press(Key::ArrowLeft);
    h.key_press(Key::ArrowLeft);
    h.run();
    assert_eq!(h.state().markers[0].bar, 1, "Left stops at bar 1");

    h.key_press_modifiers(Modifiers::COMMAND, Key::Enter);
    h.run();
    assert!(h.state().looping, "Ctrl+Enter loops the section");
    assert_eq!(h.state().loop_bars, (1, 3), "to the end of the song");

    h.key_press(Key::F2);
    h.run();
    h.run();
    let field = h.get_by_role(egui::accesskit::Role::TextInput);
    assert!(
        field.is_focused(),
        "F2 opens the rename with the field focused"
    );
    field.type_text(" verse");
    h.run();
    h.key_press(Key::Enter);
    h.run();
    assert_eq!(h.state().markers[0].name, "Section A verse");

    focus(&mut h, "Section A verse, marker");
    h.key_press(Key::Delete);
    h.run();
    assert!(h.state().markers.is_empty(), "Delete removes the marker");
}

#[test]
fn a_clip_can_be_dragged_to_another_track() {
    let mut h = with_a_clip();
    let from = h.get_by_label_contains("backing on Guitar").rect();
    let to = h.get_by_label_contains("Bass volume").rect();
    let start = from.center();
    let end = egui::pos2(start.x + 30.0, to.center().y);
    assert!(end.y > from.bottom(), "Bass's lane is below Guitar's");
    h.drag_at(start);
    h.run();
    for i in 1..=4 {
        h.hover_at(start.lerp(end, i as f32 / 4.0));
        h.run();
    }
    h.drop_at(end);
    h.run();
    assert!(h.state().tracks[0].clips.is_empty(), "left Guitar");
    assert_eq!(h.state().tracks[1].clips.len(), 1, "landed on Bass");
    assert!(h.query_by_label_contains("backing on Bass").is_some());
}

/// A comment written, replied to, resolved and deleted in the panel ends
/// up in the project document each time.
#[test]
fn comments_are_written_and_deleted_through_the_panel() {
    let mut h = with_tracks();
    h.state_mut().comment_ui.open = true;
    h.run();
    h.get_by_label("New comment").focus();
    h.run();
    h.get_by_label("New comment").type_text("Rushing here");
    h.run();
    h.get_by_label("Add comment").click();
    h.run();
    let doc = |h: &Harness<'_, App>| h.state().proj.doc.get().comments;
    assert_eq!(doc(&h).len(), 1);
    assert_eq!(doc(&h)[0].text, "Rushing here");
    assert_eq!(doc(&h)[0].author, "me");

    h.get_by_label("Reply").click();
    h.run();
    h.get_by_role_and_label(egui::accesskit::Role::MultilineTextInput, "Reply")
        .focus();
    h.run();
    h.get_by_role_and_label(egui::accesskit::Role::MultilineTextInput, "Reply")
        .type_text("Better now");
    h.run();
    h.get_by_label("Send reply").click();
    h.run();
    h.get_by_label("Resolve").click();
    h.run();
    let c = doc(&h);
    assert_eq!(c.len(), 2);
    let root = c.iter().find(|c| c.parent.is_none()).unwrap();
    let reply = c.iter().find(|c| c.parent.is_some()).unwrap();
    assert_eq!(reply.parent.as_deref(), Some(root.id.as_str()));
    assert_eq!(reply.text, "Better now");
    assert!(root.resolved);

    h.state_mut().comment_ui.show_resolved = true;
    h.run();
    h.get_by_label("Delete comment: Rushing here").click();
    h.run();
    h.get_by_label("Delete it and its replies for good").click();
    h.run();
    assert!(doc(&h).is_empty());
}

/// Ctrl+K opens the command search: typing narrows it, the arrow keys
/// move through it and Enter runs the command.
#[test]
fn commands_are_found_by_typing_and_run_with_enter() {
    let mut h = with_tracks();
    h.key_press_modifiers(Modifiers::COMMAND, Key::K);
    h.run();
    assert!(
        h.get_by_label("Type a command's name").is_focused(),
        "the search has the keyboard"
    );
    h.get_by_label("Type a command's name").type_text("metro");
    h.run();
    h.key_press(Key::Enter);
    h.run();
    assert!(h.state().metronome.enabled, "Enter ran Metronome on or off");
    assert!(!h.state().commands.palette_open(), "and closed it");

    // Letters typed into the search don't run their commands.
    h.key_press_modifiers(Modifiers::COMMAND, Key::K);
    h.run();
    h.get_by_label("Type a command's name").type_text("loop");
    h.run();
    assert!(!h.state().looping);
    h.key_press(Key::ArrowDown);
    h.key_press(Key::ArrowUp);
    h.key_press(Key::Enter);
    h.run();
    assert!(h.state().looping, "Loop on or off is first for \"loop\"");

    // Esc closes it without running anything.
    h.key_press_modifiers(Modifiers::COMMAND, Key::K);
    h.run();
    h.key_press(Key::Escape);
    h.run();
    assert!(!h.state().commands.palette_open());
    assert!(h.state().looping && h.state().metronome.enabled);
}

/// A key is given to a command in Settings → Key commands, by pressing
/// it; one that another command has moves only when asked.
#[test]
fn keys_are_changed_in_the_key_commands_window() {
    let mut h = with_tracks();
    let ctx = h.ctx.clone();
    h.state_mut().run(&ctx, Cmd::KeyCommands);
    h.run();
    // Typing narrows the list, as in the command search.
    h.get_by_label("Find a command to change").focus();
    h.run();
    h.get_by_label("Find a command to change")
        .type_text("o on or");
    h.run();
    assert!(h.query_by_label("Add a key for Save").is_none());
    // More commands match than fit: bring it into view first.
    h.get_by_label("Add a key for Loop on or off")
        .scroll_to_me();
    h.run();
    h.get_by_label("Add a key for Loop on or off").click();
    h.run();
    assert!(h.state().commands.editor.capturing.is_some());
    h.key_press(Key::F6);
    h.run();
    assert_eq!(h.state().commands.keymap.text(Cmd::Loop), "L, F6");
    h.key_press(Key::F6);
    h.run();
    assert!(h.state().looping, "F6 loops now");

    // K is the metronome's: asked first, then moved.
    h.get_by_label("Add a key for Loop on or off").click();
    h.run();
    h.key_press(Key::K);
    h.run();
    assert_eq!(h.state().commands.keymap.text(Cmd::Metronome), "K");
    h.get_by_label("Move it").click();
    h.run();
    assert_eq!(h.state().commands.keymap.text(Cmd::Loop), "L, F6, K");
    assert_eq!(h.state().commands.keymap.text(Cmd::Metronome), "");

    // Keys that move the keyboard around can't be given.
    h.get_by_label("Add a key for Loop on or off").click();
    h.run();
    h.key_press(Key::Tab);
    h.run();
    assert_eq!(h.state().commands.keymap.text(Cmd::Loop), "L, F6, K");

    h.get_by_label("Remove L from Loop on or off").click();
    h.run();
    assert_eq!(h.state().commands.keymap.text(Cmd::Loop), "F6, K");
    h.get_by_label("Reset the keys for Loop on or off").click();
    h.run();
    assert_eq!(h.state().commands.keymap.text(Cmd::Loop), "L");
    h.get_by_label("Reset the keys for Metronome on or off")
        .click();
    h.run();
    assert_eq!(h.state().commands.keymap.text(Cmd::Metronome), "K");
}

/// Menu entries run the same commands and show their keys.
#[test]
fn menus_show_each_commands_keys() {
    let mut h = with_tracks();
    h.get_by_label("Transport").click();
    h.run();
    assert!(h.query_by_label_contains("Ctrl+K").is_none());
    h.get_by_label_contains("Metronome on or off").click();
    h.run();
    assert!(h.state().metronome.enabled);
}

#[test]
fn fades_and_volume_are_set_from_the_keyboard_and_kept() {
    let mut h = with_a_clip();
    focus(&mut h, "backing on Guitar");
    h.key_press(Key::F);
    h.run();
    h.run();
    assert!(h.state().timeline.fades_open, "F opens fades and volume");
    for name in [
        "Fade in length",
        "Fade in shape",
        "Fade out length",
        "Fade out shape",
        "Clip volume",
    ] {
        assert!(names(&h).iter().any(|n| n == name), "no control {name}");
    }
    assert_eq!(unspoken(&h), Vec::<String>::new());
    h.get_by_label("Fade in length").focus();
    h.run();
    h.get_by_label("Fade in length").type_text("250");
    h.run();
    h.key_press(Key::Enter);
    h.run();
    let c = clip_of(h.state(), 0);
    assert_eq!(c.fade_in.len, 12_000, "250 ms at 48 kHz");
    assert!(
        h.query_by_label_contains("backing on Guitar, bar 1 to bar 2, fades in over 250 ms")
            .is_some(),
        "{:?}",
        names(&h)
    );

    // Kept through the project document, as saving and opening do.
    let app = h.state_mut();
    app.tracks[0].clips[0].gain_db = -4.5;
    app.tracks[0].clips[0].add_point(1000, -6.0);
    let mut doc = daw_project::ProjectDoc::new(1);
    doc.set(&app.project());
    app.apply(doc.get());
    let c = clip_of(h.state(), 0);
    assert_eq!(
        (c.fade_in.len, c.gain_db, c.envelope.len()),
        (12_000, -4.5, 1)
    );
}

#[test]
fn a_fade_is_dragged_from_its_handle() {
    let mut h = with_a_clip();
    let r = h.get_by_label_contains("backing on Guitar").rect();
    // The fade-in handle sits at the clip's top left corner.
    let start = egui::pos2(r.left() + 4.0, r.top() + 4.0);
    let end = egui::pos2(start.x + 200.0, start.y);
    h.hover_at(start);
    h.run();
    h.drag_at(start);
    h.run();
    for i in 1..=4 {
        h.hover_at(start.lerp(end, i as f32 / 4.0));
        h.run();
    }
    h.drop_at(end);
    h.run();
    let c = clip_of(h.state(), 0);
    assert_eq!(c.start, 0, "the clip stays put");
    // Snapped to the grid: a whole number of beats.
    assert!(
        c.fade_in.len > 0 && c.fade_in.len.is_multiple_of(24_000),
        "{}",
        c.fade_in.len
    );
}

/// What a screen reader would find wrong with the controls on screen: a
/// control with no name or only symbols for one, and a slider or number
/// whose value is read without its unit.
fn problems(h: &Harness<'_, App>) -> Vec<String> {
    use egui::accesskit::Role;
    let mut out = Vec::new();
    for n in h.root().children_recursive() {
        let node = n.accesskit_node();
        let role = node.role();
        if !egui::accessibility::is_input(role) {
            continue;
        }
        let label = node.label().unwrap_or_default();
        if !label.chars().any(char::is_alphanumeric) {
            out.push(format!("{role:?} named {label:?}"));
        }
        if role == Role::Slider && node.value().is_none() {
            out.push(format!("{label}: value without its unit"));
        }
    }
    out.sort();
    out.dedup();
    out
}

/// A project with something in every pane: an audio track with every
/// built-in effect and a volume lane, an instrument track with a note
/// clip, and a bus.
fn full_project() -> crate::agent::driver::Driver {
    use serde_json::json;
    let mut d = crate::agent::driver::Driver::new();
    let id = |v: serde_json::Value, k: &str| v[k].as_str().unwrap().to_string();
    let audio = id(
        d.call("add_track", json!({"kind": "audio"})).unwrap(),
        "track",
    );
    for k in daw_effects::Kind::ALL {
        d.call("add_effect", json!({"track": audio, "effect": k.id()}))
            .unwrap();
    }
    d.call(
        "edit_automation",
        json!({"track": audio, "param": "gain", "add": [
            {"bar": 1, "value": -20.0}, {"bar": 3, "value": -6.0}
        ]}),
    )
    .unwrap();
    let inst = id(
        d.call("add_track", json!({"kind": "instrument"})).unwrap(),
        "track",
    );
    let clip = id(
        d.call("add_note_clip", json!({"track": inst, "bar": 1, "bars": 2}))
            .unwrap(),
        "clip",
    );
    d.call(
        "edit_notes",
        json!({"clip": clip, "add": [{"pitch": 60, "beat": 0, "beats": 1}]}),
    )
    .unwrap();
    d.call("add_track", json!({"kind": "bus"})).unwrap();
    d
}

/// Every pane, each shown full size, and the effects and instrument
/// windows: every control has a name in words and every slider says its
/// value with its unit. A new pane is added to `workspace::PANES`, so it
/// is checked here before it merges.
#[test]
fn every_pane_names_its_controls_and_says_its_values() {
    let mut d = full_project();
    let h = &mut d.h;
    h.run();
    let mut found = Vec::new();
    let mut check = |h: &Harness<'_, App>, what: &str| {
        found.extend(problems(h).into_iter().map(|p| format!("{what}: {p}")));
    };
    check(h, "the main window");
    h.state_mut().effects.open_track = Some(0);
    h.run();
    check(h, "the effects window");
    h.state_mut().effects.open_track = None;
    h.state_mut().instrument_open = Some(1);
    h.run();
    check(h, "the instrument window");
    h.state_mut().instrument_open = None;
    let clip = h.state().tracks[1].note_clips[0].id.clone();
    h.state_mut().notes_editor.edit(&clip);
    for p in crate::workspace::PANES {
        h.state_mut().workspace.open(p);
        h.state_mut().workspace.full = Some(p);
        h.run();
        h.run();
        check(h, p.title());
    }
    assert_eq!(found, Vec::<String>::new());
}

/// The value a screen reader reads for the control named `name`.
fn value_of(h: &Harness<'_, App>, name: &str) -> Option<String> {
    use egui::accesskit::Role;
    h.query_all_by_label(name)
        .find(|n| matches!(n.accesskit_node().role(), Role::Slider | Role::SpinButton))
        .and_then(|n| n.accesskit_node().value())
}

#[test]
fn values_are_read_with_their_units() {
    let mut h = with_tracks();
    let app = h.state_mut();
    app.tracks[1].mix.pan = -0.3;
    app.tracks[1].mix.gain_db = -6.0;
    app.workspace.open(crate::workspace::Pane::Mixer);
    h.run();
    h.run();
    assert_eq!(value_of(&h, "Bass pan").as_deref(), Some("30% left"));
    assert_eq!(value_of(&h, "Bass volume").as_deref(), Some("-6.0 dB"));

    // A built-in effect's settings, as its panel shows them.
    let mut d = crate::agent::driver::Driver::new();
    let t = d
        .call("add_track", serde_json::json!({"kind": "audio"}))
        .unwrap()["track"]
        .as_str()
        .unwrap()
        .to_string();
    d.call(
        "add_effect",
        serde_json::json!({"track": t, "effect": "compressor"}),
    )
    .unwrap();
    d.h.state_mut().effects.open_track = Some(0);
    d.h.run();
    let ratio = value_of(&d.h, "Ratio").unwrap();
    assert!(ratio.ends_with(":1"), "{ratio}");
    let attack = value_of(&d.h, "Attack").unwrap();
    assert!(attack.ends_with("ms"), "{attack}");
}

/// The note editor with a clip of two notes on an instrument track.
fn with_notes() -> Harness<'static, App> {
    let mut d = full_project();
    let clip = d.h.state().tracks[1].note_clips[0].id.clone();
    d.call(
        "edit_notes",
        serde_json::json!({"clip": clip, "add": [{"pitch": 64, "beat": 4, "beats": 2}]}),
    )
    .unwrap();
    let mut h = d.h;
    h.state_mut().notes_editor.edit(&clip);
    h.state_mut()
        .workspace
        .open(crate::workspace::Pane::NotesEditor);
    h.run();
    h.run();
    h
}

fn notes(h: &Harness<'_, App>) -> Vec<(u8, i64)> {
    let c = &h.state().tracks[1].note_clips[0];
    c.notes.iter().map(|n| (n.pitch, n.start)).collect()
}

#[test]
fn notes_are_read_and_edited_from_the_keyboard() {
    use daw_project::PPQ;
    let mut h = with_notes();
    assert!(
        h.query_by_label("C4 at bar 1, 1 beat long, velocity 100")
            .is_some(),
        "{:?}",
        names(&h)
    );
    assert!(
        h.query_by_label("E4 at bar 2, 2 beats long, velocity 100")
            .is_some()
    );
    focus(&mut h, "C4 at bar 1");
    h.key_press(Key::ArrowUp);
    h.run();
    assert_eq!(notes(&h)[0], (61, 0), "Up: a semitone");
    h.key_press_modifiers(Modifiers::SHIFT, Key::ArrowUp);
    h.run();
    assert_eq!(notes(&h)[0], (73, 0), "Shift+Up: an octave");
    h.key_press(Key::ArrowRight);
    h.run();
    let moved = notes(&h)[0].1;
    assert!(
        moved > 0 && moved % (PPQ / 4) == 0,
        "Right: a grid step, {moved}"
    );
    assert!(
        h.query_by_label_contains("C#5 at bar 1 beat").is_some(),
        "the name follows the note: {:?}",
        names(&h)
    );
    h.key_press(Key::Enter);
    h.run();
    assert_eq!(h.state().notes_editor.selected.len(), 1, "Enter selects");
    h.key_press(Key::Delete);
    h.run();
    assert_eq!(notes(&h), vec![(64, 4 * PPQ)], "Delete removes it");
}

#[test]
fn an_automation_lane_is_read_heard_and_set_from_the_keyboard() {
    let mut d = full_project();
    let h = &mut d.h;
    h.state_mut()
        .workspace
        .open(crate::workspace::Pane::Automation);
    h.run();
    h.run();
    let words = "Volume lane: -20.0 dB at bar 1, rising to -6.0 dB at bar 3";
    assert!(h.query_by_label(words).is_some(), "{:?}", names(h));
    h.get_by_label(words).focus();
    h.run();
    h.run();
    // Heard: a tone that rises with the lane, its words at the foot.
    let ctx = h.ctx.clone();
    h.state_mut().run(&ctx, Cmd::HearValue);
    h.run();
    assert_eq!(h.state().said.text(), Some(words));
    assert!(
        wait_for(h, |a| a.ctl.status().preview().is_some()),
        "it plays"
    );
    // Up at the playhead (bar 1) raises the lane there.
    h.key_press(Key::ArrowUp);
    h.run();
    let first = h.state().tracks[0].automation[0].points[0].value;
    assert!(first > -20.0 && first < -18.0, "{first}");
    assert!(
        h.query_by_label_contains("Volume lane: -18.6 dB at bar 1")
            .is_some(),
        "{:?}",
        names(h)
    );
}

/// Hear the value plays the control with the keyboard, or the one last
/// moved, and says it; with neither it says what to do.
#[test]
fn the_focused_value_is_heard_and_said() {
    let mut h = with_tracks();
    h.state_mut().workspace.open(crate::workspace::Pane::Mixer);
    h.run();
    h.run();
    let ctx = h.ctx.clone();
    h.state_mut().run(&ctx, Cmd::HearValue);
    h.run();
    assert!(
        h.state()
            .said
            .text()
            .unwrap()
            .starts_with("Nothing to hear")
    );
    // Moved by something other than the person: still nothing.
    h.state_mut().tracks[1].mix.pan = 0.5;
    h.run();
    h.state_mut().run(&ctx, Cmd::HearValue);
    h.run();
    assert!(
        h.state()
            .said
            .text()
            .unwrap()
            .starts_with("Nothing to hear")
    );

    h.query_all_by_label("Bass pan")
        .find(|n| n.accesskit_node().role() == egui::accesskit::Role::Slider)
        .unwrap()
        .focus();
    h.run();
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::H);
    // (Not run: the short tone may be over by the time the frames settle.)
    assert!(wait_for(&mut h, |a| a.ctl.status().preview().is_some()));
    assert_eq!(h.state().said.text(), Some("50% right"));
    h.run();
    assert!(
        h.query_by_label("50% right").is_some(),
        "the words are on screen for a screen reader"
    );
}

/// An assistant hears a value as the person does, and gets its words.
#[test]
fn an_assistant_plays_a_value_to_the_person() {
    use serde_json::json;
    let mut d = full_project();
    let t = d.h.state().tracks[0].key.clone();
    d.call(
        "set_parameter",
        json!({"track": t, "param": "pan", "value": -1.0}),
    )
    .unwrap();
    let r = d
        .call("hear_value", json!({"track": t, "param": "pan"}))
        .unwrap();
    assert_eq!(r["text"], "Audio 1 pan, 100% left");
    assert!(wait_for(&mut d.h, |a| a.ctl.status().preview().is_some()));
    let r = d
        .call("hear_value", json!({"track": t, "lane": true}))
        .unwrap();
    assert_eq!(
        r["text"],
        "Audio 1, Volume lane: -20.0 dB at bar 1, rising to -6.0 dB at bar 3"
    );
    let r = d.call("hear_value", json!({"track": "master"})).unwrap();
    assert_eq!(r["text"], "Master volume, 0 dB");
    assert!(
        d.call(
            "hear_value",
            json!({"track": t, "param": "pan", "lane": true})
        )
        .is_err()
    );
    assert!(d.call("hear_value", json!({"track": "nope"})).is_err());
    // A read-only assistant may: it changes nothing.
    d.access = crate::agent::Access::Read;
    assert!(d.call("hear_value", json!({"track": "master"})).is_ok());
}

/// Controls stay where they are as things change, so someone who finds
/// them by position (with a magnifier, or from memory) finds them again:
/// arming, muting, soloing, adding effects, recording, looping and
/// playing move nothing in the mixer or on the transport bar.
#[test]
fn controls_stay_in_place() {
    let mut h = with_tracks();
    h.state_mut().workspace.open(crate::workspace::Pane::Mixer);
    h.run();
    h.run();
    let watched = [
        "Arm Guitar for recording",
        "Guitar pan",
        "Mute Guitar",
        "Effects on Guitar",
        "Bass pan",
        "Solo Bass",
        "Effects on Bass",
        "Back to the start",
        "⏺ Record",
        "Click level",
        "Tempo",
        "Undo",
    ];
    let places = |h: &Harness<'_, App>| -> Vec<egui::Rect> {
        watched
            .iter()
            .map(|n| h.query_all_by_label(n).next().unwrap().rect())
            .collect()
    };
    let before = places(&h);
    let app = h.state_mut();
    app.tracks[0].arm = true;
    app.tracks[0].mix.mute = true;
    app.tracks[1].mix.solo = true;
    for _ in 0..12 {
        let slot = app.effects.create_builtin(daw_effects::Kind::Eq, app.rate);
        app.tracks[0].fx.push(slot);
    }
    app.rebuild();
    h.run();
    h.key_press(Key::L);
    h.key_press(Key::K);
    h.key_press(Key::Space);
    assert!(wait_for(&mut h, playing));
    h.step();
    h.step();
    let after = places(&h);
    for ((n, a), b) in watched.iter().zip(&before).zip(&after) {
        assert!(
            (a.min - b.min).length() < 0.5,
            "{n} moved from {a:?} to {b:?}"
        );
    }
}
