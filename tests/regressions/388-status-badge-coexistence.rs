//! GitHub #388 gap 1: the separator's status and its `↑ N more` badge share
//! one row, and they degrade instead of colliding.
//!
//! The defect: `Separator::render_inner` knew nothing of an overflow count.
//! A caller that painted the badge onto that row wrote over the status, and a
//! long status wrote over the badge — so the one row that is the dock's only
//! status channel corrupted itself exactly when a long prompt made the count
//! matter.
//!
//! The behavioural rows live beside the code (`crates/lca-ui/src/separator.rs`'s
//! own module). This file is the NFR-24 named guard: it pins the *shape* from
//! outside, so a refactor cannot reintroduce a second painter, a byte-length
//! measurement, or a bare `render_top` that forgets the count.

use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(rel: &str) -> String {
    let path = root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {path:?}: {err}"))
}

/// The separator's source, without its test module — the tests are allowed to
/// mention whatever they like.
fn separator_source() -> String {
    let source = read("crates/lca-ui/src/separator.rs");
    match source.find("#[cfg(test)]") {
        Some(at) => source[..at].to_string(),
        None => source,
    }
}

// Verifies: gh #388 gap 1 - the badge is composed by the separator itself.
//
// While the widget knew nothing of a count, the only way to paint one was for
// the caller to append it after the fact, which is what made the collision
// possible: two painters, one row, no shared idea of what was already there.
// The widget must therefore render the count from a count it was *told*.
#[test]
fn the_separator_renders_the_overflow_badge_itself() {
    let source = separator_source();
    assert!(
        source.contains("more"),
        "the separator must compose the `↑ N more` badge rather than leaving it \
         to a caller that would paint over the status"
    );
    assert!(
        source.contains("render_top_with"),
        "the top cap must accept the hidden count (render_top_with)"
    );
    assert!(
        source.contains("render_bottom_with"),
        "the bottom cap must accept its own hidden count (render_bottom_with), \
         because rows above and rows below are different facts"
    );
}

// Verifies: gh #388 gap 1 - the fit is measured in cells.
//
// A byte length makes the fitter lie about a row holding a wide glyph, which is
// how #387/#378 shipped. The separator already imported `visible_width`; this
// row says every width decision keeps going through it.
#[test]
fn every_width_decision_measures_cells_not_bytes() {
    let source = separator_source();
    for forbidden in [".len() < ", ".len() > ", ".len() <= ", ".len() >= "] {
        assert!(
            !source.contains(forbidden),
            "`{forbidden}` is a byte comparison in a width decision: measure with \
             visible_width instead"
        );
    }
    assert!(
        source.contains("visible_width"),
        "the fitter must measure with visible_width"
    );
}

// Verifies: gh #388 gap 1 - the degradation order lives in one function.
//
// Three tiers (full status + badge, spinner-only + badge, spinner-only) is the
// contract. Splitting them across callers is how they drift, so the fixed order
// is asserted structurally: the full-status branch is tried before the
// spinner-only branch, and both before the bare fallback.
#[test]
fn the_degradation_tiers_are_tried_in_order() {
    let source = separator_source();
    let start = source
        .find("fn render_with_badge")
        .expect("the badge fitter exists");
    let body = &source[start..];
    // The end of the function: the next `fn ` at the same indentation.
    let end = body.find("\n    }\n").unwrap_or(body.len());
    let body = &body[..end];

    // Two halves, because order alone is a guard that cannot fail: a tier-1
    // branch that is *unreachable* (`if false && ...`) satisfies every ordering
    // assertion while the row drops the words on every width. Mutation testing
    // found exactly that, so the branch's own condition is checked too.
    //
    // The anchors are semantic (`status_width` is tier 1's measurement,
    // `spinner_width` tier 2's), not the arithmetic's spelling: keying on
    // `3 + x + 1` breaks on a rename or a `cargo fmt` reflow without the order
    // changing - the trap that made earlier shape guards in this repo worthless.
    let tier1_gate = body
        .split("let only_spinner")
        .next()
        .expect("tier 1 precedes tier 2");
    assert!(
        tier1_gate.contains("if badge_start >= left_width"),
        "tier 1's branch must be a live condition on the measured gap, not \
         disabled while the ordering still reads correctly: {tier1_gate}"
    );
    assert!(
        !tier1_gate.contains("if false"),
        "tier 1 must not be short-circuited to false"
    );
    let full = body
        .find("let status = self.status_text(")
        .expect("tier 1: the full status is composed first");
    let full_width = body
        .find("3 + status_width")
        .expect("tier 1: measured so the gap can be checked");
    let spinner = body
        .find("let only_spinner")
        .expect("tier 2: the spinner alone, so the count still lands");
    let bare = body
        .find("let prefix = 3.min")
        .expect("tier 4: the flush spinner when even the head cannot be placed");
    assert!(
        full < spinner && full_width < spinner,
        "the full status must be measured and tried before dropping to the \
         spinner, or the row loses its words while the width still had room"
    );
    assert!(
        spinner < bare,
        "the badge is the last thing to give up, not the first"
    );
}

// Verifies: gh #388 gap 1 - the capped editor window is what feeds the badge.
//
// A badge with no cap is a badge that can never appear: the editor rendered
// every row, so nothing was ever hidden and a long prompt simply pushed the
// composer's frame off the screen. The two halves must exist together.
#[test]
fn the_editor_window_is_capped_and_the_count_comes_from_it() {
    let render = read("crates/lca-ui/src/chat_render.rs");
    assert!(
        render.contains("fn editor_view"),
        "the frame must compute a bounded editor window (editor_view)"
    );
    assert!(
        render.contains("render_top_with"),
        "the capped window's hidden count must reach the separator's top cap"
    );
    // The cap is pi's rule, stated so a future change has to argue with it.
    assert!(
        render.contains("* 3 / 10"),
        "the window is 30% of the terminal's rows (pi's `editor.ts`), not an \
         arbitrary constant"
    );
    assert!(render.contains(".max(5)"), "and never fewer than five rows");
}

// Verifies: gh #388 gap 1 - the frame, the hit-test and the caret all read the
// same window.
//
// gh #331's lesson: a second copy of the height subtraction is a row-off hit
// test waiting to happen. The editor's rect must come from `editor_view`, not
// from a fresh count of rendered rows.
#[test]
fn the_editor_rect_reads_the_same_window_as_the_frame() {
    let render = read("crates/lca-ui/src/chat_render.rs");
    let start = render
        .find("pub fn editor_rect")
        .expect("editor_rect exists");
    let body = &render[start..];
    let end = body.find("\n    }\n").unwrap_or(body.len());
    let body = &body[..end];
    assert!(
        body.contains("editor_view()"),
        "editor_rect must take its height from editor_view, or clicks address \
         rows the frame never painted: {body}"
    );
}
