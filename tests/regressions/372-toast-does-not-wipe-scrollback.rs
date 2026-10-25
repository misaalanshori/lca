//! GitHub #372 bug 1: a toast must never wipe the scrollback.
//!
//! In scrollback mode `viewport` handed `paint_toasts` the **entire session
//! document**, and it indexed `i + 1` into it — correct for a screen slice of
//! `height` rows, wrong for a document of hundreds. The toast spliced into
//! **document row 1**, far above the visible window; the renderer then saw a change
//! below `prev_viewport_top` and called `full_render(true)` →
//! `\x1b[2J\x1b[H\x1b[3J`, destroying the screen *and the scrollback* on every toast
//! appearance and expiry. The toast was never visible either.
//!
//! The behavioural rows live beside the code (`crates/lca-ui/src/chat_toast_scrollback_tests.rs`).
//! This file is the NFR-24 named guard: it pins the *contract* from outside, so a
//! future caller cannot hand the painter a document again.

use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(rel: &str) -> String {
    let path = root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {path:?}: {err}"))
}

/// The render module's source without its test modules.
fn render_source() -> String {
    let source = read("crates/lca-ui/src/chat_render.rs");
    match source.find("\n#[cfg(test)]") {
        Some(at) => source[..at].to_string(),
        None => source,
    }
}

/// The body of one function, by brace-free line range: from its `fn` line to the
/// next line that closes it at the same indent.
fn function_body<'a>(source: &'a str, signature: &str) -> &'a str {
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("{signature} exists"));
    let rest = &source[start..];
    let end = rest.find("\n    }\n").map_or(rest.len(), |at| at + 7);
    &rest[..end]
}

// Verifies: gh #372 bug 1 - the painter's parameter is named for the contract it
// requires. `viewport` was the ambiguous name that let a document be passed as if it
// were a screen; the fix renames it to `screen`, and the doc comment says a slice of
// the frame's height.
//
// A name is weak evidence on its own, which is why the two rows below check the call
// sites as well - but it is the cheapest tripwire against the original spelling
// coming back.
#[test]
fn the_toast_painter_takes_a_screen_not_a_document() {
    let source = render_source();
    let body = function_body(&source, "pub(crate) fn paint_toasts(");
    assert!(
        body.contains("screen: &mut [String]"),
        "paint_toasts' slice must be named for what it is - a screen of the frame's \
         height: {body}"
    );
    assert!(
        !body.contains("viewport: &mut [String]"),
        "the ambiguous `viewport` name is what let a whole document be passed in"
    );
}

// Verifies: gh #372 bug 1 - the scrollback path never hands the painter the document.
//
// This is the exact call that caused the bug: `paint_toasts(&mut lines, ...)` inside
// the scrollback branch, where `lines` is `self.render(width)` - the whole session.
// It must go through `paint_toasts_at_bottom`, which slices the frame out first.
#[test]
fn the_scrollback_path_slices_the_frame_before_painting_toasts() {
    let source = render_source();
    let viewport = function_body(&source, "pub fn viewport(");
    // The scrollback branch is the `if !self.screen_mode` one.
    let plain = viewport
        .find("if !self.screen_mode {")
        .expect("the scrollback branch exists");
    let branch = &viewport[plain..];
    let end = branch.find("\n        }").unwrap_or(branch.len());
    let branch = &branch[..end];
    assert!(
        branch.contains("paint_toasts_at_bottom"),
        "the scrollback branch must slice the frame out before painting toasts, or \
         the toast lands in document row 1 again: {branch}"
    );
    assert!(
        !branch.contains("self.paint_toasts("),
        "the scrollback branch must never hand paint_toasts the whole document: {branch}"
    );
}

// Verifies: gh #372 bug 1 - the seam exists and does the subtraction once.
//
// `compose_overlays_bottom_anchored` had already been doing this correctly for the
// modals, which is why toasts were the only overlay that could reach document row 1.
// The toast seam mirrors it, so the document/screen distinction is one call rather
// than a subtraction at each site.
#[test]
fn the_bottom_anchored_seam_mirrors_the_overlay_composer() {
    let source = render_source();
    let body = function_body(&source, "pub(crate) fn paint_toasts_at_bottom(");
    assert!(
        body.contains("lines.len()"),
        "the seam computes the frame's offset from the document's length: {body}"
    );
    assert!(
        &body[start_of_offset(body)..].contains("&mut lines["),
        "and slices the document rather than copying it: {body}"
    );
}

/// Where the slice expression starts, so the assertion above does not depend on the
/// exact spelling of the offset arithmetic.
fn start_of_offset(body: &str) -> usize {
    body.find("&mut lines[").unwrap_or(0)
}

// Verifies: gh #372 bug 1 - the fullscreen path still passes its own frame.
//
// Alt-screen builds `lines` bottom-up as exactly the terminal's frame, so it hands
// the painter a screen slice directly. Pinning it keeps a future refactor from
// "unifying" the two paths by slicing a frame that is already one.
#[test]
fn the_fullscreen_path_paints_its_frame_directly() {
    let source = render_source();
    let viewport = function_body(&source, "pub fn viewport(");
    let tail = viewport
        .rfind("self.paint_toasts(&mut lines, width)")
        .expect("the fullscreen path paints its frame directly");
    assert!(
        tail > viewport.len() / 2,
        "the direct call belongs to the fullscreen path (after the scrollback \
         branch), not to the document path"
    );
}

