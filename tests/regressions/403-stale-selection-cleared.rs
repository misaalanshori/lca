//! GitHub #403 gap 1: a transcript replacement must not keep the previous
//! transcript's selection.
//!
//! The bug: `clear_selection` fired on a picker press only, so every path
//! that replaces the transcript - a `/resume` switch, a `/tree` branch jump,
//! a fork, a clone, an import - left the previous transcript's selection
//! rectangles painted over rows that now hold different text. A selection is
//! in *content* coordinates, so a copy after the replacement handed back
//! bytes from a transcript that no longer existed: a wrong-data path, not
//! just a stale highlight.
//!
//! The fix is a seam rather than five call sites: `Transcript::generation`
//! moves only when the transcript is *cleared*, and the run loop resets the
//! selection whenever it sees a new value. Appends (streaming deltas),
//! resizes, and marker repaints leave the generation alone, so a selection
//! over unchanged rows survives - the mirror-image bug the brief warns about.
//!
//! This file pins the mechanism from outside the crates: the seam exists, it
//! is the only thing that clears, and the reset clears the multi-click run as
//! well as the rectangles. The behavioural bite-proof lives beside the logic
//! in `crates/lca-ui/src/run_selection_tests.rs` (removing the reset fails
//! five of seven rows by name); this one guards against the fix being
//! refactored away while those rows are moved or renamed.

use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(rel: &str) -> String {
    let path = root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {path:?}: {err}"))
}

#[test]
fn the_only_transcript_replacement_bumps_a_generation() {
    let transcript = read("crates/lca-ui/src/transcript/mod.rs");

    // `clear` is the replacement, and it is the only thing that bumps.
    let clear_start = transcript
        .find("pub fn clear(&mut self) {")
        .expect("Transcript::clear exists");
    let clear_body = &transcript[clear_start..];
    let clear_end = clear_body.find("\n    }\n").expect("clear has a body");
    assert!(
        clear_body[..clear_end].contains("self.generation = self.generation"),
        "Transcript::clear must bump the generation: that bump is what tells \
         the run loop a selection no longer addresses live rows (gh #403)"
    );

    // And the accessor the loop reads exists.
    assert!(
        transcript.contains("pub fn generation(&self) -> u64"),
        "Transcript::generation() is the seam's reader"
    );

    // Appends must NOT bump it: a streaming delta keeps the earlier rows
    // exactly where they were, so the user's selection over them stays
    // valid. This is the mirror-image half of the contract.
    for append in ["fn push_notice", "fn push_raw", "fn push_user"] {
        if let Some(at) = transcript.find(&format!("pub fn {append}"))
            && let Some(end) = transcript[at..].find("\n    }\n")
            && transcript[at..at + end].contains("self.generation =")
        {
            panic!("{append} must not bump the generation: an append does not replace rows");
        }
    }
}

#[test]
fn the_reset_clears_the_click_run_too() {
    let alt = read("crates/lca-tui/src/engine/alt_screen.rs");
    let start = alt
        .find("pub fn reset_text_selection(&mut self) {")
        .expect("AltScreenRenderer::reset_text_selection exists (gh #403 gap 1)");
    let body = &alt[start..];
    let end = body.find("\n    }\n").expect("the reset has a body");
    let reset = &body[..end];

    // pi's `resetTextSelection()` drops the selection *and* `lastClick`.
    // Leaving `click_count` at 2 or 3 lets the next single click continue the
    // old run and select a word or a whole line the user never asked for.
    for required in [
        "self.selection.clear()",
        "self.click_count = 0",
        "self.last_click = None",
    ] {
        assert!(
            reset.contains(required),
            "the reset must include `{required}`; a selection is more than \
             rectangles, and a stale click run corrupts the next gesture"
        );
    }
}

#[test]
fn the_run_loop_resets_at_one_seam_not_at_call_sites() {
    let run = read("crates/lca-ui/src/run.rs");
    let selection = read("crates/lca-ui/src/run_selection.rs");

    // The decision is a function, so the loop and the tests answer it the
    // same way, and the comparison is not buried in the loop body.
    assert!(
        selection.contains("fn reset_selection_if_replaced("),
        "the seam decision must be one function (gh #403 gap 1)"
    );
    assert!(
        run.contains("reset_selection_if_replaced("),
        "the run loop must call the seam"
    );

    // The seam compares a generation rather than being sprinkled over the
    // five replacement paths: four call sites is how the fifth gets missed.
    assert!(
        selection.contains("chat.transcript.generation()"),
        "the seam decides on the transcript generation, not on which command ran"
    );
}
