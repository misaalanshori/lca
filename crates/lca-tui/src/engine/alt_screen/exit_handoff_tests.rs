//! Gh #374 gap 1: the alt-screen exit handoff.
//!
//! Two losses, measured in a real pane before the fix (100x24, eight shell cards,
//! `/exit` while fullscreen): the dump was 2426 bytes after `?1049l`, `MARKER-1`
//! (the oldest card) was absent, `MARKER-8` (the newest) was present, and the SGR
//! count in that payload was **0**.
//!
//! Split out of `alt_screen.rs` for the workspace's 1,200-line ceiling (gate 11).

use super::*;
use crate::engine::terminal::FakeTerminal;

/// A terminal big enough that the frame is obviously smaller than the document.
fn exit_term() -> FakeTerminal {
    FakeTerminal::new(40, 6)
}

fn rows(lines: &[&str]) -> Vec<String> {
    lines.iter().map(|s| s.to_string()).collect()
}

/// Render one frame with this renderer, so `previous` holds the visible window.
fn frame(r: &mut AltScreenRenderer, term: &mut FakeTerminal, lines: Vec<String>, height: u16) {
    r.render_lines(term, lines, 40, height);
    let _ = term.take_output();
}

/// The bytes the exit wrote after it left the alt screen.
fn dumped(out: &str) -> String {
    out.split_once("\x1b[?1049l")
        .map(|(_, rest)| rest.to_string())
        .unwrap_or_else(|| panic!("the exit leaves the alt screen: {out:?}"))
}

// Verifies: gh #374 gap 1 (loss (a), the bigger one) - **every** row of the
// document leaves the alt screen, not just the visible window. This is the row
// that was 200-into-24: it asserts the count and a row from far above the fold,
// because a count alone could be satisfied by dumping the window twice.
#[test]
fn the_exit_dumps_the_whole_document_not_the_visible_window() {
    let mut r = AltScreenRenderer::new();
    let mut term = exit_term();
    // A frame of 6 rows, a document of 20: `FIRST-ROW` is 14 rows above what
    // `previous` can hold.
    let document = rows(&[
        "FIRST-ROW",
        "second",
        "third",
        "fourth",
        "fifth",
        "sixth",
        "seventh",
        "eighth",
        "ninth",
        "tenth",
        "eleventh",
        "twelfth",
        "thirteenth",
        "fourteenth",
        "fifteenth",
        "sixteenth",
        "seventeenth",
        "eighteenth",
        "nineteenth",
        "LAST-ROW",
    ]);
    r.set_document(document);
    // The visible frame is the last six rows of it, as the host would cut it.
    frame(
        &mut r,
        &mut term,
        rows(&[
            "fifteenth",
            "sixteenth",
            "seventeenth",
            "eighteenth",
            "nineteenth",
            "LAST-ROW",
        ]),
        6,
    );

    r.leave(&mut term, false);
    let payload = dumped(&term.take_output());

    assert!(
        payload.contains("FIRST-ROW"),
        "a row 14 above the fold is in the scrollback - it was the whole bug: {payload:?}"
    );
    assert!(
        payload.contains("LAST-ROW"),
        "and so is the last row: {payload:?}"
    );
    assert_eq!(
        payload.matches("\x1b[2K").count(),
        20,
        "every row painted exactly once - the whole document and nothing twice: {payload:?}"
    );
}

// Verifies: gh #374 gap 1 (loss (b)) - the dumped rows keep their SGR. The
// measured defect had **zero** SGR sequences after `?1049l`; a row that only
// checked the text would have passed through both the bug and the fix.
#[test]
fn the_exit_dump_keeps_every_colour() {
    let mut r = AltScreenRenderer::new();
    let mut term = exit_term();
    let document = rows(&[
        "\x1b[38;2;236;205;0myellow text\x1b[39m",
        "\x1b[48;2;45;40;56m \x1b[38;2;149;117;205m[session]\x1b[39m \x1b[49m",
        "\x1b[1;31mred\x1b[0m",
    ]);
    r.set_document(document.clone());
    frame(&mut r, &mut term, document, 6);

    r.leave(&mut term, false);
    let payload = dumped(&term.take_output());

    for seq in ["\x1b[38;2;236;205;0m", "\x1b[48;2;45;40;56m", "\x1b[1;31m"] {
        assert!(
            payload.contains(seq),
            "the dump keeps {seq:?} - stripping it is the loss: {payload:?}"
        );
    }
    // ...and the text is still there, so "kept the colours" cannot pass on a
    // payload that is all escapes.
    assert!(payload.contains("yellow text"), "{payload:?}");
}

