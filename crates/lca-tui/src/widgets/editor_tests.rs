//! The editor's tests, split out to keep `editor.rs` under the
//! 1,200-line ceiling (S3/S10).

use super::*;
use crate::engine::core::extract_cursor_position;

#[test]
fn typing_and_newline() {
    let mut e = Editor::new();
    e.insert_str("hello");
    e.newline();
    e.insert_str("world");
    assert_eq!(e.lines(), &["hello", "world"]);
    assert_eq!(e.cursor_line, 1);
}

// Verifies: R7 - the sticky column survives a shorter line.
#[test]
fn the_sticky_column_survives_shorter_lines() {
    let mut e = Editor::new();
    e.set_text("long line here\nx\nanother long line");
    e.cursor_up();
    e.cursor_up();
    e.cursor_line_start();
    for _ in 0..14 {
        e.cursor_right();
    }
    assert_eq!(e.cursor_col, 14);
    e.cursor_down();
    assert_eq!(e.cursor_col, 1, "clamped to the short line");
    e.cursor_down();
    assert_eq!(e.cursor_col, 14, "sticky column restored");
}

// Verifies: R7 - jump mode moves to the next occurrence of a character.
#[test]
fn jump_mode_moves_to_the_next_character() {
    let mut e = Editor::new();
    e.set_text("abcabc");
    e.cursor_line_start();
    e.handle_key("\x1d"); // Ctrl+]
    e.handle_key("b");
    assert_eq!(e.cursor_col, 1);
    e.handle_key("\x1d");
    e.handle_key("b");
    assert_eq!(e.cursor_col, 4);
}

#[test]
fn enter_submits_and_resets() {
    let mut e = Editor::new();
    e.insert_str("hi");
    let ev = e.handle_key("\r");
    assert_eq!(ev, EditorEvent::Submitted("hi".to_string()));
    assert_eq!(e.text(), "");
}

#[test]
fn shift_enter_inserts_newline() {
    let mut e = Editor::new();
    e.insert_str("a");
    e.handle_key("\x1b[13;2u");
    e.insert_str("b");
    assert_eq!(e.lines(), &["a", "b"]);
}

// A terminal that cannot report Shift+Enter at all: a `\` typed before
// Enter inserts a newline instead of submitting (pi's workaround).
#[test]
fn backslash_enter_inserts_a_newline() {
    let mut e = Editor::new();
    e.insert_str("a\\");
    assert_eq!(
        e.handle_key("\r"),
        EditorEvent::Changed,
        "newline, not submit"
    );
    e.insert_str("b");
    assert_eq!(e.lines(), &["a", "b"]);
}

// The fallback pi documents: Ctrl+J is a newline in every dialect,
// including a legacy terminal where Shift+Enter is indistinguishable
// from Enter.
#[test]
fn ctrl_j_inserts_newline_in_every_dialect() {
    let mut e = Editor::new();
    e.insert_str("a");
    assert_eq!(e.handle_key("\n"), EditorEvent::Changed, "legacy ctrl+j");
    e.insert_str("b");
    assert_eq!(e.lines(), &["a", "b"]);
}

#[test]
fn backspace_joins_lines() {
    let mut e = Editor::new();
    e.insert_str("a");
    e.newline();
    e.insert_str("b");
    e.cursor_line_start();
    e.backspace();
    assert_eq!(e.lines(), &["ab"]);
}

#[test]
fn history_navigation() {
    let mut e = Editor::new();
    e.insert_str("first");
    e.submit();
    e.insert_str("second");
    e.submit();
    e.insert_str("draft");
    e.cursor_up();
    assert_eq!(e.text(), "second");
    e.cursor_up();
    assert_eq!(e.text(), "first");
    e.cursor_down();
    e.cursor_down();
    assert_eq!(e.text(), "draft");
}

#[test]
fn word_motion_and_deletion() {
    let mut e = Editor::new();
    e.insert_str("hello world");
    e.cursor_word_left();
    assert_eq!(e.cursor_col, 6);
    e.delete_word_backward();
    assert_eq!(e.text(), "world");
}

#[test]
fn undo_restores_the_previous_buffer() {
    let mut e = Editor::new();
    e.insert_str("hello");
    e.insert_str(" world");
    e.undo();
    assert_eq!(e.text(), "hello");
}

// pi's fish-style coalescing: a typed word is one undo unit, and a
// space starts the next one; an atomic insert is always its own unit.
#[test]
fn typing_coalesces_into_word_sized_undo_units() {
    let mut e = Editor::new();
    for c in "hello world".chars() {
        e.handle_key(&c.to_string());
    }
    assert_eq!(e.text(), "hello world");
    e.undo();
    assert_eq!(e.text(), "hello ");
    e.undo();
    assert_eq!(e.text(), "hello");
    e.undo();
    assert_eq!(e.text(), "");
}

// pi accumulates consecutive kills into one ring entry: a forward kill
// appends, a backward kill prepends, so one yank restores the run.
#[test]
fn consecutive_kills_accumulate_in_the_ring() {
    let mut e = Editor::new();
    e.insert_str("one two");
    e.cursor_line_end();
    e.delete_word_backward(); // kills "two "
    e.delete_word_backward(); // kills "one " and prepends
    assert_eq!(e.text(), "");
    assert_eq!(e.kill_ring.len(), 1, "one accumulated entry");
    assert_eq!(e.kill_ring[0], "one two");
    e.yank();
    assert_eq!(e.text(), "one two");
}

