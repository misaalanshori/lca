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