// Verifies: gh #374 gap 1 - the narrow strip. LCA's own in-band markers must not
// land in the scrollback as garbage, and pi strips exactly these two.
#[test]
fn the_exit_dump_strips_our_own_markers_and_nothing_else() {
    let mut r = AltScreenRenderer::new();
    let mut term = exit_term();
    let document = rows(&[
        "\x1b]133;A\x07\x1b[32mwith a zone prefix\x1b[39m",
        "caret\x1b_pi:c\x07 stays out",
        "\x1b]133;B\x07\x1b]133;C\x07two prefixes\x1b[33m and a colour\x1b[39m",
        // A zone marker that is NOT at the start is text, not a marker: the
        // strip is anchored, mirroring pi's `^`-anchored regex.
        "mid\x1b]133;A\x07row",
    ]);
    r.set_document(document.clone());
    frame(&mut r, &mut term, document, 6);

    r.leave(&mut term, false);
    let payload = dumped(&term.take_output());

    assert!(
        !payload.contains(CURSOR_MARKER),
        "the caret marker never reaches the scrollback: {payload:?}"
    );
    assert!(
        !payload.contains("\x1b]133;A\x07\x1b[32m"),
        "a leading zone prefix is stripped: {payload:?}"
    );
    assert!(
        payload.contains("with a zone prefix"),
        "and its text stays: {payload:?}"
    );
    assert!(
        payload.contains("\x1b[32m") && payload.contains("\x1b[33m"),
        "the colours beside them are kept: {payload:?}"
    );
    assert!(
        payload.contains("mid\x1b]133;A\x07row"),
        "an unanchored zone marker is text and stays: {payload:?}"
    );
    assert!(
        payload.contains("two prefixes"),
        "both prefixes stripped, text intact: {payload:?}"
    );
}

// Verifies: gh #374 gap 1 - **one** synchronized block, with autowrap toggled,
// so a many-row write is applied by the terminal at once instead of reflowing
// per line. Asserted as sequence *order* around the payload, not as presence:
// an `?2026h` anywhere would pass a presence check while the dump still went
// out row by row.
#[test]
fn the_exit_dump_is_one_synchronized_block_with_autowrap_off() {
    let mut r = AltScreenRenderer::new();
    let mut term = exit_term();
    let document: Vec<String> = (0..40).map(|i| format!("row {i}")).collect();
    r.set_document(document.clone());
    frame(&mut r, &mut term, document, 6);

    r.leave(&mut term, false);
    let out = term.take_output();
    let at = |needle: &str| {
        out.find(needle)
            .unwrap_or_else(|| panic!("{needle:?} is in the exit: {out:?}"))
    };
    assert_eq!(
        out.matches("\x1b[?2026h").count(),
        1,
        "one synchronized block for the whole dump: {out:?}"
    );
    assert!(
        at("\x1b[?2026h") < at("\x1b[?1049l"),
        "the block opens before the alt screen is left: {out:?}"
    );
    assert!(
        at("\x1b[?1049l") < at("\x1b[?7l"),
        "autowrap goes off after the exit: {out:?}"
    );
    assert!(
        at("\x1b[?7l") < at("row 0"),
        "and before the first row: {out:?}"
    );
    assert!(
        at("row 39") < at("\x1b[?2026l"),
        "every row is inside the block: {out:?}"
    );
    assert!(
        at("\x1b[?2026l") < at("\x1b[?7h"),
        "the block closes, **then** autowrap comes back on: {out:?}"
    );
    // The park is the last thing written. LCA's released gh #33 contract ends
    // `\r\n?7h?25h` and nothing may move the cursor after it, so the block
    // closes before the park rather than after it (pi ends `?25h?2026l`; the
    // older released contract wins).
    assert!(
        out.ends_with("\r\n\x1b[?7h\x1b[?25h"),
        "and the park is the final write: {out:?}"
    );
}

