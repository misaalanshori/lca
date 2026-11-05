//! NFR-24 guard for gh #375 gaps 2 and 4: the scrollbar column keeps the block's
//! background, and plain `Home`/`End` belong to the editor alone.
//!
//! # Gap 2, the 1-cell black hole
//!
//! The scrollbar column was built by truncating the line, padding with spaces and
//! appending the thumb glyph. Truncation ends with a reset, so the cell that
//! followed it was painted on the terminal's default background - a one-cell hole
//! down the right edge of every coloured block, on a shaded user message, on a
//! banded code block, and (found while fixing it) on the `▲`/`▼` stepper cells
//! too. `get_active_background_ansi` already existed for exactly this and was
//! simply never called on that path.
//!
//! The fix measures the background on the **source line's prefix** at the bar's
//! column, not on the truncated string, because the truncation's own reset would
//! always answer "no background". That is the same measurement pi makes on its
//! target cell in `replaceScrollbarCell`.
//!
//! # Gap 4, two promises on one key
//!
//! `Home` was bound both to `tui.editor.cursorLineStart` and to
//! `tui.altScreen.top`, and the viewport's matcher ran first, so pressing `Home`
//! mid-prompt teleported the transcript instead of moving the caret. `End` had
//! the same collision with `tui.altScreen.bottom`, and there the viewport matcher
//! won every time because `run.rs` returned early before the editor saw the key.
//!
//! The split is pi's, adopted rather than re-invented: plain `Home`/`End` always
//! belong to the editor, and the viewport's top/bottom live on `Ctrl+Home`/
//! `Ctrl+End`. That removes the ambiguity **by construction** - there is no
//! priority rule to get wrong per keystroke.
//!
//! # What is pinned here, and where the rest lives
//!
//! The behavioural rows sit beside the code they exercise
//! (`chat_viewport_tests.rs` for the cell's SGR, `run_tests.rs` for the key
//! routing), because they need the private frame and input plumbing. This file
//! guards the properties that survive a rewrite of either call site:
//!
//! 1. **The default table has no shared key** between the editor's line-start/end
//!    and the viewport's top/bottom. This is the check the brief asked for, stated
//!    where it is decidable: the loader only reports *user-vs-user* conflicts
//!    (a default may legitimately be contextual), so the shipped defaults are
//!    compared directly.
//! 2. **The scrollbar path consults the extractor.** A future simplification that
//!    went back to `truncate + pad + glyph` would keep the SGR rows green only if
//!    it re-derived the background some other way; this row fails if the call
//!    disappears.
//! 3. **The scrollbar owns only the foreground channel** - the block keeps its
//!    background, so a fix that painted the glyph's colour over the background
//!    would be the mirror bug.
//!
//! Verifies: gh #375 gaps 2 and 4, FR-UI-21 (the fullscreen viewport), NFR-24,
//! and NFR-28 (the thumb is never distinguished by colour alone).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use lca_tui::engine::keybindings::KeybindingsManager;

/// Read one repository file, for a shape guard.
fn read(rel: &str) -> String {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {path:?}: {err}"))
}

/// The body of one function, from its signature to the next line closing it at four
/// spaces of indent.
fn function_body<'a>(source: &'a str, signature: &str) -> &'a str {
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("{signature} exists"));
    let rest = &source[start..];
    let end = rest.find("\n    }\n").map_or(rest.len(), |at| at + 6);
    &rest[..end]
}

/// The frame builder that paints the scrollbar, so its mechanism can be read.
fn chat_render_source() -> String {
    std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/crates/lca-ui/src/chat_render.rs"
    ))
    .expect("chat_render.rs is readable")
}

