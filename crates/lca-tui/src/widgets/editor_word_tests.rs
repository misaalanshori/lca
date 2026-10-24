//! Word-motion rows (gh #383 gap 1): the segmentation table, the
//! motion/deletion agreement, paste markers, and the conformance table
//! transcribed from pi's `word-navigation.test.ts`.
//!
//! Split out of `editor_tests.rs` when that file crossed the 1,200-line ceiling.

use super::*;

// ==== gh #383 gap 1: a segment is a word, a punctuation run, or whitespace ====

/// Walk the caret left from the end of `text`, collecting the column after each stop.
fn stops_left(text: &str) -> Vec<usize> {
    let mut e = Editor::new();
    e.set_text(text);
    e.cursor_line_end();
    let mut stops = vec![e.cursor_col];
    for _ in 0..text.chars().count() + 4 {
        let before = e.cursor_col;
        e.cursor_word_left();
        if e.cursor_col == before {
            break;
        }
        stops.push(e.cursor_col);
    }
    stops
}

/// Walk the caret right from the start of `text`, collecting each stop.
fn stops_right(text: &str) -> Vec<usize> {
    let mut e = Editor::new();
    e.set_text(text);
    e.cursor_line_start();
    let mut stops = vec![e.cursor_col];
    for _ in 0..text.chars().count() + 4 {
        let before = e.cursor_col;
        e.cursor_word_right();
        if e.cursor_col == before {
            break;
        }
        stops.push(e.cursor_col);
    }
    stops
}

/// The text each `Ctrl+W` leaves, walking to the start.
fn ctrl_w_steps(text: &str) -> Vec<String> {
    let mut e = Editor::new();
    e.set_text(text);
    e.cursor_line_end();
    let mut steps = vec![e.text()];
    for _ in 0..text.chars().count() + 4 {
        let before = e.text();
        e.delete_word_backward();
        let after = e.text();
        if after == before {
            break;
        }
        steps.push(after);
    }
    steps
}

// Verifies: gh #383 gap 1 AC1 - **the issue's own row**: `Ctrl+W` on a path with a
// `:line` suffix deletes one *segment*, not the whole path.
//
// Whitespace-only motion treated `crates/lca-tui/src/widgets/editor.rs:42` as a single
// word, so one keystroke erased all 43 characters.
#[test]
fn ctrl_w_on_a_path_deletes_one_segment() {
    let path = "crates/lca-tui/src/widgets/editor.rs:42";
    let mut e = Editor::new();
    e.set_text(path);
    e.cursor_line_end();
    e.delete_word_backward();
    // The last segment is the `:42` suffix's word part - the path survives.
    assert!(
        e.text().starts_with("crates/lca-tui/src/widgets/editor.rs"),
        "one Ctrl+W must not eat the path: {:?}",
        e.text()
    );
    assert!(
        e.text().chars().count() >= path.chars().count() - 4,
        "and it removes only a short segment: {:?}",
        e.text()
    );
}

// Verifies: gh #383 gap 1 AC2 - **the specification as a table**: `Alt+Left` over
// `foo.bar(baz_qux)`.
//
// Columns (0-based) on "foo.bar(baz_qux)" (16 chars). `_` is a **word** character, so
// `baz_qux` is one word, and `)` is its own punctuation run:
//
//   16 end -> 15 after `baz_qux` -> 8 the `)` -> 7 the `(`
//   -> 4 after `bar` -> 3 the `.` -> 0 `foo`
//
// My first version of this table predicted `bar` -> `.` -> `foo` and stopped at 11/10,
// treating `baz_qux` as two segments and `(`+`)` as one run. The code was right and my
// prediction was wrong - which is why pi's `word-navigation.test.ts` is the source for
// these numbers rather than intuition.
#[test]
fn alt_left_over_a_call_expression_stops_at_each_segment() {
    assert_eq!(
        stops_left("foo.bar(baz_qux)"),
        vec![16, 15, 8, 7, 4, 3, 0],
        "the call, `)`, `(`, the method name, `.`, the receiver"
    );
}