// Verifies: gh #372 bug 1 - no UI code *emits* the scrollback clear.
//
// `\x1b[3J` destroys the scrollback. It has exactly one legitimate producer: the
// renderer's deliberate full redraw. A UI-side emitter would be a second one, and it
// would be the bug returning by another route.
//
// Doc comments are stripped first, and that is the whole difficulty: my own
// explanation of this bug quotes the sequence, and a naive scan flagged the
// documentation of the fix as the defect. A guard that cannot tell prose from code
// is a guard that fires on the wrong thing.
#[test]
fn only_the_renderer_emits_the_scrollback_clear() {
    for rel in [
        "crates/lca-ui/src/chat_render.rs",
        "crates/lca-ui/src/chat.rs",
        "crates/lca-ui/src/chat_overlays.rs",
    ] {
        for (line_no, line) in read(rel).lines().enumerate() {
            let code = line.trim_start();
            if code.starts_with("//") {
                continue;
            }
            assert!(
                !code.contains("3J"),
                "{rel}:{} must not clear the scrollback; that sequence belongs to \
                 the renderer's full redraw alone: {line}",
                line_no + 1
            );
        }
    }
}

// ============================================================================
// Bugs 2-4 of the same issue: three ways a frame acted without a reason
// ============================================================================

/// The overlay composition seam, so its guard can be read.
fn overlays_source() -> String {
    std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/crates/lca-ui/src/chat_overlays.rs"
    ))
    .expect("chat_overlays.rs is readable")
}

/// The scrollback renderer, so the two exemptions can be read.
fn main_screen_source() -> String {
    std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/crates/lca-tui/src/engine/main_screen.rs"
    ))
    .expect("main_screen.rs is readable")
}

// Verifies: gh #372 bug 2 - **the padding follows its reason.** The seam padded to the
// viewport height on every frame, so a fresh 8-line session in a 45-row terminal
// appended 37 blank rows and pushed the prompt and header out of the scrollback.
// Padding exists so an overlay lands at the visible bottom, so it is conditioned on an
// overlay being open - the same lesson bug 1 taught: act only when there is something
// to act on.
#[test]
fn the_viewport_padding_is_conditioned_on_an_open_overlay() {
    let source = overlays_source();
    let start = source
        .find("fn compose_overlays_bottom_anchored")
        .expect("the seam exists");
    let body = &source[start..];
    let end = body.find("\n    }\n").unwrap_or(body.len());
    let body = &body[..end];
    let guard = body
        .find("has_overlay()")
        .expect("the padding asks whether anything needs it");
    let pad = body
        .find("lines.resize(")
        .expect("the padding is still there");
    assert!(
        guard < pad,
        "the overlay check must precede the pad: {body}"
    );
    // ...and the predicate covers every overlay `compose_overlays` composites, so a new
    // one cannot be added without its bottom anchor.
    let predicate = source.find("fn has_overlay").expect("the predicate exists");
    let pred_body = &source[predicate..];
    let pred_end = pred_body.find("\n    }\n").unwrap_or(pred_body.len());
    let pred_body = &pred_body[..pred_end];
    for overlay in [
        "picker_open",
        "modal_active",
        "panel_open",
        "dialog",
        "wizard",
    ] {
        assert!(
            pred_body.contains(overlay),
            "the predicate must know about `{overlay}`"
        );
    }
}

// Verifies: gh #372 bug 3 - **the Termux exemption is height-only.** The soft keyboard
// changes the height by 10-15 rows every few seconds, and each change ran
// `full_render(true, ..)` -> `\x1b[2J\x1b[H\x1b[3J`, wiping screen and scrollback
// repeatedly. A width change still rebuilds, because it invalidates wrapping.
#[test]
fn the_termux_exemption_is_height_only() {
    let source = main_screen_source();
    // Anchor on the *arms*, not the flag computations: `let width_changed = ...` and
    // the `prev_viewport_top` branch also mention these names and come first.
    let width_arm = source
        .find("\n        if width_changed {")
        .expect("the width arm exists");
    let height_arm = source
        .find("\n        if height_changed && !self.termux {")
        .expect("the height arm exists");
    assert!(
        width_arm < height_arm,
        "a width change is decided first, so the height exemption cannot swallow it"
    );
    assert!(
        source.contains("TERMUX_VERSION"),
        "the detection reads Termux's own variable, not `TERM=screen`"
    );
}

// Verifies: gh #372 bug 4 - **the key is off by default and fires only on a shrink.**
// The risk in either direction: on by default would clear scrollback for everyone, and
// firing on growth would clear it on every append.
#[test]
fn clear_on_shrink_is_off_by_default_and_gated_on_shrinking() {
    let source = main_screen_source();
    assert!(
        source.contains("self.clear_on_shrink && new_lines.len() < self.max_lines_rendered"),
        "the clear is gated on the setting AND on the content having shrunk"
    );
    assert!(
        source.contains("pub fn set_clear_on_shrink"),
        "the setting has one setter"
    );
    assert!(
        !source.contains("clear_on_shrink: true"),
        "nothing turns it on by default"
    );
}