// Verifies: gh #375 gap 4 - no default key is claimed by both the editor's
// line-start/end actions and the viewport's top/bottom actions. This is the
// "the double-claim warning must not fire" check in the form that can actually
// fail: the manager only reports user-vs-user conflicts, so a shared *default*
// is invisible to it and has to be compared here.
#[test]
fn the_default_table_gives_home_and_end_to_exactly_one_owner() {
    let kb = KeybindingsManager::new();
    let editor: Vec<String> = ["tui.editor.cursorLineStart", "tui.editor.cursorLineEnd"]
        .iter()
        .flat_map(|action| kb.keys(action).to_vec())
        .collect();
    let viewport: Vec<String> = ["tui.altScreen.top", "tui.altScreen.bottom"]
        .iter()
        .flat_map(|action| kb.keys(action).to_vec())
        .collect();

    for key in ["home", "end"] {
        assert!(
            editor.contains(&key.to_string()),
            "plain {key} belongs to the editor: editor={editor:?}"
        );
        assert!(
            !viewport.contains(&key.to_string()),
            "plain {key} must not also be a viewport key: viewport={viewport:?}"
        );
    }
    for key in ["ctrl+home", "ctrl+end"] {
        assert!(
            viewport.contains(&key.to_string()),
            "{key} is the viewport's: viewport={viewport:?}"
        );
        assert!(
            !editor.contains(&key.to_string()),
            "{key} must not also be an editor key: editor={editor:?}"
        );
    }
    assert!(
        kb.conflicts().is_empty(),
        "the shipped defaults raise no double-claim warning: {:?}",
        kb.conflicts()
    );
}

// Verifies: gh #375 gap 2 - the scrollbar cell consults the background extractor.
// Asserted on the source because the alternative is asserting a string that a
// rewrite could reconstruct without the extractor and still look right.
#[test]
fn the_scrollbar_cell_asks_for_the_block_background() {
    let source = chat_render_source();
    let start = source
        .find("if let Some(geometry) = geometry")
        .or_else(|| source.find("let content_width = geometry.column"))
        .unwrap_or_else(|| panic!("the scrollbar build is present in chat_render.rs"));
    let body = &source[start..];
    let end = body.find("// The jump indicator").unwrap_or(body.len());
    let body = &body[..end];

    assert!(
        body.contains("get_active_background_ansi"),
        "the scrollbar cell must keep the block's background through the \
         extractor that already existed for it:\n{body}"
    );
    assert!(
        body.contains("slice_by_column"),
        "the background must be measured on the source line's prefix: the \
         truncation emits its own reset, so measuring the truncated string \
         always answers 'no background' - which is the defect:\n{body}"
    );
    // The reset before the re-applied background: without it, the block's
    // foreground and decorations would reach the glyph.
    assert!(
        body.contains("\\x1b[0m"),
        "the cell resets before re-applying the background, so the block's \
         foreground cannot reach the glyph:\n{body}"
    );
}

// Verifies: gh #375 gap 2 - the scrollbar owns the **foreground** channel and the
// block keeps the background, which is the split that keeps the thumb visible
// without punching the hole. A fix that re-applied a foreground would make the
// thumb disappear into the band, and one that dropped the background is the
// original defect.
#[test]
fn the_scrollbar_keeps_the_background_and_takes_the_foreground() {
    let source = chat_render_source();
    let start = source
        .find("let content_width = geometry.column")
        .unwrap_or_else(|| panic!("the scrollbar build is present"));
    let body = &source[start..];
    let end = body.find("// The jump indicator").unwrap_or(body.len());
    let body = &body[..end];

    // The glyph goes through the theme's *role* style, which is a foreground.
    assert!(
        body.contains("self.theme.role(role)(glyph)"),
        "the glyph is painted through the theme's role style:\n{body}"
    );
    // The background is closed again so the cell cannot leak into the next row.
    assert!(
        body.contains("\\x1b[49m"),
        "the reopened background is closed at the end of the cell:\n{body}"
    );
}

// ---------------------------------------------------------------------------
// gap 3: a jump target is an identity, so a resize cannot move it
// ---------------------------------------------------------------------------
//
// `jump_prompt` stored a **document row** computed at the width and theme in force when
// the key was pressed, and `take_jump_scroll` applied it on a later frame under whatever
// width and theme were in force then. A row is a derived coordinate: resize the pane and
// the wrap changes, switch theme and a row's height changes, and the stored number now
// names a different line.
//
// The behavioural rows live in `crates/lca-ui/src/chat_jump_tests.rs` (and the marker's
// own unit rows in `prompt_marker.rs`). These pin the shape:

