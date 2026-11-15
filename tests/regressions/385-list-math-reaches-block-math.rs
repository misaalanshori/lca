//! NFR-24 guard for gh #385: a list item's display math reaches `block_math`.
//!
//! The behavioural rows live in `crates/lca-tui/src/widgets/markdown/list_math_tests.rs`
//! (fourteen of them: bullet, ordered, run, boundary, nested, quote-nested, pending,
//! malformed, over-wide, inline guards, mid-line boundary, plain path). This file
//! is the structural half, because a behavioural row can pass while the routing is
//! a coincidence of its fixture: `- $$…$$` would also type-set if the list path
//! happened to hand the line to `render_blocks` for an unrelated reason, and it
//! would stop type-setting if the routing moved out of the item branch.
//!
//! One claim, one assertion: **the list-item branch of `render_blocks` calls the
//! display-math router before it calls `render_inline`**. Read through the module
//! walk (`module_sources`), so the ceiling split gh #385 needed - `list_math.rs`
//! beside `mod.rs` - does not make the guard vacuous (the gh #574 lesson).
//!
//! Verifies: gh #385 ("`mod.rs::render_blocks` recurses into blockquotes ... but a
//! list item calls `render_inline` on one line").

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use crate::code_lines;

/// Capture the source of the enclosing `fn`/`if` block, by brace balance, starting
/// at the first occurrence of `start`.
fn block_from(source: &str, start: &str) -> String {
    let at = source
        .find(start)
        .unwrap_or_else(|| panic!("`{start}` exists in the markdown module"));
    let mut depth = 0i32;
    let mut out = String::new();
    for ch in source[at..].chars() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                out.push(ch);
                if depth == 0 {
                    break;
                }
                continue;
            }
            _ => {}
        }
        out.push(ch);
    }
    out
}

// Verifies: gh #385 - the item branch routes display math through the block
// detector **before** the inline renderer, which is the whole change.
//
// The assertion is positional on purpose: presence alone would let a future
// edit call `render_inline` first and then paint the block's rows on top, which
// is the double-render shape (`- $$x$$` printing the raw source *and* the
// typeset rows). The order is what makes the branch a branch.
#[test]
fn the_list_item_branch_routes_display_math_before_rendering_inline() {
    let text = code_lines(&crate::module_text(
        "crates/lca-tui/src/widgets/markdown/mod.rs",
    ));
    let item_branch = block_from(
        &text,
        "if let Some((kind, content, indent)) = list_item(trimmed)",
    );
    let router = item_branch
        .find("list_math::render_item")
        .unwrap_or_else(|| {
            panic!("the list-item branch calls the display-math router: {item_branch}")
        });
    let inline = item_branch
        .find("render_inline(&content")
        .unwrap_or_else(|| {
            panic!("the list-item branch still renders inline after the router: {item_branch}")
        });
    assert!(
        router < inline,
        "the display-math router runs before the inline fallback: {item_branch}"
    );
}

// Verifies: gh #385, the other half of the routing claim - the router asks
// `block_math`, not "does the line contain `$$`".
//
// That distinction is pi's block/inline split (`markdown.ts:655` handles the
// inline `latex` token; only a block-start reaches `latexBlock`). A substring test
// would route `- $x^2$` into the 2D layout engine and stack an inline fraction -
// which is what the behavioural row
// `an_inline_dollar_run_in_a_list_item_keeps_the_inline_path` measures. Asserted
// on the source because the *classification* is the contract a future edit would
// loosen, not the fixtures a row happens to carry.
#[test]
fn the_router_classifies_with_block_math_rather_than_a_substring() {
    let text = code_lines(&crate::module_text(
        "crates/lca-tui/src/widgets/markdown/mod.rs",
    ));
    let body = block_from(&text, "pub(super) fn render_item(");
    assert!(
        body.contains("math::block_math"),
        "the router delegates the classification to `block_math`: {body}"
    );
}
