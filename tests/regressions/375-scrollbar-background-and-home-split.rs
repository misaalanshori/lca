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