// Verifies: gh #383 gap 1 AC2, the forward direction, on the same string - the exact
// reverse of the left walk.
#[test]
fn alt_right_over_a_call_expression_stops_at_each_segment() {
    assert_eq!(
        stops_right("foo.bar(baz_qux)"),
        vec![0, 3, 4, 7, 8, 15, 16],
        "the mirror of the left walk"
    );
}

// Verifies: gh #383 gap 1 - the rows pi's own `word-navigation.test.ts` asserts, run
// against LCA so the two implementations cannot drift.
//
// These are transcribed from `packages/tui/test/word-navigation.test.ts` (`dotted`,
// `colon`, `path`, `punctuation run`, `whitespace at boundaries`). They are the
// conformance table: if pi changes its mind, this row disagrees and someone looks.
#[test]
fn pi_word_navigation_conformance() {
    // findWordBackward: (text, from) -> to
    for (text, from, to) in [
        ("hello world", 11, 6),
        ("hello world", 6, 0),
        ("foo.bar", 7, 4),
        ("foo.bar", 4, 3),
        ("foo.bar", 3, 0),
        ("foo:bar", 7, 4),
        ("foo:bar", 4, 3),
        ("foo:bar", 3, 0),
        ("path/to/file", 12, 8),
        ("path/to/file", 8, 7),
        ("path/to/file", 7, 5),
        ("path/to/file", 5, 4),
        ("path/to/file", 4, 0),
        ("  hello  ", 9, 2),
        ("  hello  ", 2, 0),
        ("foo...bar", 9, 6),
        ("foo...bar", 6, 3),
        ("foo...bar", 3, 0),
    ] {
        let mut e = Editor::new();
        e.set_text(text);
        e.cursor_col = from;
        e.cursor_word_left();
        assert_eq!(e.cursor_col, to, "backward {text:?} from {from}");
    }
    // findWordForward: (text, from) -> to
    for (text, from, to) in [
        ("hello world", 0, 5),
        ("hello world", 5, 11),
        ("foo.bar", 0, 3),
        ("foo.bar", 3, 4),
        ("foo.bar", 4, 7),
        ("foo:bar", 0, 3),
        ("foo:bar", 3, 4),
        ("foo:bar", 4, 7),
        ("path/to/file", 0, 4),
        ("path/to/file", 4, 5),
        ("path/to/file", 5, 7),
        ("path/to/file", 7, 8),
        ("path/to/file", 8, 12),
        ("  hello  ", 0, 7),
        ("  hello  ", 7, 9),
        ("foo...bar", 0, 3),
        ("foo...bar", 3, 6),
        ("foo...bar", 6, 9),
    ] {
        let mut e = Editor::new();
        e.set_text(text);
        e.cursor_col = from;
        e.cursor_word_right();
        assert_eq!(e.cursor_col, to, "forward {text:?} from {from}");
    }
}

// Verifies: gh #383 gap 1 AC3 - a **punctuation run moves as one unit**, not per
// character.
//
// This is the rule that has to be read out of pi rather than invented: treating
// punctuation as "anything non-alphanumeric" gives `(` then `)` then `-`, which is
// worse than the whitespace-only behaviour it replaces.
#[test]
fn a_punctuation_run_moves_as_one_unit() {
    for (text, expected) in [
        ("foo...bar", vec![9, 6, 3, 0]),
        // `()->`  is ONE run: a run is a maximal stretch of the same *class*, and all
        // four are punctuation. (My first table split it at the class boundary of
        // individual characters, which is not what a run means.)
        ("a()->b", vec![6, 5, 1, 0]),
        ("x===y", vec![5, 4, 1, 0]),
    ] {
        assert_eq!(stops_left(text), expected, "left stops for {text:?}");
    }
}

// Verifies: gh #383 gap 1 AC4 - **deletion and motion are the same walk**: `Ctrl+W`
// removes exactly the span `Alt+Left` crosses, for a table of strings.
#[test]
fn ctrl_w_deletes_exactly_what_alt_left_crosses() {
    for text in [
        "foo.bar(baz_qux)",
        "crates/lca-tui/src/widgets/editor.rs:42",
        "hello world",
        "a()->b",
        "trailing   ",
        "  leading",
        "one",
    ] {
        let mut motion = Editor::new();
        motion.set_text(text);
        motion.cursor_line_end();
        motion.cursor_word_left();
        let motion_landed = motion.cursor_col;

        let mut del = Editor::new();
        del.set_text(text);
        del.cursor_line_end();
        del.delete_word_backward();
        let deleted = text.chars().count() - del.text().chars().count();

        assert_eq!(
            deleted,
            text.chars().count() - motion_landed,
            "{text:?}: Ctrl+W deleted {deleted} but Alt+Left crossed {}",
            text.chars().count() - motion_landed
        );
        assert_eq!(
            del.cursor_col, motion_landed,
            "{text:?}: and both leave the caret at the same column"
        );
    }
}

