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
    let natural: Vec<usize> = (0..cols)
        .map(|c| {
            rows.iter()
                .map(|r| visible_width(r.get(c).map(String::as_str).unwrap_or("")))
                .max()
                .unwrap_or(0)
        })
        .collect();
    let min_word: Vec<usize> = (0..cols)
        .map(|c| {
            rows.iter()
                .flat_map(|r| r.get(c).into_iter().flat_map(|s| s.split_whitespace()))
                .map(visible_width)
                .max()
                .unwrap_or(1)
                .min(30)
        })
        .collect();
    let available = width - (cols * 3 + 1);
    let mut widths = natural.clone();
    let total: usize = natural.iter().sum();
    if total > available {
        // Start at min-word, distribute the remainder by grow potential.
        widths = min_word.clone();
        let mut used: usize = widths.iter().sum();
        let mut fixup = 0;
        while used < available && fixup < 1000 {
            let mut progressed = false;
            for c in 0..cols {
                if widths[c] < natural[c] && used < available {
                    widths[c] += 1;
                    used += 1;
                    progressed = true;
                }
            }
            fixup += 1;
            if !progressed {
                break;
            }
        }
        // If min-words alone overflow, shrink proportionally.
        if used > available {
            let mut over = used - available;
            while over > 0 {
                let mut progressed = false;
                for w in widths.iter_mut() {
                    if over == 0 {
                        break;
                    }
                    if *w > 1 {
                        *w -= 1;
                        over -= 1;
                        progressed = true;
                    }
                }
                if !progressed {
                    break;
                }
            }
        }
    }

    let border = |left: &str, mid: &str, right: &str| {
        let mut s = String::from(left);
        for (c, w) in widths.iter().enumerate() {
            s.push_str(&"─".repeat(w + 2));
            s.push_str(if c + 1 == cols { right } else { mid });
        }
        (theme.code_block_border)(&s)
    };
    out.push(border("┌", "┬", "┐"));
    for (r, row) in rows.iter().enumerate() {
        // Wrap each cell to its column width, then emit as many lines as the
        // tallest cell needs (pi's `wrapCellText`). A long cell wraps instead
        // of being truncated.
        let cells: Vec<Vec<String>> = (0..cols)
            .map(|c| {
                let raw = row.get(c).map(String::as_str).unwrap_or("");
                let rendered = render_inline(raw, theme, &MarkdownOptions::default());
                let mut lines = wrap_text_with_ansi(&rendered, widths[c].max(1));
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

fn pad_or_truncate(text: &str, width: usize) -> String {
    let w = visible_width(text);
    if w > width {
        truncate_to_width(text, width, "…", false)
    } else {
        format!("{text}{}", " ".repeat(width - w))
    }
}
