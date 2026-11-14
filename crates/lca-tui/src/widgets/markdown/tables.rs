//! Markdown table parsing and rendering (ceiling split from
//! `mod.rs`): pipe tables as aligned columns. Behavior unchanged.

use super::{MarkdownOptions, MarkdownTheme, render_inline};
use crate::engine::text::{truncate_to_width, visible_width, wrap_text_with_ansi};

pub(super) fn is_table_start(lines: &[&str], i: usize) -> bool {
    if i + 1 >= lines.len() {
        return false;
    }
    let header = lines[i].trim();
    // Streaming tolerance: a table needs a complete header row and an
    // intact separator row; a partial pipe or a half-written separator
    // renders as text until the next chunk completes it.
    header.starts_with('|') && header.ends_with('|') && is_table_separator(lines[i + 1])
}

/// Whether a row is a complete GFM separator: every cell is `---`, `:--`,
/// `--:`, or `:-:`.
pub(super) fn is_table_separator(line: &str) -> bool {
    let t = line.trim();
    if !t.starts_with('|') || !t.ends_with('|') {
        return false;
    }
    let cells = split_table_row(t);
    !cells.is_empty()
        && cells.iter().all(|cell| {
            let cell = cell.trim();
            !cell.is_empty() && cell.contains('-') && cell.chars().all(|c| c == '-' || c == ':')
        })
}

fn split_table_row(line: &str) -> Vec<String> {
    let t = line.trim();
    let t = t.strip_prefix('|').unwrap_or(t);
    let t = t.strip_suffix('|').unwrap_or(t);
    t.split('|').map(|c| c.trim().to_string()).collect()
}

pub(super) fn collect_table(lines: &[&str], start: usize) -> (Vec<Vec<String>>, usize) {
    let mut rows = Vec::new();
    let header = split_table_row(lines[start]);
    rows.push(header);
    let mut i = start + 2; // skip the separator
    while i < lines.len() {
        let t = lines[i].trim();
        // A partial row (no closing pipe) is not yet a table row.
        if !t.starts_with('|') || !t.ends_with('|') {
            break;
        }
        rows.push(split_table_row(lines[i]));
        i += 1;
    }
    (rows, i - start)
}