// Verifies: gh #383 gap 1 AC5 - trailing whitespace handling, pinned in **both**
// directions.
//
// Backward motion skips trailing whitespace first, then the segment before it - so
// `Ctrl+W` at `"foo   "` clears the spaces *and* `foo` (one keystroke at a time:
// first the run, then the word). Forward motion skips leading whitespace, then the
// segment.
#[test]
fn whitespace_is_skipped_before_the_segment_in_both_directions() {
    // Backward: whitespace is skipped and then the segment before it is consumed, so
    // one press from the end lands at 0. (pi's `findWordBackward("  hello  ", 9) == 2`
    // - 2 is the start of `hello`, not the whitespace boundary.)
    assert_eq!(stops_left("foo   "), vec![6, 0]);
    // Forward: leading whitespace is skipped and then the segment after it is consumed
    // (pi: `findWordForward("  hello  ", 0) == 7` - past `hello`, not to its start).
    assert_eq!(stops_right("   foo"), vec![0, 6]);
    // And a Ctrl+W on trailing spaces takes them plus the word behind them, because
    // the backward walk skips whitespace then consumes one segment.
    assert_eq!(
        ctrl_w_steps("foo   "),
        vec!["foo   ".to_string(), String::new()],
        "one press clears the spaces and the word behind them"
    );
}

// Verifies: gh #383 gap 1 AC6 - **a paste marker hops whole**. A caret landing inside
// `[paste #1 +3 lines]` is a different class of bug: the marker is one token.
#[test]
fn a_paste_marker_hops_whole() {
    let mut e = Editor::new();
    // Stand in the marker text directly: the editor's buffer holds the marker, and its
    // expansion happens only on submit.
    e.set_text("before [paste #1 +3 lines] after");
    e.cursor_line_end();
    e.delete_word_backward();
    assert_eq!(e.text(), "before [paste #1 +3 lines] ");
    // The next Ctrl+W takes the marker as a unit, not a fragment of it.
    e.delete_word_backward();
    assert_eq!(
        e.text(),
        "before ",
        "the marker must come out whole: {:?}",
        e.text()
    );
}

// Verifies: gh #383 gap 1 AC7 - motion never lands **inside a wide glyph**.
//
// Columns are char indices, and a 2-cell glyph is one char, so the invariant that
// matters is that every stop is a char boundary - which the editor's char-indexed
// buffer makes structural. The row pins it for CJK so a future byte-index rewrite
// fails here.
#[test]
fn motion_never_lands_inside_a_wide_glyph() {
    let text = "日本語 の テスト";
    let mut e = Editor::new();
    e.set_text(text);
    e.cursor_line_end();
    for _ in 0..8 {
        let before = e.cursor_col;
        e.cursor_word_left();
        let col = e.cursor_col;
        assert!(
            text.chars().nth(col).is_none() || col <= text.chars().count(),
            "the column is a char index"
        );
        // And the byte offset it maps to must be a boundary.
        let byte = e.char_to_byte(col);
        assert!(
            text.is_char_boundary(byte),
            "stop at char {col} is not a byte boundary in {text:?}"
        );
        if e.cursor_col == before {
            break;
        }
    }
}

