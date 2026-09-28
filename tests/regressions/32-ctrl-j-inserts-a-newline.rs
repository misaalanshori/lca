//! Cycle-4 defect: the editor checked `tui.input.submit` before
//! `tui.input.newLine`, and a bare LF matches `enter` in legacy mode, so
//! **Ctrl+J submitted the prompt instead of inserting a newline** on any
//! terminal without the Kitty protocol. The documented fallback for a
//! terminal that cannot report Shift+Enter was therefore broken too.
//!
//! Fixed by taking pi's order (`editor.ts`: the "New line" block precedes
//! "Submit (Enter)"), pi's full spelling set (`\n`, `\x1b\r`,
//! `\x1b[13;2~`, any ESC+CR sequence), and pi's backslash-Enter workaround
//! (a `\` typed before Enter newlines instead of submitting).
//!
//! Verifies: FR-UI-10 (a multi-line prompt), editor.md section 3.

use lca_tui::widgets::editor::{Editor, EditorEvent};

#[test]
fn ctrl_j_inserts_a_newline_in_legacy_mode() {
    let mut editor = Editor::new();
    editor.insert_str("AAA");
    // A bare LF: Ctrl+J everywhere, and what a terminal sends when
    // Shift+Enter cannot be reported.
    assert_eq!(editor.handle_key("\n"), EditorEvent::Changed);
    editor.insert_str("BBB");
    assert_eq!(editor.lines(), &["AAA".to_string(), "BBB".to_string()]);
}

#[test]
fn every_shift_enter_spelling_inserts_a_newline() {
    for spelling in ["\x1b[13;2u", "\x1b\r", "\x1b[13;2~"] {
        let mut editor = Editor::new();
        editor.insert_str("AAA");
        assert_eq!(
            editor.handle_key(spelling),
            EditorEvent::Changed,
            "{spelling:?} is a newline"
        );
        editor.insert_str("BBB");
        assert_eq!(
            editor.lines(),
            &["AAA".to_string(), "BBB".to_string()],
            "{spelling:?}"
        );
    }
}

#[test]
fn a_backslash_before_enter_inserts_a_newline() {
    let mut editor = Editor::new();
    editor.insert_str("AAA\\");
    assert_eq!(
        editor.handle_key("\r"),
        EditorEvent::Changed,
        "not a submit"
    );
    editor.insert_str("BBB");
    assert_eq!(editor.lines(), &["AAA".to_string(), "BBB".to_string()]);
}

#[test]
fn a_plain_enter_still_submits() {
    let mut editor = Editor::new();
    editor.insert_str("hello");
    assert_eq!(
        editor.handle_key("\r"),
        EditorEvent::Submitted("hello".to_string())
    );
}
