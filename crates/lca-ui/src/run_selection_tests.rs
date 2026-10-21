//! gh #403 gap 1: a transcript replacement must not keep the previous
//! transcript's selection (or its multi-click history).
//!
//! Split from `run_tests.rs` for the 1,200-line ceiling.
//!
//! **Why this is a wrong-data bug, not a cosmetic one.** A selection lives in
//! *content* coordinates (row/column in the rendered document), not screen
//! coordinates - that is what lets it survive scrolling. So when the
//! transcript is replaced, the rectangles do not point at "the same place";
//! they point at whatever now occupies those rows. The rows below pin that
//! consequence, not just the highlight.

use super::reset_selection_if_replaced;
use crate::run::tests::chat;
use crate::run::{Screen, switch_screen};
use lca_tui::engine::alt_screen::AltScreenRenderer;
use lca_tui::engine::main_screen::MainScreenRenderer;
use lca_tui::engine::selection::{Granularity, SelectionPoint};
use lca_tui::engine::terminal::FakeTerminal;

/// A record set the replay paths can load: text a stale selection would
/// provably mis-address.
fn replacement_records() -> Vec<lca_protocol::Record> {
    vec![
        lca_protocol::Record::SessionStart {
            v: 1,
            ts: 0,
            agent_version: "0.6.0".to_string(),
            abi_version: "0.6".to_string(),
            working_dir: "/tmp/replacement".to_string(),
        },
        lca_protocol::Record::User {
            v: 1,
            ts: 0,
            id: "r1".to_string(),
            parent: None,
            content: "REPLACEMENT TEXT".to_string(),
            attachments: Vec::new(),
            queue: None,
        },
    ]
}

/// Paint `lines` as the alt screen's last frame: selection is in *content*
/// coordinates, so rows must exist in `previous` before a selection can
/// address (or be read back from) them.
fn render_frame(screen: &mut Screen, term: &mut FakeTerminal, lines: &[&str]) {
    if let Screen::Alt(alt) = screen {
        let owned: Vec<String> = lines.iter().map(|s| s.to_string()).collect();
        alt.render_lines(term, owned, 80, 24);
    }
}

fn alt_screen(term: &mut FakeTerminal) -> Screen {
    let mut screen = Screen::Main(MainScreenRenderer::new());
    switch_screen(&mut screen, true, term);
    screen
}

/// Start a character selection and extend it, the shape a drag produces.
fn select_range(alt: &mut AltScreenRenderer, from: (u16, u16), to: (u16, u16)) {
    alt.selection.start(
        SelectionPoint {
            row: from.0,
            col: from.1,
        },
        Granularity::Char,
        1,
        "",
    );
    alt.selection.update(
        SelectionPoint {
            row: to.0,
            col: to.1,
        },
        "",
    );
    alt.selection.end();
}

/// The text a copy would take right now.
fn copied(screen: &Screen) -> String {
    screen.selected_text()
}

// Verifies: gh #403 gap 1 - a session switch (the `/resume` path) drops the
// selection instead of leaving it over the new transcript's rows.
#[test]
fn a_session_switch_drops_the_selection() {
    let mut term = FakeTerminal::new(80, 24);
    let mut screen = alt_screen(&mut term);
    render_frame(&mut screen, &mut term, &["old transcript row"]);
    let mut chat = chat();
    chat.transcript
        .push_notice("old transcript row".to_string());
    if let Screen::Alt(alt) = &mut screen {
        select_range(alt, (0, 0), (0, 5));
    }

    let seen = chat.transcript.generation();
    chat.replay_records(&replacement_records());
    let _ = reset_selection_if_replaced(&mut screen, &chat, seen);

    assert!(
        copied(&screen).is_empty(),
        "a replaced transcript must not keep a selection: {:?}",
        copied(&screen)
    );
}

// Verifies: gh #403 gap 1 - a `/tree` branch jump drops the selection too.
#[test]
fn a_tree_branch_jump_drops_the_selection() {
    let mut term = FakeTerminal::new(80, 24);
    let mut screen = alt_screen(&mut term);
    render_frame(&mut screen, &mut term, &["before the branch"]);
    let mut chat = chat();
    chat.transcript.push_notice("before the branch".to_string());
    if let Screen::Alt(alt) = &mut screen {
        select_range(alt, (0, 0), (0, 4));
    }

    // `branch_and_replay` is the same replay seam a tree jump lands on.
    let seen = chat.transcript.generation();
    chat.replay_records(&replacement_records());
    let _ = reset_selection_if_replaced(&mut screen, &chat, seen);

    assert!(copied(&screen).is_empty());
}

