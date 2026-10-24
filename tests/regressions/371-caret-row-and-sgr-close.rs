//! gh #371 bugs 1 and 2 — the caret's row must be one the terminal can be on, and
//! the painted caret must close only its own channel. NFR-24's guard.
//!
//! The behaviour rows live beside the code they drive:
//! `crates/lca-tui/src/engine/main_screen.rs` (5 rows: the viewport invariant, the
//! next-frame diff, a tall-prompt journey, the mixed-shape sweep, and the saturated
//! row) and `crates/lca-tui/src/widgets/editor.rs` (3 rows: the `27m` close, the
//! leak observed on following text, and the end-of-line form).
//!
//! This file guards the two things that would let the bugs come back silently.

use std::path::Path;

fn read(path: &str) -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(root.join(path)).unwrap_or_else(|err| panic!("read {path}: {err}"))
}

// Verifies: gh #371 bug 2 - no full SGR reset is emitted for a caret anywhere.
//
// The bug was three format strings. A row asserting "the caret closes with 27m" passes
// for any edit that changes the string, so the guard is the negative: the *combination*
// of reverse video and a full reset does not exist in the editor at all. A future caret
// form ("a block caret", "an underline caret") that reintroduces `\x1b[0m` next to `7m`
// fails here regardless of how it is spelled.
#[test]
fn no_caret_closes_reverse_video_with_a_full_reset() {
    let source = read("crates/lca-tui/src/widgets/editor.rs");
    let offenders: Vec<String> = source
        .lines()
        .enumerate()
        .filter(|(_, line)| {
            // A caret is reverse video; a full reset beside it is the leak.
            line.contains("7m") && line.contains("\\x1b[0m")
        })
        .map(|(index, line)| format!("{}: {}", index + 1, line.trim()))
        .collect();
    assert!(
        offenders.is_empty(),
        "a caret must close only reverse video (ADR-0037's channel-scoped rule); a \
         full reset strips the foreground, background and decorations of everything \
         after it:\n{}",
        offenders.join("\n")
    );
    // And the channel-scoped close really is there, so the guard above cannot be
    // satisfied by deleting the caret.
    assert!(
        source.contains("\\x1b[7m{g}\\x1b[27m") && source.contains("\\x1b[7m \\x1b[27m"),
        "both caret forms (a grapheme, and the end-of-line space) close with 27m"
    );
}

// Verifies: gh #371 bug 1 - the hardware cursor row is only ever recorded for a row
// the terminal can address.
//
// The defect was not the arithmetic, it was *which fact was stored*: the row we asked
// for, not the row the terminal clamped to. So the guard is that the one function which
// writes `hardware_cursor_row` refuses an unaddressable row before it writes anything -
// and that the viewport, not the document, decides.
#[test]
fn the_cursor_row_is_recorded_only_inside_the_viewport() {
    let source = read("crates/lca-tui/src/engine/main_screen.rs");
    let start = source
        .find("fn position_hardware_cursor(")
        .expect("the positioning function exists");
    let body = &source[start..];
    let body = &body[..body.find("\n    }\n").unwrap_or(body.len())];

    // The guard exists, names the viewport, and hides rather than clamping.
    assert!(
        body.contains("previous_viewport_top") && body.contains("viewport_bottom"),
        "the recorded row is bounded by the *viewport*, not the document: {body}"
    );
    assert!(
        body.contains("term.hide_cursor();\n            return;"),
        "an unaddressable caret hides the cursor and returns before recording \
         anything - clamping it to the viewport top would park an IME candidate \
         window on an unrelated line: {body}"
    );
    // And the write comes after the guard - with **one** write in the function.
    //
    // Counting matters as much as ordering: a mutation that *prepends* a write leaves
    // the one guarded write still last, so an ordering-only check passes while the
    // function now stores the requested row before deciding it cannot. The invariant is
    // "every write is downstream of the guard", which is a count of one.
    let guard = body.find("viewport_bottom").expect("the guard");
    let writes: Vec<usize> = body
        .match_indices("self.hardware_cursor_row = ")
        .map(|(at, _)| at)
        .collect();
    assert_eq!(
        writes.len(),
        1,
        "`position_hardware_cursor` stores the row exactly once, after the guard; \
         more than one write is more than one place to forget the clamp"
    );
    assert!(
        guard < writes[0],
        "the clamp must happen *before* the row is stored - storing the requested row \
         and letting the emulator clamp the result is gh #371 itself"
    );
}

// Verifies: gh #371 - the main screen is the only renderer with this shape.
//
// The brief asked whether `alt_screen.rs` shares bug 1. It does not: it positions with
// an absolute `CSI row;col H` every frame and keeps no `hardware_cursor_row`, so a
// caret it cannot address cannot corrupt a later frame. Pinned so a future
// delta-positioning optimisation there does not arrive without the same guard.
#[test]
fn alt_screen_has_no_recorded_cursor_row_to_desynchronise() {
    let source = read("crates/lca-tui/src/engine/alt_screen.rs");
    assert!(
        !source.contains("hardware_cursor_row"),
        "alt_screen positions absolutely (CSI row;col H) and records no cursor row, \
         which is why gh #371 bug 1 is main-screen-only; introducing a recorded row \
         here needs the viewport guard too"
    );
    assert!(
        source.contains("\\x1b[{};{}H"),
        "and it positions absolutely, which is the property that makes it immune: \
         every frame rewrites the whole screen from row 1"
    );
}

// Verifies: gh #371 - the caret is still painted, in both renderers.
//
// The contract ADR-0037 states: the caret is painted into the row and the terminal
// cursor is never trusted. Fixing the close sequence must not have moved the caret to
// the hardware cursor as a side effect.
#[test]
fn the_painted_caret_is_still_the_visual_caret() {
    let editor = read("crates/lca-tui/src/widgets/editor.rs");
    assert!(
        editor.contains("paint_caret: true,"),
        "the painted caret is the default (ADR-0037)"
    );
    assert!(
        editor.contains("CURSOR_MARKER"),
        "and the marker still rides the caret cell, so the engine can anchor IME"
    );
    // The engine hides the hardware cursor when the component painted its own.
    let main = read("crates/lca-tui/src/engine/main_screen.rs");
    assert!(
        main.contains("if painted {"),
        "the main screen honours the `painted` flag from the marker"
    );
    let alt = read("crates/lca-tui/src/engine/alt_screen.rs");
    assert!(alt.contains("if painted {"), "and so does the alt screen");
}