#[test]
fn an_atomic_insert_is_one_undo_unit() {
    let mut e = Editor::new();
    e.insert_str("pasted block");
    e.undo();
    assert_eq!(e.text(), "");
}

// Verifies: FR-UI-10 - a multi-line paste is one atomic segment.
#[test]
fn large_paste_becomes_a_marker() {
    let mut e = Editor::new();
    let big = (0..12)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    e.handle_key(&format!("\x1b[200~{big}\x1b[201~"));
    assert_eq!(e.text(), "[paste #1 +12 lines]");
}

// Verifies: FR-UI-10 - a paste marker expands to the pasted content when
// the buffer is submitted, so the model receives the real text.
#[test]
fn a_large_paste_expands_on_submit() {
    let mut e = Editor::new();
    let big = (0..12)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    e.handle_key(&format!("\x1b[200~{big}\x1b[201~"));
    assert_eq!(e.text(), "[paste #1 +12 lines]");
    assert_eq!(e.submit(), big);
    // The registry is per-buffer: the next paste starts at #1 again.
    e.handle_key("\x1b[200~one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\nten\neleven\ntwelve\x1b[201~");
    assert_eq!(e.text(), "[paste #1 +12 lines]");
}

// Verifies: FR-UI-10 - the real Enter path (`handle_key` -> `on_submit`)
// expands the marker too, not just the `submit` method.
#[test]
fn the_enter_key_path_expands_a_large_paste() {
    let mut e = Editor::new();
    let big = (0..12)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    e.handle_key(&format!("\x1b[200~{big}\x1b[201~"));
    match e.handle_key("\r") {
        EditorEvent::Submitted(text) => assert_eq!(text, big),
        other => panic!("expected Submitted, got {other:?}"),
    }
}

// Verifies: FR-UI-10 - the external-editor view expands without mutating
// the buffer (pi's `getExpandedText`).
#[test]
fn expanded_text_expands_without_clearing() {
    let mut e = Editor::new();
    let big = (0..12)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    e.handle_key(&format!("\x1b[200~{big}\x1b[201~"));
    assert_eq!(e.expanded_text(), big);
    assert_eq!(e.text(), "[paste #1 +12 lines]");
}

// Verifies: FR-UI-10 - a hand-typed marker shape with no entry stays literal.
#[test]
fn an_unknown_paste_marker_stays_literal() {
    let mut e = Editor::new();
    e.insert_str("see [paste #9 +3 lines] and [paste #abc]");
    assert_eq!(e.submit(), "see [paste #9 +3 lines] and [paste #abc]");
}

#[test]
fn a_long_single_line_paste_becomes_a_chars_marker() {
    let mut e = Editor::new();
    let big = "x".repeat(1200);
    e.handle_key(&format!("\x1b[200~{big}\x1b[201~"));
    assert_eq!(e.text(), "[paste #1 1200 chars]");
}

#[test]
fn paste_decodes_tmux_csi_u_ctrl_and_normalizes() {
    let mut e = Editor::new();
    // tmux CSI-u Ctrl+J inside the paste becomes a newline; a tab
    // expands to four spaces.
    e.handle_key("\x1b[200~a\x1b[106;5ub\tc\x1b[201~");
    assert_eq!(e.text(), "a\nb    c");
}

#[test]
fn cursor_marker_sits_at_the_cursor() {
    let mut e = Editor::new();
    e.insert_str("abc");
    e.cursor_left();
    let lines = e.render(40);
    let (stripped, pos) = extract_cursor_position(&lines);
    assert_eq!(stripped[0], "abc");
    assert_eq!(pos, Some((0, 2)));
}

#[test]
fn autocomplete_accepts_and_inserts() {
    use crate::widgets::autocomplete::{AutocompleteItem, SlashCommand};
    let commands = vec![SlashCommand {
        name: "model".into(),
        description: None,
        argument_hint: None,
        argument_completions: Some(Arc::new(|_p: &str| {
            vec![AutocompleteItem {
                value: "gpt-4o".into(),
                label: "gpt-4o".into(),
                description: None,
            }]
        })),
    }];
    let provider = Arc::new(
        crate::widgets::autocomplete::CombinedAutocompleteProvider::new(
            commands,
            std::env::temp_dir(),
        ),
    );
    let mut e = Editor::new();
    e.set_autocomplete(provider);
    e.insert_str("/model ");
    assert!(e.suggestions().is_some());
    e.accept_suggestion();
    assert_eq!(e.text(), "/model gpt-4o");
}

#[test]
fn space_is_inserted() {
    let mut e = Editor::new();
    e.handle_key("a");
    e.handle_key(" ");
    e.handle_key("b");
    assert_eq!(e.text(), "a b");
}

struct SingleCandidate;
impl AutocompleteProvider for SingleCandidate {
    fn get_suggestions(&self, _prefix: &str, _force: bool) -> Option<Suggestions> {
        Some(Suggestions {
            items: vec![crate::widgets::autocomplete::AutocompleteItem {
                value: "README.md".into(),
                label: "README.md".into(),
                description: None,
            }],
            prefix: "REA".into(),
        })
    }
}

// Verifies: FR-UI-9 (pi's editor.md §8 - force+Tab with one candidate
// applies it silently, so one Tab suffices).
#[test]
fn tab_applies_a_single_candidate_silently() {
    let mut e = Editor::new();
    e.set_autocomplete(Arc::new(SingleCandidate));
    e.insert_str("read REA");
    e.handle_key("\t");
    assert_eq!(e.text(), "read README.md");
    assert!(e.suggestions().is_none());
}