// Verifies: gh #375 gap 3 - **the target is an identity, resolved at paint time.**
//
// The one line a future edit could restore is a row stored at keypress. `jump_prompt` must
// store an ordinal, and the row must come from `user_offsets` at the width the frame is
// painting - which is the "resolve late, from a stable key" shape this repo has used for
// #331's `tab_top` and #373's layer stack.
#[test]
fn a_jump_target_is_an_identity_not_a_row() {
    let jump = read("crates/lca-ui/src/chat_jump.rs");
    let body = function_body(&jump, "pub(crate) fn jump_prompt(");
    assert!(
        body.contains("JumpTarget::Prompt("),
        "the prompt jump stores an ordinal: {body}"
    );
    assert!(
        !body.contains("self.jump_target = Some(JumpTarget::Row("),
        "and the target it stores is never a row, which is the bug: {body}"
    );
    let resolve = function_body(&jump, "pub(crate) fn resolve_jump_row(");
    assert!(
        resolve.contains("user_offsets("),
        "and the row is resolved from the offsets at paint time: {resolve}"
    );
    // The offsets themselves must come from the painted rows, not a second measurement
    // pass over the entries (the repo's named "measured one thing, painted another").
    let measure = read("crates/lca-ui/src/transcript/measure.rs");
    let offsets = function_body(&measure, "pub fn user_offsets(");
    assert!(
        offsets.contains("render_for_protocol_at("),
        "the offsets scan the rows the painter will produce: {offsets}"
    );
    assert!(
        !offsets.contains("render_entry("),
        "not a per-entry re-render that can disagree with the frame: {offsets}"
    );
}

// Verifies: gh #375 gap 3 - **the pin survives the clamp**, and records the render epoch.
//
// `take_jump_scroll` clears the request; the pin is what a later frame re-resolves. Two
// things must hold: the clamp must not add the re-wrap's growth on top (which drags the
// prompt off the top row), and the pin must notice a theme change - which moves row
// heights without moving the pane, so geometry alone cannot see it.
#[test]
fn the_pin_outlives_the_request_and_notices_a_rerender() {
    let jump = read("crates/lca-ui/src/chat_jump.rs");
    let take = function_body(&jump, "pub fn take_jump_scroll(");
    assert!(
        take.contains("self.pinned_jump = Some("),
        "taking the request leaves a pin behind: {take}"
    );
    assert!(
        take.contains("epoch: self.transcript.epoch()"),
        "and records the render epoch it resolved under: {take}"
    );
    let render = read("crates/lca-ui/src/chat_render.rs");
    let clamp = function_body(&render, "pub fn clamp_scroll(");
    assert!(
        clamp.contains("pinned_jump"),
        "the clamp knows about a live pin, or the growth-hold moves it: {clamp}"
    );
}

// Verifies: gh #375 gap 3 - **the marker is in-band, exact and stripped before paint.**
//
// In-band so it reflows with its row; exact so no escape bytes reach the screen (#366's
// class one layer up); stripped at the one seam that turns rows into paint, so a copy, a
// scrollback write or a plain-mode frame cannot inherit it.
#[test]
fn the_prompt_marker_is_in_band_and_never_painted() {
    let render = read("crates/lca-ui/src/transcript/render.rs");
    let user = function_body(&render, "fn render_user(");
    assert!(
        user.contains("PROMPT_START"),
        "the user band carries the marker: {user}"
    );

    let chat_render = read("crates/lca-ui/src/chat_render.rs");
    assert!(
        chat_render.contains("strip_markers_in_place"),
        "and the paint seam strips it"
    );

    // The module, not one function body: `function_body` stops at the first four-space
    // closing brace, which here is an inner block. The claims are about the strip path as
    // a whole anyway.
    let marker = read("crates/lca-ui/src/prompt_marker.rs");
    assert!(
        marker.contains("pub(crate) fn strip_markers("),
        "the strip exists"
    );
    assert!(
        marker.contains("BEL") && marker.contains("string-terminator"),
        "and names both terminators it must handle, so the strip is exact"
    );
    let strip = &marker[marker
        .find("pub(crate) fn strip_markers(")
        .expect("the strip is defined")..];
    assert!(
        strip.contains("find('\\x07')") && strip.contains("find(\"\\x1b\\\\\")"),
        "both are located in the implementation"
    );
    assert!(
        strip.contains("malformed"),
        "and a truncated sequence is left in place rather than eating the row"
    );
}