pub(super) fn render_table(
    rows: &[Vec<String>],
    raw: &[&str],
    width: usize,
    theme: &MarkdownTheme,
    options: &MarkdownOptions,
    out: &mut Vec<String>,
) {
    #[allow(clippy::needless_range_loop)] // columns index parallel width vectors
    if rows.is_empty() {
        return;
    }
    let cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    if cols == 0 {
        return;
    }
    // Border overhead is 3n+1; too narrow to be stable -> pi falls back to
    // the raw markdown source, wrapped.
    if width < cols * 3 + 1 {
        for line in raw {
            out.extend(wrap_text_with_ansi(line, width));
        }
        return;
    }
    // Gh #384: render every cell's inline tokens exactly once, here, and
    // measure *this* - pi pre-renders the cells and measures the rendered
    // form, which is the whole fix: a measurement function must not take a
    // different input than the painter. The same strings are what the paint
    // below wraps, so the two cannot drift apart when the inline renderer
    // changes. Before this, `natural`/`min_word` read raw Markdown, so a
    // `[link](https://…)` counted ~42 columns while painting as 4 and
    // starved every other column to its 1-column minimum.
    //
    // The options are the caller's, not `MarkdownOptions::default()`: the
    // link mode alone changes a cell's painted width between OSC 8 (the
    // label) and the inline fallback (`text (url)`), and measuring a
    // different mode than the one painted would reintroduce the same bug
    // in the other direction.
    let rendered: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            (0..cols)
                .map(|c| {
                    render_inline(row.get(c).map(String::as_str).unwrap_or(""), theme, options)
                })
                .collect()
        })
        .collect();
    let natural: Vec<usize> = (0..cols)
        .map(|c| {
            rendered
                .iter()
                .map(|r| visible_width(r.get(c).map(String::as_str).unwrap_or("")))
                .max()
                .unwrap_or(0)
        })
        .collect();
    let min_word: Vec<usize> = (0..cols)
        .map(|c| {
            rendered
                .iter()
                .flat_map(|r| r.get(c).into_iter().flat_map(|s| s.split_whitespace()))
                .map(visible_width)
                .max()
                .unwrap_or(1)
                .min(30)
        })
        .collect();
    let available = width - (cols * 3 + 1);
    let widths = solve_column_widths(&min_word, &natural, available);

    let border = |left: &str, mid: &str, right: &str| {
        let mut s = String::from(left);
        for (c, w) in widths.iter().enumerate() {
            s.push_str(&"─".repeat(w + 2));
            s.push_str(if c + 1 == cols { right } else { mid });
        }
        (theme.code_block_border)(&s)
    };
    out.push(border("┌", "┬", "┐"));
    for (r, row) in rendered.iter().enumerate() {
        // Wrap each already-rendered cell to its column width, then emit as
        // many lines as the tallest cell needs (pi's `wrapCellText`). A long
        // cell wraps instead of being truncated. Gh #384: this wraps the
        // same string the layout measured, not a fresh render of it.
        let cells: Vec<Vec<String>> = row
            .iter()
            .enumerate()
            .map(|(c, rendered_cell)| {
                let mut lines = wrap_text_with_ansi(rendered_cell, widths[c].max(1));
                // pi's wrapCellText: the narrow styles reset after every
                // non-final fragment, so a style inside a multi-line cell
                // cannot bleed into the next fragment or the padding.
                let fragments = lines.len();
                for line in lines.iter_mut().take(fragments.saturating_sub(1)) {
                    line.push_str("\x1b[22;23;24;25;27;28;29;39m");
                }
                if lines.is_empty() {
                    lines.push(String::new());
                }
                lines
            })
            .collect();
        let height = cells.iter().map(Vec::len).max().unwrap_or(1).max(1);
        for line in 0..height {
            let parts: Vec<String> = cells
                .iter()
                .enumerate()
                .map(|(c, cell)| {
                    pad_or_truncate(cell.get(line).map(String::as_str).unwrap_or(""), widths[c])
                })
                .collect();
            // pi bolds the header cells and only the header cells
            // (`markdown.ts` renderTable: `theme.bold(padded)` per cell) -
            // the border characters stay unstyled.
            let joined = if r == 0 {
                parts
                    .iter()
                    .map(|part| (theme.bold)(part))
                    .collect::<Vec<String>>()
                    .join(" │ ")
            } else {
                parts.join(" │ ")
            };
            out.push(format!("│ {joined} │"));
        }
        // pi draws a separator after every row except the last.
        if r + 1 < rows.len() {
            out.push(border("├", "┼", "┤"));
        }
    }
    out.push(border("└", "┴", "┘"));
}

