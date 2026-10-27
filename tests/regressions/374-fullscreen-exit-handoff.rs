//! GitHub #374 gap 1: leaving fullscreen kept only the visible window, and
//! lost every colour doing it.
//!
//! ## The defect, measured
//!
//! `AltScreenRenderer::leave` wrote `self.previous` — which `render_lines`
//! resizes to exactly `height` — and ran `strip_terminal_sequences` over every
//! row first. In a real 100×24 pane with eight shell cards and `/exit` while
//! fullscreen, the dump after `\x1b[?1049l` was 2426 bytes, contained
//! `MARKER-8` and **not** `MARKER-1`, and held **zero** SGR sequences.
//!
//! Two losses, and the first is data loss rather than a cosmetic complaint:
//!
//! 1. **Truncation.** A 200-line session kept 24 rows. The scrollback is the
//!    durable record of the session, so everything the user could scroll through
//!    must leave the alt screen — pi renders the whole document
//!    (`this.render(width)` in `afterTerminalStop`).
//! 2. **De-styling.** `strip_terminal_sequences` removes *every* escape,
//!    including syntax highlights, diffs, markdown and theme colours. pi strips
//!    exactly `OSC133_ZONE_PREFIX` and `CURSOR_MARKER` — its own in-band markers
//!    — and keeps SGR.
//!
//! ## Why this file checks sources as well as behaviour
//!
//! The behavioural rows live beside the code
//! (`crates/lca-tui/src/engine/alt_screen.rs::exit_handoff_tests`, and
//! `lca-ui`'s `run_tests`). This file is the NFR-24 named guard, so it pins the
//! *seams* from outside: which function does the narrow strip, that the broad
//! strip did not come back into `leave`, that the host actually feeds a
//! document, and that the two composition calls cannot drift.
//!
//! Verifies: NFR-24 (a released defect gets a named guard) and gh #374 gap 1.

use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(rel: &str) -> String {
    let path = root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {path:?}: {err}"))
}

/// A source file without its test modules, so no row can be satisfied by a
/// test's own spelling of the seam.
fn production_source(rel: &str) -> String {
    let source = read(rel);
    match source.find("\n#[cfg(test)]") {
        Some(at) => source[..at].to_string(),
        None => source,
    }
}

/// The body of one function, found by brace matching from its signature.
///
/// Brace matching rather than "the next line at the same indent": these functions
/// contain nested blocks and `match` arms, and a shallow scan stops inside a
/// comment or an inner block, which makes a row pass or fail for the wrong
/// reason.
fn function_body<'a>(source: &'a str, signature: &str) -> &'a str {
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("{signature} exists"));
    let bytes = source.as_bytes();
    let mut depth = 0usize;
    let mut seen = false;
    let mut i = start;
    while i < bytes.len() {
        match bytes[i] {
            b'{' => {
                depth += 1;
                seen = true;
            }
            b'}' => {
                depth -= 1;
                if seen && depth == 0 {
                    return &source[start..=i];
                }
            }
            _ => {}
        }
        i += 1;
    }
    &source[start..]
}

// Verifies: gh #374 gap 1 (loss (b)) - the exit uses a **narrow** strip, not the
// broad one. This is the exact line that de-styled every preserved transcript:
// `out.push_str(&strip_terminal_sequences(line))`. A behaviour row would catch a
// regression only if it happened to assert on a styled document; this one names
// the instrument, so re-introducing the broad call fails immediately.
#[test]
fn the_exit_uses_a_narrow_strip_not_the_broad_one() {
    let source = production_source("crates/lca-tui/src/engine/alt_screen.rs");
    let leave = function_body(&source, "pub fn leave(&mut self, term: &mut dyn Terminal");
    assert!(
        !leave.contains("strip_terminal_sequences"),
        "the broad strip removes every colour - that was loss (b): {leave}"
    );
    assert!(
        leave.contains("strip_own_markers"),
        "the exit strips exactly LCA's own markers: {leave}"
    );
    // ...and the narrow helper is what keeps SGR.
    let helper = function_body(&source, "fn strip_own_markers(line: &str)");
    assert!(
        helper.contains("CURSOR_MARKER") && helper.contains("strip_osc133_prefix"),
        "the two markers pi strips, and only those: {helper}"
    );
}