// Verifies: gh #33 - the `?1049l` cursor-restore quirk. `?1049l` puts the
// cursor back at its pre-alt position (usually row 1 col 1), so the dump needs
// a trailing newline or the returning shell prompt overwrites the last row.
// The new dump rewrote this code, so the guarantee is pinned again rather than
// assumed.
#[test]
fn the_exit_dump_ends_with_the_cursor_restore_newline() {
    let mut r = AltScreenRenderer::new();
    let mut term = exit_term();
    let document = rows(&["only row"]);
    r.set_document(document.clone());
    frame(&mut r, &mut term, document, 6);

    r.leave(&mut term, false);
    let payload = dumped(&term.take_output());
    let tail = payload.rsplit("only row").next().unwrap_or_default();
    assert!(
        tail.contains('\n'),
        "a newline after the last row, or the shell prints over it: {payload:?}"
    );
    assert!(
        tail.contains("\x1b[?7h"),
        "with autowrap restored before that newline: {payload:?}"
    );
}

// Verifies: gh #374 gap 1 - `preserve_screen` (the mid-session handoff) still
// writes no dump. `$EDITOR` and an interactive `!` command leave this way and
// come straight back, so a dump there would be a duplicate of the transcript.
#[test]
fn a_preserved_screen_writes_no_dump() {
    let mut r = AltScreenRenderer::new();
    let mut term = exit_term();
    let document = rows(&["SHOULD-NOT-APPEAR"]);
    r.set_document(document.clone());
    frame(&mut r, &mut term, document, 6);

    r.leave(&mut term, true);
    let out = term.take_output();
    assert!(
        out.contains("\x1b[?1049l") && out.contains(MOUSE_DISABLE),
        "the alt screen is left and the mouse released: {out:?}"
    );
    assert!(
        !out.contains("SHOULD-NOT-APPEAR"),
        "and nothing is dumped: {out:?}"
    );
}

// Verifies: gh #374 gap 1 - a very tall document is written whole, rather than
// being silently cut.
//
// # The bound, stated rather than discovered
//
// There is none, and none is imposed here. 200 rows is ~40 KiB; 5,000 is about
// 1 MiB, written as one `String` in one `write` on a path that runs once per
// session. The real ceiling is the transcript in memory, which the host builds
// every frame anyway - so this dump cannot be the thing that runs a machine out
// of memory, and capping it would reintroduce loss (a) at an arbitrary
// threshold the user never sees. pi has no cap either. A row pins the shape at
// a size large enough that a hidden `take(n)` would show.
#[test]
fn a_very_tall_document_is_dumped_whole_without_a_hidden_cap() {
    let mut r = AltScreenRenderer::new();
    let mut term = FakeTerminal::new(40, 6);
    let document: Vec<String> = (0..5_000).map(|i| format!("line {i:04}")).collect();
    r.set_document(document.clone());
    frame(&mut r, &mut term, document[..6].to_vec(), 6);

    r.leave(&mut term, false);
    let out = term.take_output();
    assert!(
        out.contains("line 0000") && out.contains("line 4999"),
        "both ends of a 5,000-row document are in the dump"
    );
    assert_eq!(
        out.matches("\x1b[2K").count(),
        5_000,
        "every row is cleared-and-painted exactly once (no hidden cap, no double write)"
    );
}

// Verifies: gh #374 gap 1 - the fallback. A renderer that never received a
// document (an embedder that only calls `render_lines`) still dumps what it
// has, styled. Without this the fix would trade one loss for a blank
// scrollback.
#[test]
fn without_a_document_the_exit_falls_back_to_the_frame_styled() {
    let mut r = AltScreenRenderer::new();
    let mut term = exit_term();
    frame(
        &mut r,
        &mut term,
        rows(&["\x1b[31mframe row\x1b[39m", "second"]),
        6,
    );

    r.leave(&mut term, false);
    let payload = dumped(&term.take_output());
    assert!(
        payload.contains("frame row"),
        "the frame is the best available document: {payload:?}"
    );
    assert!(
        payload.contains("\x1b[31m"),
        "and it is dumped styled, like any other: {payload:?}"
    );
}

// Verifies: gh #374 gap 1 - a document row wider than the pane is dumped whole,
// not truncated, and autowrap is off for it. The scrollback holds full-width
// content the user can scroll sideways into; truncating here would lose the
// tail of every long code line, which is a third way to lose work.
#[test]
fn a_row_wider_than_the_pane_is_dumped_whole() {
    let mut r = AltScreenRenderer::new();
    let mut term = FakeTerminal::new(20, 4);
    let long = format!("START{}END", "x".repeat(200));
    let document = rows(&[long.as_str()]);
    r.set_document(document.clone());
    r.render_lines(&mut term, document, 20, 4);
    let _ = term.take_output();

    r.leave(&mut term, false);
    let out = term.take_output();
    assert!(
        out.contains(&long),
        "the whole row, not a pane-width slice of it"
    );
}