/// Solve one table's column widths (gh #384 bug 2).
///
/// pi's three rules (`packages/tui/src/components/markdown.ts`, `renderTable`),
/// transcribed with integer arithmetic so the answer cannot depend on the
/// platform's float rounding:
///
/// 1. **Fits naturally** (`sum(natural) <= available`): every column gets its
///    natural width. Unchanged from the round-robin version, byte for byte.
/// 2. **Minima overflow** (`sum(min_word) > available`): start every column at
///    **1** and hand out `available - cols` by **weight** `min_word[c] - 1`,
///    because the space a column cannot do without is exactly what
///    `min_word` measures. Round-robin shrink here took one column from every
///    column in turn, so a 2-character id lost as much as the prose column
///    beside it.
/// 3. **Between the minima and the natural widths**: distribute
///    `available - sum(minima)` by **grow potential** `natural[c] - minima[c]`,
///    so a column with little left to gain stops early and the prose column
///    keeps taking space. Round-robin growth gave a 2-char id the same `+1`
///    per pass as a 100-char description until the id hit its natural width.
///
/// Both distributions use the **largest-remainder** method: every column gets
/// `floor(share)`, then the columns with the largest fractional remainders take
/// the leftovers one at a time, **ties broken by column index**. That is what
/// makes the layout deterministic - a pure `while remaining > 0 { … }` sweep is
/// iteration-ordered, and a hash-ordered one would lay the same table out
/// differently between runs.
///
/// Returns one width per column, always >= 1. When `available >= cols` the sum
/// is exactly `available`; when the terminal is too narrow for one cell per
/// column the sum is `cols` (every column at 1), because a width below 1 is not
/// a narrower column, it is a missing one.
pub(super) fn solve_column_widths(
    min_word: &[usize],
    natural: &[usize],
    available: usize,
) -> Vec<usize> {
    let cols = natural.len().min(min_word.len());
    let mut widths: Vec<usize> = natural[..cols].to_vec();
    if widths.iter().sum::<usize>() <= available {
        return widths;
    }
    let floor = min_word[..cols].to_vec();
    if floor.iter().sum::<usize>() <= available {
        // Rule 3: grow from the minima by potential.
        let extra = available - floor.iter().sum::<usize>();
        let weights: Vec<usize> = (0..cols)
            .map(|c| natural[c].saturating_sub(floor[c]))
            .collect();
        return distribute(floor, &weights, extra, &natural[..cols]);
    }
    // Rule 2: shrink to 1 each and share what is left by min-word weight.
    let mut base = vec![1usize; cols];
    let remaining = available.saturating_sub(cols);
    if remaining > 0 {
        let weights: Vec<usize> = floor.iter().map(|w| w.saturating_sub(1)).collect();
        base = distribute(base, &weights, remaining, &natural[..cols]);
        // The weight sweep may have handed a column more than it needs; give it
        // back to whoever still wants it, and if nobody does the table simply
        // renders narrower than the budget rather than wider.
        for c in 0..cols {
            base[c] = base[c].min(natural[c].max(1));
        }
    }
    widths = base;
    widths
}

/// Largest-remainder distribution of `budget` over `weights`, added to `base`.
///
/// `floor(share)` per column, then the leftovers to the largest remainders,
/// tie-broken by column index. A column is never pushed past its `cap`; space
/// that cannot be placed anywhere stays unplaced, which is honest (the frame is
/// narrower than the budget) rather than borrowed from a neighbour.
fn distribute(mut base: Vec<usize>, weights: &[usize], budget: usize, cap: &[usize]) -> Vec<usize> {
    let cols = base.len();
    if budget == 0 || cols == 0 {
        return base;
    }
    let total: usize = weights.iter().sum();
    if total == 0 {
        // No column has weight: the budget cannot be justified anywhere, so it
        // stays unspent. (Round-robin would have spent it on column 0 first.)
        return base;
    }
    let mut leftover = budget;
    let mut remainders: Vec<(usize, usize)> = Vec::with_capacity(cols);
    for c in 0..cols {
        let product = budget.saturating_mul(weights[c]);
        let share = product / total;
        let room = cap[c].saturating_sub(base[c]);
        let placed = share.min(room);
        base[c] += placed;
        leftover -= placed;
        remainders.push((product % total, c));
    }
    // Largest remainder first, column index ascending on a tie: deterministic by
    // construction, not by iteration order.
    remainders.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    for (_, c) in remainders {
        if leftover == 0 {
            break;
        }
        if base[c] < cap[c] {
            base[c] += 1;
            leftover -= 1;
        }
    }
    base
}

fn pad_or_truncate(text: &str, width: usize) -> String {
    let w = visible_width(text);
    if w > width {
        truncate_to_width(text, width, "…", false)
    } else {
        format!("{text}{}", " ".repeat(width - w))
    }
}