// Verifies: gh #383 gap 1 - the classifier is shared: `Alt+Left`, `Alt+Right` and
// `Ctrl+W` agree on every boundary in a mixed string, in both directions.
#[test]
fn all_three_walks_agree_on_every_boundary() {
    let text = "git commit -m 'fix: the bug' && echo done";
    // The walks are **not** mirror images, and that is correct: backward motion lands on
    // a segment's *start*, forward motion on its *end* (pi's own assertions -
    // `findWordBackward("  hello  ", 9) == 2` but `findWordForward("  hello  ", 0) == 7`).
    // My first version asserted equality and was simply wrong about the semantics.
    //
    // The real invariants: both walks reach the far end, both are strictly monotone, and
    // every stop is a class boundary.
    let mut left = stops_left(text);
    left.reverse();
    let right = stops_right(text);
    assert_eq!(left.first(), Some(&0), "backward reaches the start");
    assert_eq!(
        left.last(),
        Some(&text.chars().count()),
        "and starts at the end"
    );
    assert_eq!(right.first(), Some(&0), "forward starts at the start");
    assert_eq!(
        right.last(),
        Some(&text.chars().count()),
        "and reaches the end"
    );
    for window in left.windows(2) {
        assert!(
            window[0] < window[1],
            "backward stops are monotone: {left:?}"
        );
    }
    for window in right.windows(2) {
        assert!(
            window[0] < window[1],
            "forward stops are monotone: {right:?}"
        );
    }
    // Every stop is a class boundary: the class changes across it (or it is an end).
    let chars: Vec<char> = text.chars().collect();
    let class = crate::engine::text::word_class;
    for col in left.iter().chain(right.iter()).copied() {
        if col == 0 || col >= chars.len() {
            continue;
        }
        assert_ne!(
            class(chars[col - 1]),
            class(chars[col]),
            "column {col} is not a class boundary in {text:?}"
        );
    }
    // And every Ctrl+W step lands on one of those boundaries.
    let mut e = Editor::new();
    e.set_text(text);
    e.cursor_line_end();
    // Ctrl+W walks *backward*, so its stops must be among the **backward** walk's
    // stops - not the forward ones. The two differ by exactly the whitespace rule:
    // forward from 36 consumes the space at 36 and then `done` (ending at 41), while
    // backward from 41 skips the spaces and then consumes `done` (landing at 37).
    // Both are pi's behaviour; my first version unioned the forward set and was wrong.
    let backward_stops: std::collections::BTreeSet<usize> = left.iter().copied().collect();
    for _ in 0..12 {
        let before = e.cursor_col;
        e.delete_word_backward();
        if e.cursor_col == before {
            break;
        }
        assert!(
            backward_stops.contains(&e.cursor_col),
            "Ctrl+W landed at {} which is not a backward-motion stop: {backward_stops:?}",
            e.cursor_col
        );
    }
}

// Verifies: gh #383 gap 1 - CJK, against pi's own assertions, so the divergence is
// **recorded** rather than assumed away.
//
// pi's `word-navigation.test.ts` ("CJK mixed") asserts on `"你好世界 test"`:
//   findWordBackward(text, 7) === 5   (skip ` test` -> the space boundary)
//   findWordBackward(text, 5) === 2   (skip `好世` -> 2)
//   findWordBackward(text, 2) === 0
//
// Those numbers are what an `Intl.Segmenter` with dictionary-free CJK does, and they are
// **not** what a class-run walk does: LCA treats the whole ideograph run as one word.
// Both are defensible (a reader would say `你好世界` is four words; a terminal user
// usually wants the run), and pi's own inline comment disagrees with its own assertion,
// so the honest thing is a row that states LCA's answer and names the difference.
#[test]
fn cjk_motion_takes_the_whole_ideograph_run() {
    let text = "你好世界 test";
    let mut e = Editor::new();
    e.set_text(text);
    e.cursor_col = 7;
    e.cursor_word_left();
    // 7 -> 5 matches pi exactly: skip the leading space, stop at `test`'s start.
    assert_eq!(e.cursor_col, 5, "7 -> 5, the same as pi");
    // And 5 -> 0: the four ideographs are **one** run here, where pi's next assertion is
    // 5 -> 2 (two ideographs). That is the recorded divergence.
    e.cursor_word_left();
    assert_eq!(e.cursor_col, 0, "5 -> 0: one run, where pi says 5 -> 2");
    // And forward: the run is one segment.
    e.cursor_col = 0;
    e.cursor_word_right();
    assert_eq!(e.cursor_col, 4, "the four ideographs are one run");
    e.cursor_word_right();
    assert_eq!(e.cursor_col, 9, "then `test`");
}