// Verifies: gh #374 gap 1 (loss (a)) - the exit reads the document, not the
// frame. `self.previous` is exactly `height` rows by construction
// (`render_lines` resizes to it), so dumping it is the 200-into-24 bug.
#[test]
fn the_exit_dumps_the_document_rather_than_the_frame() {
    let source = production_source("crates/lca-tui/src/engine/alt_screen.rs");
    let leave = function_body(&source, "pub fn leave(&mut self, term: &mut dyn Terminal");
    assert!(
        leave.contains("self.document"),
        "the dump reads the recorded document: {leave}"
    );
    // `self.previous` is allowed in exactly one place: the fallback for a caller
    // that never set a document (an embedder driving only `render_lines`). It must
    // not be the row source.
    for (index, _) in leave.match_indices("self.previous") {
        let context = &leave[index.saturating_sub(120)..leave.len().min(index + 120)];
        assert!(
            context.contains("is_empty()"),
            "`self.previous` may only be the empty-document fallback, never the \
             row source - that is the 200-into-24 bug: {context}"
        );
    }
}

// Verifies: gh #374 gap 1 - the atomic write. One synchronized block, autowrap
// off across it, so a many-row dump is applied by the terminal in one go instead
// of reflowing per line. Pinned as one contiguous literal so the ordering cannot
// drift apart: pi's `afterTerminalStop` writes exactly this prefix.
#[test]
fn the_exit_opens_one_synchronized_block_with_autowrap_off() {
    let source = production_source("crates/lca-tui/src/engine/alt_screen.rs");
    let leave = function_body(&source, "pub fn leave(&mut self, term: &mut dyn Terminal");
    assert!(
        leave.contains(r#"\x1b[?2026h\x1b[?1049l\x1b[?7l""#),
        "synchronized-open, leave, autowrap-off as one write: {leave}"
    );
    assert!(
        leave.matches(r"\x1b[?2026h").count() == 1,
        "exactly one block for the whole dump: {leave}"
    );
}

// Verifies: gh #33 - the cursor-restore newline survives the rewrite. `?1049l`
// returns the cursor to its pre-alt position, so without a trailing newline the
// returning shell prompt prints over the last dumped row.
#[test]
fn the_exit_still_ends_with_the_cursor_restore_newline() {
    let source = production_source("crates/lca-tui/src/engine/alt_screen.rs");
    let leave = function_body(&source, "pub fn leave(&mut self, term: &mut dyn Terminal");
    assert!(
        leave.contains(r"\r\n"),
        "the trailing newline of gh #33 is still written: {leave}"
    );
    assert!(
        leave.contains(r"\x1b[?7h"),
        "with autowrap restored before it: {leave}"
    );
}

// Verifies: gh #374 gap 1 - `preserve_screen` still writes no dump. That branch
// is the mid-session handoff (`$EDITOR`, an interactive `!`), which comes
// straight back to the alt screen, so a dump there would duplicate the
// transcript. The row is here because a rewrite of `leave` is exactly the moment
// the branch could be folded away.
#[test]
fn the_preserving_branch_writes_no_dump() {
    let source = production_source("crates/lca-tui/src/engine/alt_screen.rs");
    let leave = function_body(&source, "pub fn leave(&mut self, term: &mut dyn Terminal");
    let preserving = leave
        .find("if preserve_screen {")
        .unwrap_or_else(|| panic!("the preserve branch exists: {leave}"));
    let dump = leave
        .find(r"\x1b[?2026h\x1b[?1049l")
        .unwrap_or_else(|| panic!("the dump exists: {leave}"));
    assert!(
        preserving < dump,
        "the preserve branch is decided before the dump is written: {leave}"
    );
}

// Verifies: gh #374 gap 1 - the host actually feeds the document. A renderer
// that dumps its document faithfully proves nothing if `lca-ui` never sets one,
// which is the shape this defect had: the renderer did exactly what it was told
// with the wrong input.
#[test]
fn the_interface_sets_the_document_on_every_fullscreen_frame() {
    let source = production_source("crates/lca-ui/src/run.rs");
    assert!(
        source.contains("set_document"),
        "the alt screen is told what the whole session is: {source}"
    );
    let render = function_body(&source, "fn render(");
    assert!(
        render.contains("set_document"),
        "and it is set on the render path, so every frame refreshes it: {render}"
    );
    assert!(
        render.contains("Screen::Main"),
        "the main screen keeps its own path (it has no exit dump to feed): {render}"
    );
}

// Verifies: gh #374 gap 1 - the frame and the document come from **one**
// composition, and the host does not pay for a second one.
//
// The first shape of this fix asked `Chat` for the document with a second call,
// which composes the transcript again: measured at **9.2 ms per frame** on a
// 300-entry transcript at 100 columns - more than half a 60 Hz frame budget, on
// the path where frames are most frequent. So the document is recorded by the
// `viewport` call that already walked the sections, and taken from there.
#[test]
fn the_document_rides_the_frame_composition_instead_of_a_second_walk() {
    let source = production_source("crates/lca-ui/src/chat_render.rs");
    let viewport = function_body(&source, "pub fn viewport(&self, width: u16, height: u16");
    assert_eq!(
        viewport.matches("self.sections(width)").count(),
        1,
        "one sections walk per frame: {viewport}"
    );
    assert!(
        viewport.contains("*self.document.borrow_mut() = document;"),
        "and the document is recorded from that one walk: {viewport}"
    );
    let take = function_body(&source, "pub fn take_document(&self)");
    assert!(
        take.contains("std::mem::take(&mut self.document.borrow_mut())"),
        "the host takes rather than recomposes: {take}"
    );
    // The stale two-output helper must be gone: leaving it would invite a caller
    // back onto the double walk.
    assert!(
        !source.contains("fn viewport_with_document"),
        "the double-composition entry point is gone, not merely unused"
    );
}

// ============================================================================
// gh #374 gap 2 - `FOCUS_OUT` must not wipe a finished selection.
//
// The behavioural rows live beside the code
// (`lca-tui`'s `alt_screen::focus_tests` and `lca-ui`'s
// `run_tests::focus_reports`). This file is the NFR-24 named guard, so it pins
// the two *seams* from outside, because this defect had a rare shape: the
// guarded `clear()` alone would have been **dead code**, and every row would
// still have passed.
// ============================================================================

// Verifies: gh #374 gap 2 - the focus arms are reachable, and the clear is
// guarded rather than unconditional.
//
// Two independent ways the fix can be inert, both pinned here:
//
// 1. The old arm was `if data == "\x1b[O" { self.selection.clear(); }`. A guard
//    that keeps that `clear()` on the finished path is the bug itself.
// 2. `\x1b[O` never reached the renderer: the host dispatched to the screen only
//    inside the `\x1b[<` gate, so the arm could not run at all. That is why this
//    row checks *both* files - a renderer row cannot see the host's gate.
#[test]
fn a_focus_loss_is_dispatched_and_only_clears_an_in_progress_gesture() {
    let alt = production_source("crates/lca-tui/src/engine/alt_screen.rs");
    let focus = function_body(&alt, "fn on_focus_out(&mut self)");
    // The exact predicate, not merely a mention of the field: the first version
    // of this row asserted only `focus.contains("press_active")`, which a
    // mutation setting `let interrupted = true;` still satisfied - a vacuous
    // guard. The assignment is what has to be there.
    assert!(
        focus.contains("let interrupted = self.selection.press_active;"),
        "the guard asks 'was a press still unreleased', read from the predicate \
         that a release also clears: {focus}"
    );
    // And the clear is *inside* that condition, never before it.
    let clear_at = focus
        .find("self.selection.clear();")
        .unwrap_or_else(|| panic!("the interrupted gesture is still cleared: {focus}"));
    let guard_at = focus
        .find("if interrupted {")
        .unwrap_or_else(|| panic!("the clear is behind `if interrupted`: {focus}"));
    assert!(
        guard_at < clear_at,
        "the clear sits behind `if interrupted`, so a finished selection \
         survives: {focus}"
    );
    // Each non-selection gesture state is aborted, because one can be forgotten.
    for field in [
        "self.drag_edge = None;",
        "self.press_cell = None;",
        "self.scrollbar_drag = false;",
        "self.click_count = 0;",
    ] {
        assert!(
            focus.contains(field),
            "focus loss must abort `{field}` too, or it trades one stale state \
             for another: {focus}"
        );
    }

    // And the host dispatches the report outside the SGR-mouse gate.
    let run = production_source("crates/lca-ui/src/run.rs");
    let dispatch = function_body(&run, "fn handle_input(");
    // The arm's own condition, spelled out: a mutation that deletes the arm or
    // moves it behind the `\x1b[<` gate must not satisfy this.
    assert!(
        dispatch.contains("if data == lca_tui::engine::alt_screen::FOCUS_OUT"),
        "the host has an arm that dispatches a focus-out by name: {dispatch}"
    );
    let gate = dispatch
        .find("data.starts_with(\"\\x1b[<\")")
        .unwrap_or_else(|| panic!("the SGR gate exists"));
    let focus_arm = dispatch
        .find("if data == lca_tui::engine::alt_screen::FOCUS_OUT")
        .unwrap_or_else(|| panic!("the focus arm exists"));
    assert!(
        focus_arm < gate,
        "the focus arm sits before the `\\x1b[<` gate: a focus report does not \
         start with `\\x1b[<`, so inside the gate it is unreachable - which is \
         how the arm stayed dead"
    );
    assert!(
        dispatch.contains("chat.scroll_drag = None;"),
        "and the host clears its own half (the scrollbar grab), which no \
         renderer reset can reach"
    );
}

// Verifies: gh #374 gap 2 - the alt screen asks for focus reports at all.
//
// Without `?1004h` the terminal never sends `\x1b[O`, so the guard above could
// never run: the fix would be correct and unreachable, which is the worst kind
// of green. pi enables it in `ENABLE_BUTTON_MOTION_MOUSE`.
#[test]
fn the_fullscreen_renderer_asks_for_focus_reports_and_stops_them() {
    let alt = production_source("crates/lca-tui/src/engine/alt_screen.rs");
    let enable = function_body(&alt, "pub fn mouse_enable_sequences()");
    assert!(
        enable.contains("\\x1b[?1004h"),
        "the terminal is asked to report focus changes: {enable}"
    );
    let disable = alt
        .lines()
        .find(|line| line.contains("pub const MOUSE_DISABLE"))
        .unwrap_or_else(|| panic!("MOUSE_DISABLE exists"));
    assert!(
        disable.contains("\\x1b[?1004l"),
        "and the exit turns it off, or the shell prompt reads the reports as \
         typing: {disable}"
    );
}

// ============================================================================
// gh #374 gap 3 - dragging backwards kept the word you double-clicked.
//
// The behavioural rows live beside the code (`lca-tui`'s
// `selection::backward_drag_tests` for the direction matrix, and
// `alt_screen::backward_drag_tests` for the same through the real mouse path).
// This file is the NFR-24 named guard, so it pins the *shape* that makes those
// rows pass: the initial range carries its row, and the comparison is
// lexicographic.
// ============================================================================

// Verifies: gh #374 gap 3 - the anchor range is remembered **with its row**, and
// the direction test compares `(row, col)` rather than a column alone.
//
// Both halves are load-bearing and each fails differently:
//
// - a range without a row cannot tell that a drag *up* is "before", so the
//   vertical rows keep the anchor on the wrong side;
// - a column-only comparison calls down-left "before" and up-right "after",
//   which is exactly the pair of diagonals a naive fix gets backwards.
#[test]
fn the_anchor_range_carries_its_row_and_the_direction_is_lexicographic() {
    let source = production_source("crates/lca-tui/src/engine/selection.rs");

    // The range type has a row, and `start` fills it in.
    let anchor = source
        .split("struct AnchorRange")
        .nth(1)
        .unwrap_or_else(|| panic!("AnchorRange exists: {source}"));
    let anchor = &anchor[..anchor.find('}').unwrap_or(anchor.len())];
    assert!(
        anchor.contains("row: u16"),
        "the remembered range carries the row it was captured on: {anchor}"
    );

    let start = function_body(&source, "pub fn start(");
    assert!(
        start.contains("row: point.row") && start.contains("line_word_range"),
        "and `start` records the press row with the word's columns: {start}"
    );

    // The comparison, spelled out. `(point.row, target_start) < (initial.row, ...)`
    // is the tuple form; the assertion is on the tuple so a rewrite back to
    // `point.col <` fails here rather than only in the behavioural rows.
    let update = function_body(&source, "pub fn update(");
    assert!(
        update.contains("(point.row, target_start) < (initial.row, initial.start)"),
        "the direction test compares row first, then column: {update}"
    );
    assert!(
        update.contains("initial.end") && update.contains("initial.start"),
        "and both endpoints come from the remembered range, not re-derived: {update}"
    );
    // The pre-fix shape: the anchor never moved, so the pressed word could fall
    // out of the normalized range entirely.
    assert!(
        update.contains("self.anchor = Some("),
        "the backward direction reassigns the anchor: {update}"
    );
    assert!(
        update.contains("Granularity::Line"),
        "line granularity takes the same path, per the issue: {update}"
    );
}

// Verifies: gh #374 gap 3 - line granularity compares *ranges*, not columns.
//
// The issue names this as the same defect: `update` used to test
// `point.col < anchor.col` on the target row, which is meaningless for a
// full-width line. A fix that only handled the word case would leave it.
#[test]
fn line_granularity_derives_its_range_from_the_target_line() {
    let source = production_source("crates/lca-tui/src/engine/selection.rs");
    let update = function_body(&source, "pub fn update(");
    // The line arm spans the target line's own width, so a line drag grows by
    // whole lines rather than by columns.
    assert!(
        update.contains("Granularity::Line => (0, visible_width(line) as u16)")
            || update.contains("Granularity::Line =>"),
        "the line case builds the target's full-line range: {update}"
    );
    assert!(
        update.contains("visible_width(line)"),
        "and it uses the target's width, not the anchor's column: {update}"
    );
}
