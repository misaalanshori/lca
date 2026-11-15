//! Gh #385: display math inside a list item.
//!
//! The line-based tokenizer rendered an item's content with `render_inline`, so
//! `$$…$$` / `\[…\]` inside `- item` never reached [`math::block_math`] and
//! printed flattened (`x = a/b`) while the identical content inside a
//! blockquote typeset — pi routes a `latexBlock` through the block path in both
//! contexts (`markdown.ts`'s `latexBlock` case, reached from `renderList` because
//! an item's children are tokens, not a string).
//!
//! This module adds exactly that branch. The list machinery (markers,
//! renumbering, continuation indent, nesting) is untouched: an item whose body
//! contains a display-math block renders its body as blocks and prefixes the rows;
//! every other item keeps `render_inline` byte for byte. The test is "does
//! `block_math` answer" rather than "does the line contain `$$`" because that is
//! pi's own shape: `- note: $$x$$` is an *inline* token (`markdown.ts:655`) and
//! keeps rendering as inline math.

use super::{MarkdownOptions, MarkdownTheme, math, render_blocks};
use crate::engine::text::{visible_width, wrap_text_with_ansi};

/// The item being rendered: its source lines, where it starts, and the geometry
/// the prefix needs (bundled so the function takes a context rather than six
/// loose numbers that are easy to pass in the wrong order).
pub(super) struct Item<'a> {
    /// The whole block sequence the item lives in.
    pub(super) lines: &'a [&'a str],
    /// The item's own line index.
    pub(super) at: usize,
    /// The item's content, already stripped of its marker.
    pub(super) content: &'a str,
    /// The column the content starts at: continuation lines dedent to it.
    pub(super) content_col: usize,
    /// The first row's prefix (indent + marker).
    pub(super) prefix: &'a str,
    /// The item's available width (terminal width minus the prefix).
    pub(super) width: usize,
}

/// Render an item's body as blocks when it contains a display-math block, or
/// `None` when it does not — the caller then keeps today's inline path.
///
/// Rows come back already prefixed: the first with `item.prefix` (the marker),
/// the rest with the same width of spaces, which is pi's
/// `firstPrefix`/`continuationPrefix` pair. The second return value is how many
/// source lines *beyond* the item's own line the body consumed, so the caller's
/// cursor lands on the next item.
pub(super) fn render_item(
    item: &Item<'_>,
    theme: &MarkdownTheme,
    options: &MarkdownOptions,
) -> Option<(Vec<String>, usize)> {
    let Item {
        lines,
        at,
        content,
        content_col,
        prefix,
        width,
    } = *item;
    // The item's body: its continuation lines dedented back to the content
    // column (pi hands the item's token body, not the raw source, to the block
    // renderer, so an item's own indentation is not the block's).
    let mut body: Vec<&str> = vec![content];
    let mut j = at + 1;
    while j < lines.len() {
        let line = lines[j];
        // A blank line ends the body, and so does a sibling or nested item:
        // `- $$…$$` followed by `- second` must keep `second` as an item
        // (measured on pi), never as part of the first item's source.
        if line.trim().is_empty() || leading(line) < content_col || super::list_item(line).is_some()
        {
            break;
        }
        body.push(line.get(content_col..).unwrap_or("").trim_end());
        j += 1;
    }

    let width = width.max(1);
    let mut rows: Vec<String> = Vec::new();
    let mut at_body = 0;
    let mut found = false;
    while at_body < body.len() {
        match math::block_math(&body, at_body) {
            Some(block) => {
                found = true;
                at_body += block.consumed;
                rows.extend(block_rows(
                    &body,
                    at_body - block.consumed,
                    &block,
                    width,
                    options,
                ));
            }
            None => {
                // A run of ordinary lines, up to the next block boundary: it
                // renders as blocks too, which is what makes `- note:` a
                // paragraph the bullet can prefix rather than inline text.
                let start = at_body;
                while at_body < body.len() && math::block_math(&body, at_body).is_none() {
                    at_body += 1;
                }
                let mut run = Vec::new();
                render_blocks(&body[start..at_body], width, theme, options, &mut run);
                rows.extend(run);
            }
        }
    }
    if !found {
        return None;
    }

    // pi drops the trailing blank an inner block adds before its own spacing.
    while rows.last().is_some_and(|row| row.is_empty()) {
        rows.pop();
    }
    let pad = " ".repeat(visible_width(prefix));
    let prefixed = rows
        .into_iter()
        .enumerate()
        .map(|(k, row)| {
            if k == 0 {
                format!("{prefix}{row}")
            } else if row.is_empty() {
                String::new()
            } else {
                format!("{pad}{row}")
            }
        })
        .collect();
    Some((prefixed, j - at - 1))
}

/// One display-math block's rows, or the raw-source fallback when it cannot be
/// typeset (pending, disabled, malformed) or does not fit.
///
/// **Width honesty, a deliberate divergence from pi.** pi wraps the typeset rows
/// like any other line, which splits a 2D block mid-cell — measured at 20 columns,
/// a six-column matrix prints `- 11111111 │` then `  22222222 │`, a broken cell.
/// The acceptance criterion prefers a readable fallback, so a block wider than the
/// item's content width prints its raw source wrapped instead, the same rule
/// tables use for a narrow terminal.
fn block_rows(
    body: &[&str],
    at: usize,
    block: &math::BlockMath,
    width: usize,
    options: &MarkdownOptions,
) -> Vec<String> {
    if !block.pending
        && options.render_latex
        && let Some(rendered) = crate::widgets::latex::render_latex(&block.content, true)
    {
        let rows: Vec<String> = rendered.split('\n').map(str::to_string).collect();
        if rows.iter().all(|row| visible_width(row) <= width) {
            return rows;
        }
    }
    body[at..at + block.consumed]
        .iter()
        .flat_map(|line| wrap_text_with_ansi(line, width))
        .collect()
}

fn leading(line: &str) -> usize {
    line.len() - line.trim_start().len()
}
