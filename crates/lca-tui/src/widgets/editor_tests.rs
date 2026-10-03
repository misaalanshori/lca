//! The editor's tests, split out to keep `editor.rs` under the
//! 1,200-line ceiling (S3/S10).

use super::*;
use crate::engine::core::extract_cursor_position;
use proptest::prelude::*;

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

// R8(b) verification receipt. pi synthesizes Shift+Enter by polling the
// OS modifier state, because Apple Terminal and the Windows console send
// bare `\r` for it and the modifier is gone by the time the byte arrives.
// LCA does not carry a platform addon, so the dialect tables + a
// documented fallback have to carry the same weight:
//
//   console spellings  ->  `\x1b[13;2u`, `ESC CR`, `\x1b[13;2~` all mean
//                          shift+enter (asserted in keys.rs / regression 32);
//   the one case that really cannot be distinguished (bare `\r`, as the
//   Windows console sends) -> `\r` keeps submitting and Ctrl+J / `\`+Enter
//                          are the newline spellings, both bound and
//                          both asserted here.
//
// That is the complete matrix: there is no spelling a legacy console can
// send that has no answer, so the native modifier poll buys nothing and
// is deliberately not ported.
#[test]
fn the_console_without_a_distinct_shift_enter_uses_the_ctrl_j_fallback() {
    // Bare `\r` — the Windows console's only Enter spelling — submits.
    let mut e = Editor::new();
    e.insert_str("send me");
    assert_eq!(
        e.handle_key("\r"),
        EditorEvent::Submitted("send me".to_string()),
        "the console's Enter still submits"
    );

    // …and the newline it cannot express arrives through the bound
    // fallback (`tui.input.newLine` = [shift+enter, ctrl+j]).
    let mut e = Editor::new();
    e.insert_str("line one");
    assert_eq!(e.handle_key("\n"), EditorEvent::Changed, "ctrl+j = newline");
    e.insert_str("line two");
    assert_eq!(e.lines(), &["line one", "line two"]);
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
    // V3 (pi's boundary): a top-line Up with the caret off column 0 goes
    // to the start of the line first, and only the next Up crosses into
    // history.
    e.cursor_up();
    assert_eq!(e.cursor_col, 0, "start of line first");
    assert_eq!(e.text(), "draft");
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

// Verifies: V2 (issues #10/#11) - the caret is painted by the component
// (pi's model): reverse-video grapheme under the cursor, marker on the
// caret cell, buffer text unchanged.
#[test]
fn the_caret_is_painted_and_the_marker_sits_on_it() {
    let mut e = Editor::new();
    e.insert_str("abc");
    e.cursor_left();
    let lines = e.render(40);
    let (clean, pos) = extract_cursor_position(&lines);
    assert_eq!(pos, Some((0, 2, true)), "the marker sits on the caret cell");
    assert!(
        clean[0].contains("\x1b[7mc\x1b[0m"),
        "the grapheme under the caret is reverse video: {:?}",
        clean[0]
    );
    let plain = crate::engine::text::strip_terminal_sequences(&clean[0]);
    assert_eq!(plain, "abc", "the buffer text is unchanged");
}

// Verifies: V2 - the keystroke matrix from the brief: every frame paints
// the caret and reports the column the keystrokes imply (`a`, space,
// space, Home, End, a typed letter).
#[test]
fn the_keystroke_matrix_paints_the_caret_every_frame() {
    let mut e = Editor::new();
    let mut cols: Vec<usize> = Vec::new();
    let snap = |e: &Editor, cols: &mut Vec<usize>| {
        let lines = e.render(40);
        let (clean, pos) = extract_cursor_position(&lines);
        let (row, col, _painted) = pos.expect("every frame carries the caret marker");
        assert_eq!(row, 0, "one line under test");
        assert!(
            clean[0].contains("\x1b[7m"),
            "the caret is painted into the row: {:?}",
            clean[0]
        );
        cols.push(col as usize);
    };
    snap(&e, &mut cols); // empty buffer
    e.insert_str("a");
    snap(&e, &mut cols);
    e.insert_str(" ");
    snap(&e, &mut cols);
    e.insert_str(" ");
    snap(&e, &mut cols);
    e.cursor_line_start();
    snap(&e, &mut cols);
    e.cursor_line_end();
    snap(&e, &mut cols);
    e.insert_str("z");
    snap(&e, &mut cols);
    assert_eq!(
        cols,
        vec![0, 1, 2, 3, 0, 3, 4],
        "columns after each key: {cols:?}"
    );
}

// Verifies: V2 - trailing spaces: the caret at the end of `a b  ` sits on
// a painted cell, and moving onto a typed space paints that space (the
// owner's "invisible spaces" complaint).
#[test]
fn the_caret_on_trailing_spaces_is_visible() {
    let mut e = Editor::new();
    e.insert_str("a b  ");
    let lines = e.render(40);
    let (clean, pos) = extract_cursor_position(&lines);
    assert_eq!(
        pos,
        Some((0, 5, true)),
        "end of line after the trailing spaces"
    );
    assert!(
        clean[0].contains("\x1b[7m \x1b[0m"),
        "an end-of-line caret is a reverse-video space: {:?}",
        clean[0]
    );
    let plain = crate::engine::text::strip_terminal_sequences(&clean[0]);
    assert!(plain.starts_with("a b  "), "the spaces stay: {plain:?}");

    e.cursor_left(); // onto the second typed space
    let lines = e.render(40);
    let (clean, pos) = extract_cursor_position(&lines);
    assert_eq!(pos, Some((0, 4, true)), "on the typed space");
    assert!(
        clean[0].contains("\x1b[7m \x1b[0m"),
        "the space under the caret is painted: {:?}",
        clean[0]
    );
}

// Verifies: V2 - a wide or clustered grapheme is painted whole (never in
// half), and deletion removes the whole cluster (pi's snap).
#[test]
fn the_caret_paints_whole_graphemes_and_deletes_them() {
    let mut e = Editor::new();
    e.insert_str("a👍b");
    e.cursor_left(); // on 'b'
    e.cursor_left(); // on the emoji
    let lines = e.render(40);
    let (clean, pos) = extract_cursor_position(&lines);
    assert_eq!(pos, Some((0, 1, true)), "on the wide grapheme");
    assert!(
        clean[0].contains("\x1b[7m👍\x1b[0m"),
        "the whole grapheme is reverse video: {:?}",
        clean[0]
    );
    // Backspace deletes the cluster *before* the cursor: step past the
    // emoji first so the deletion targets it.
    e.cursor_right();
    e.backspace();
    assert_eq!(e.text(), "ab", "the grapheme is deleted whole");

    // A ZWJ cluster is one movement and one deletion.
    let mut e = Editor::new();
    e.insert_str("👨‍👩‍👧");
    e.cursor_left();
    let (clean, pos) = extract_cursor_position(&e.render(40));
    assert_eq!(pos, Some((0, 0, true)), "one step back leaves the cluster");
    assert!(
        clean[0].contains("\x1b[7m👨‍👩‍👧\x1b[0m"),
        "the cluster is painted whole: {:?}",
        clean[0]
    );
    e.delete_forward();
    assert_eq!(e.text(), "", "and deleted whole");
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

// Verifies: FR-UI-24 (R5) - a trailing space moves the caret: the marker's
// column advances on the space keystroke itself, not on the next letter.
#[test]
fn a_trailing_space_advances_the_cursor_column() {
    let mut e = Editor::new();
    for ch in "a b".chars() {
        e.handle_key(&ch.to_string());
    }
    let (_, before) = extract_cursor_position(&e.render(80));
    assert_eq!(before.map(|(_, col, _)| col), Some(3), "after the b");

    e.handle_key(" ");
    let lines = e.render(80);
    let (_, after) = extract_cursor_position(&lines);
    assert_eq!(
        after.map(|(_, col, _)| col),
        Some(4),
        "the space moved the caret: {lines:?}"
    );
    // And the trailing space is really in the buffer, not trimmed away.
    assert_eq!(e.lines(), &["a b "], "the space is in the line");
}

// Verifies: V2 + FR-UI-5 - the plain theme keeps its no-escape contract:
// the caret is not painted, the marker still tells the engine where to
// put the (visible) hardware cursor.
#[test]
fn the_plain_theme_does_not_paint_the_caret() {
    let mut e = Editor::new();
    e.set_paint_caret(false);
    e.insert_str("abc");
    e.cursor_left();
    let lines = e.render(40);
    let (clean, pos) = extract_cursor_position(&lines);
    assert_eq!(pos, Some((0, 2, false)), "positioned, not painted");
    assert!(
        !clean[0].contains('\x1b'),
        "no escape at all in plain mode: {:?}",
        clean[0]
    );
    assert_eq!(
        crate::engine::text::strip_terminal_sequences(&clean[0]),
        "abc"
    );
}

// Verifies: V3 (issue #12) - pi's boundary matrix: Up/Down move inside the
// buffer first and cross into history only at the first/last line under
// pi's conditions (empty / already browsing / column 0 for Up; browsing
// for Down; end-of-line otherwise). A two-line prompt, Up/Up/Up, then
// Down x4, asserting buffer vs history state at every step.
#[test]
fn up_and_down_cross_into_history_only_at_the_boundaries() {
    let mut e = Editor::new();
    e.set_text("first");
    e.submit();
    e.set_text("second");
    e.submit();

    e.set_text("line1\nline2"); // cursor at the end of line1
    e.cursor_up();
    assert_eq!(e.cursor_line, 0, "buffer first: Up lands on line 0");
    assert!(e.history_index.is_none(), "no history yet");

    e.cursor_up();
    assert_eq!(
        e.cursor_col, 0,
        "top line with the caret off the start: start of line, not history"
    );
    assert!(e.history_index.is_none());

    e.cursor_up();
    assert_eq!(e.history_index, Some(1), "column 0 crosses into history");
    assert_eq!(e.text(), "second", "newest entry first");

    e.cursor_down();
    assert_eq!(e.text(), "line1\nline2", "down exits back to the draft");
    assert!(e.history_index.is_none(), "browsing closed");

    e.cursor_down();
    assert_eq!(
        (e.cursor_line, e.cursor_col),
        (1, 5),
        "last line, no history: end of line"
    );
    e.cursor_down();
    assert_eq!((e.cursor_line, e.cursor_col), (1, 5), "and it stays there");
}

// =============================================================================
// gh #27 / #28: the editor's visual rows - spaces that hold their cells, and
// arrows that walk the rows the renderer actually drew. Both issues live in
// the same geometry, which is why they share a cycle: navigation must agree
// with the rows `render` produces at the current width.
// =============================================================================

/// The buffer's rows as the terminal shows them: no caret marker, no SGR.
fn plain_rows(rows: &[String]) -> Vec<String> {
    rows.iter()
        .map(|row| crate::engine::text::strip_terminal_sequences(&row.replace(CURSOR_MARKER, "")))
        .collect()
}

/// The caret's row and column in `render(width)`'s output.
fn caret(e: &Editor, width: u16) -> (usize, u16) {
    let (_, pos) = extract_cursor_position(&e.render(width));
    let (row, col, _) = pos.expect("every frame carries the caret marker");
    (row as usize, col)
}

// Verifies: gh #27 (b) - a space is a cell. The word wrap used to trim the
// row it broke on, so the spaces at a wrap boundary vanished from the screen
// (and the caret's char offsets drifted with them). pi's editor keeps them:
// the break sits after the whitespace, and the continuation starts at the
// next word (`wordWrapLine`, `editor.md` §3).
#[test]
fn spaces_hold_their_cells_at_a_wrap_boundary() {
    let mut e = Editor::new();
    let text = "aaaa bbbb cccc dddd eeee ffff";
    e.set_text(text);
    // Off the end of the line, so the caret paints an existing character
    // rather than adding its own end-of-line cell to the count.
    e.cursor_line_start();
    let rows = plain_rows(&e.render(10));
    assert!(rows.len() >= 3, "the line wraps at width 10: {rows:?}");

    // Nothing the user typed is dropped: every space in the buffer is on
    // screen in some row.
    let on_screen: usize = rows
        .iter()
        .map(|row| row.chars().filter(|c| *c == ' ').count())
        .sum();
    let typed = text.chars().filter(|c| *c == ' ').count();
    assert_eq!(on_screen, typed, "no space was swallowed: {rows:?}");

    // The row the wrap broke on carries the break's space and still fills
    // the width it was given - the space occupies its cell.
    assert!(
        rows[0].ends_with(' '),
        "the wrap keeps the break's space at the row's end: {rows:?}"
    );
    assert_eq!(
        crate::engine::text::visible_width(&rows[0]),
        10,
        "the row's width includes that space: {rows:?}"
    );
}

// Verifies: gh #28 (the primary row) - Up/Down move between VISUAL rows of
// one wrapped logical line. The assertion is made against the rows `render`
// draws, so it fails if navigation and rendering ever disagree about the
// geometry (the cycle's interaction constraint).
#[test]
fn up_and_down_walk_the_visual_rows_the_renderer_drew() {
    let mut e = Editor::new();
    e.set_text("aaaa bbbb cccc dddd eeee ffff gggg");
    let total = e.render(10).len();
    assert!(total >= 3, "the line wraps into several rows: {total}");

    let (row, _) = caret(&e, 10);
    assert_eq!(row, total - 1, "the caret starts on the last rendered row");

    for want in (0..total - 1).rev() {
        e.cursor_up();
        assert_eq!(caret(&e, 10).0, want, "Up walks up the rendered rows");
    }
    for want in 1..total {
        e.cursor_down();
        assert_eq!(caret(&e, 10).0, want, "Down walks down the rendered rows");
    }
}

// Verifies: gh #28 - a vertical move keeps the VISUAL column, across several
// hops in each direction (pi's sticky column, `editor.md` §4).
#[test]
fn vertical_moves_keep_the_visual_column() {
    let mut e = Editor::new();
    e.set_text("aaaa bbbb cccc dddd eeee ffff gggg");
    e.render(10);
    e.cursor_line_start();
    e.cursor_right();
    e.cursor_right();
    let (_, column) = caret(&e, 10);
    assert_eq!(column, 2, "two cells in from the row's start");

    for hop in 1..=3 {
        e.cursor_down();
        assert_eq!(
            caret(&e, 10),
            (hop, column),
            "the visual column sticks going down"
        );
    }
    for hop in (0..=2).rev() {
        e.cursor_up();
        assert_eq!(
            caret(&e, 10),
            (hop, column),
            "the visual column sticks going up"
        );
    }
}

// Verifies: gh #28 - crossing a wrap's end moves to the NEIGHBOURING
// logical line's last/first visual row, not to Home or End.
#[test]
fn crossing_the_wrap_enters_the_neighbouring_logical_line() {
    let mut e = Editor::new();
    e.set_text("aaaa bbbb cccc dddd\ntail");
    let total = e.render(10).len();
    let line_one = total - 1; // `tail` is short: one visual row
    assert!(line_one >= 2, "line 0 wraps: {total}");

    // The caret starts at the end of the buffer (line 1).
    assert_eq!(caret(&e, 10).0, line_one);

    // Up from line 1's only row lands on line 0's LAST visual row.
    e.cursor_up();
    assert_eq!(e.cursor_line, 0, "the move crossed into line 0");
    assert_eq!(caret(&e, 10).0, line_one - 1, "on line 0's last row");

    // And back down to line 1.
    e.cursor_down();
    assert_eq!(e.cursor_line, 1, "back into line 1");
    assert_eq!(caret(&e, 10).0, line_one);

    // Up again: line 0's FIRST visual row, keeping the sticky column the
    // move started from (it began on line 1, column 4).
    e.cursor_up();
    e.cursor_up();
    assert_eq!(e.cursor_line, 0, "the first row of line 0");
    assert_eq!(caret(&e, 10).0, 0, "and the caret is drawn on row 0");
    assert_eq!(e.cursor_col, 4, "the visual column carried up with it");

    // And once there, Up keeps pi's edge behaviour: start of the line, no
    // history jump.
    e.cursor_up();
    assert_eq!(
        (e.cursor_line, e.cursor_col),
        (0, 0),
        "Up past the top row: start of the line"
    );
    assert!(
        e.history_index.is_none(),
        "Up past the top row does not jump into history"
    );

    // Down from line 0's LAST visual row enters line 1 (already asserted
    // above); Down past the buffer's last row ends the line, pi's rule.
    e.cursor_line_end();
    e.cursor_down();
    assert_eq!(
        e.cursor_col,
        e.current().chars().count(),
        "Down past the last row: end of the line"
    );
}

// Verifies: gh #27 (a) - `Home` and `End` are logical-line operations and
// are unchanged by visual-row navigation.
#[test]
fn home_and_end_still_move_within_the_logical_line() {
    let mut e = Editor::new();
    e.set_text("aaaa bbbb cccc dddd eeee ffff");
    e.render(10);
    e.cursor_line_end();
    assert_eq!(caret(&e, 10).0, e.render(10).len() - 1, "End: last row");
    e.cursor_line_start();
    assert_eq!(caret(&e, 10).0, 0, "Home: first row");
    assert_eq!(e.cursor_col, 0, "Home is still column 0 of the line");
}

// Verifies: gh #27/#28's interaction constraint - the row navigation
// resolves a position to is the row the renderer paints the caret on, for
// arbitrary text and width. Either half passing alone is not enough: a
// geometry that drifts between them is the bug both issues report.
proptest! {
    #![proptest_config(ProptestConfig::with_cases(50))]

    #[test]
    fn navigation_walks_exactly_the_rows_the_renderer_drew(
        text in "[a-z ]{0,80}",
        width in 8u16..60,
        col in 0usize..120,
    ) {
        let mut e = Editor::new();
        e.set_text(&text);
        e.cursor_line_start();
        for _ in 0..col {
            e.cursor_right();
        }
        let rendered = e.render(width);
        let drawn = rendered
            .iter()
            .position(|row| row.contains(CURSOR_MARKER))
            .expect("the caret is always painted");
        let walked = crate::widgets::editor_rows::find_visual_row(
            &crate::widgets::editor_rows::visual_rows(e.lines(), width as usize),
            0,
            e.cursor_col,
        );
        prop_assert_eq!(
            drawn,
            walked,
            "render drew row {}, navigation walked row {} (width {}, col {})",
            drawn,
            walked,
            width,
            e.cursor_col
        );
    }
}
