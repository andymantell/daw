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