// Verifies: gh #403 gap 1 - the wrong-data consequence. A selection made
// before the replacement must not yield the pre-replacement text after it.
// This is the row that says the defect corrupts what a copy returns, rather
// than merely painting a stale highlight.
#[test]
fn a_pre_replacement_selection_never_yields_the_old_text() {
    let mut term = FakeTerminal::new(80, 24);
    let mut screen = alt_screen(&mut term);
    render_frame(
        &mut screen,
        &mut term,
        &["SECRET OLD CONTENT", "second old row"],
    );
    let mut chat = chat();
    chat.transcript
        .push_notice("SECRET OLD CONTENT".to_string());
    chat.transcript.push_notice("second old row".to_string());
    if let Screen::Alt(alt) = &mut screen {
        select_range(alt, (0, 0), (1, 6));
    }
    let before = copied(&screen);
    assert!(
        before.contains("SECRET OLD CONTENT") || !before.is_empty(),
        "the fixture selects the old text: {before:?}"
    );

    let seen = chat.transcript.generation();
    chat.replay_records(&replacement_records());
    let _ = reset_selection_if_replaced(&mut screen, &chat, seen);

    let after = copied(&screen);
    assert!(
        after.is_empty() && !after.contains("SECRET OLD CONTENT"),
        "copying after a replacement must not hand back text the user \
         selected from a transcript that is gone: {after:?}"
    );
}

// Verifies: gh #403 gap 1 / design point 2 - the reset drops the multi-click
// history as well as the rectangles. pi's `resetTextSelection()` clears its
// `lastClick`; without that the next click on a row can continue the old
// click run and select a word or a whole line the user never asked for.
#[test]
fn the_reset_drops_the_multi_click_history() {
    let mut term = FakeTerminal::new(80, 24);
    let mut screen = alt_screen(&mut term);
    render_frame(&mut screen, &mut term, &["one two three"]);
    let mut chat = chat();
    chat.transcript.push_notice("one two three".to_string());
    if let Screen::Alt(alt) = &mut screen {
        // A double-click's run. The renderer's own `click_count` is the
        // authoritative one - it is what `handle_input` reads to decide the
        // next click's granularity (alt_screen.rs:514) - and `start` also
        // mirrors it onto the selection.
        alt.set_click_run(2);
        alt.selection.start(
            SelectionPoint { row: 0, col: 0 },
            Granularity::Word,
            2,
            "one two three",
        );
        alt.selection.end();
        assert_eq!(alt.click_run(), 2, "the fixture built a click run");
    }

    let seen = chat.transcript.generation();
    chat.replay_records(&replacement_records());
    let _ = reset_selection_if_replaced(&mut screen, &chat, seen);

    if let Screen::Alt(alt) = &screen {
        assert_eq!(
            alt.click_run(),
            0,
            "the reset must clear the renderer's click run, or the next click \
             continues it"
        );
        assert_eq!(
            alt.selection.click_count, 0,
            "and the selection's mirrored copy with it"
        );
    }
}

// Verifies: gh #403 gap 1 / design point 3 - the mirror-image bug. A resize
// does NOT replace the transcript, so the selection must survive it.
#[test]
fn a_resize_keeps_the_selection() {
    let mut term = FakeTerminal::new(80, 24);
    let mut screen = alt_screen(&mut term);
    render_frame(&mut screen, &mut term, &["resize me"]);
    let chat = chat();
    if let Screen::Alt(alt) = &mut screen {
        select_range(alt, (0, 0), (0, 5));
    }
    let before = copied(&screen);
    assert!(!before.is_empty(), "the fixture selects text");

    // No replay happened, so the loop's "did the transcript change" answer is
    // still no and the reset is a no-op.
    let seen = chat.transcript.generation();
    let _ = reset_selection_if_replaced(&mut screen, &chat, seen);

    assert_eq!(
        copied(&screen),
        before,
        "a resize must not lose the user's selection"
    );
}

// Verifies: gh #403 gap 1 / design point 3 - a streaming delta appends to the
// transcript; the user's earlier selection must survive it.
#[test]
fn a_streaming_delta_keeps_the_selection() {
    let mut term = FakeTerminal::new(80, 24);
    let mut screen = alt_screen(&mut term);
    render_frame(&mut screen, &mut term, &["streamed so far"]);
    let mut chat = chat();
    chat.transcript.push_notice("streamed so far".to_string());
    if let Screen::Alt(alt) = &mut screen {
        select_range(alt, (0, 0), (0, 6));
    }
    let before = copied(&screen);
    assert!(!before.is_empty());

    // A delta appends an entry; it neither clears nor replays, so the
    // generation does not move and the selection survives.
    let seen = chat.transcript.generation();
    chat.transcript.push_notice("a streamed line".to_string());
    let _ = reset_selection_if_replaced(&mut screen, &chat, seen);

    assert_eq!(
        copied(&screen),
        before,
        "appending to a transcript must not lose the user's selection"
    );
}

// Verifies: gh #403 gap 1 - and once the transcript really was replaced, a
// second reset is a no-op rather than an error (the seam runs every pass).
#[test]
fn the_reset_is_idempotent_across_passes() {
    let mut term = FakeTerminal::new(80, 24);
    let mut screen = alt_screen(&mut term);
    render_frame(&mut screen, &mut term, &["old"]);
    let mut chat = chat();
    chat.transcript.push_notice("old".to_string());
    if let Screen::Alt(alt) = &mut screen {
        select_range(alt, (0, 0), (0, 3));
    }
    let seen = chat.transcript.generation();
    chat.replay_records(&replacement_records());
    let seen = reset_selection_if_replaced(&mut screen, &chat, seen);
    let seen = reset_selection_if_replaced(&mut screen, &chat, seen);
    let _ = reset_selection_if_replaced(&mut screen, &chat, seen);
    assert!(copied(&screen).is_empty());
}
