//! Collapsed tool-card previews, sliced by **visual** lines (gh #391 gap 1).
//!
//! Split from `render.rs` for the 1,200-line ceiling (gate 11).
//!
//! **The defect this exists for.** `render_tool` used to do
//! `result.lines().take(5)` — five *logical* lines — and then wrap them. Two
//! things go wrong with that, and they compound:
//!
//! 1. **The budget is not a budget.** One logical line of minified JSON is
//!    4,000 characters, which wraps into ~50 visual rows. A card that reports
//!    itself as collapsed can therefore overflow the viewport by an order of
//!    magnitude; no `take()` on logical lines can bound visual height.
//! 2. **The interesting content is at the tail.** For `cargo test`, `npm
//!    test`, and every compiler, the first five lines are boilerplate
//!    (`Compiling …`, `Running …`) and the pass/fail summary is at the end.
//!    Keeping the head of a build log hides exactly the line the user opened
//!    the card to see.
//!
//! So: wrap first, then slice visual rows, and for shells keep the **tail**
//! with the omission marker *above* it — where a reader looks for a summary is
//! the bottom. Non-shell tools keep pi's `keep: "start"` shape instead
//! (`ls`, `grep` and friends lead with the useful part), with the marker
//! below.
//!
//! The result is cached per width because this runs on every frame for every
//! collapsed card, and again on every resize.

use lca_tui::engine::text::wrap_text_with_ansi;

/// pi's `BASH_PREVIEW_LINES` for `bash`/`shell`; `ls` 20 and `grep` 15 are
/// pi's own figures (`renderers/ls.ts`, `renderers/grep.ts`). Anything else
/// shows one row.
pub(super) fn preview_lines(name: &str) -> usize {
    match name {
        "bash" | "shell" => 5,
        "list" | "ls" => 20,
        "grep" => 15,
        _ => 0,
    }
}

/// Whether a tool keeps the **tail** of its output when collapsed.
///
/// Shells answer "did it pass?" at the bottom, so they keep the end. Every
/// other previewed tool (`ls`, `grep`) reads top-down, so it keeps the start.
pub(super) fn keeps_tail(name: &str) -> bool {
    matches!(name, "bash" | "shell")
}

/// What a collapsed preview shows, and where its omission marker goes.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Preview {
    /// The visual rows to render, already wrapped to the card's inner width.
    pub(super) rows: Vec<String>,
    /// How many visual rows were dropped.
    pub(super) omitted: usize,
    /// Whether the omission marker belongs **above** the kept rows (the shell
    /// case: the kept rows are a tail, so the marker names what precedes it).
    pub(super) marker_above: bool,
}

/// Strip the LLM-facing truncation footer from a tool's output before it
/// reaches a preview (gh #391 gap 4).
///
/// The tool layer appends `\n\n[Output truncated. Full output: …]` to the
/// content **the model reads**, so it knows where the full log went. The card
/// already renders that fact as its own styled warning line
/// (`render.rs`, the `Muted` truncation notice). Leaving the footer in the
/// preview therefore costs two of the five precious tail rows *and* tells the
/// user the same thing twice — the same family as gh #328's doubled summary.
///
/// The marker is found the way pi finds it: the last `\n\n[` whose tail names
/// a spillover path. A log line that happens to contain `[` is left alone.
pub(super) fn strip_truncation_footer(output: &str) -> &str {
    // LCA's own shape, from `lca-tools`' spill path: the tool appends
    // `\n[full output: N bytes, attachment <hash>]` when it truncated for the
    // model. It is a single newline, not pi's blank-line-prefixed form, and it
    // is what this tree actually writes - matching pi's spelling alone would
    // have left the real footer in place.
    if let Some(at) = output.rfind("\n[full output: ") {
        let tail = &output[at..];
        if tail.ends_with(']') && tail.contains("attachment ") {
            return output[..at].trim_end();
        }
    }
    // pi's shapes, kept for a provider or extension that emits one: the
    // tool layer's spillover notice and the truncation summary. Found the way
    // pi finds it - the last `\n\n[` whose tail names the path - so an
    // ordinary bracketed log line survives.
    let Some(at) = output.rfind("\n\n[") else {
        return output;
    };
    let tail = &output[at..];
    if tail.contains("Full output:") || tail.contains("Output truncated") {
        return output[..at].trim_end();
    }
    output
}

/// Collapse `\r` progress-bar overwrites to their final visual state before
/// wrapping (gh #391, the output-hygiene note in the issue).
///
/// `npm`, `cargo`, and `curl` repaint a progress line with bare `\r`. Wrapped
/// naively, `foo\rbar` becomes the two joined words `foobar`; the terminal
/// would have shown only `bar`. Keeping the last segment per line matches what
/// the user saw the command print.
pub(super) fn collapse_carriage_returns(output: &str) -> String {
    if !output.contains('\r') {
        return output.to_string();
    }
    let mut out = String::with_capacity(output.len());
    for (index, line) in output.split('\n').enumerate() {
        if index > 0 {
            out.push('\n');
        }
        // The final `\r`-segment is what the terminal leaves visible.
        match line.rsplit('\r').next() {
            Some(last) => out.push_str(last),
            None => out.push_str(line),
        }
    }
    out
}

/// Build a collapsed preview for `name`'s output at `width`.
///
/// `inner` is the card's usable width (the caller already subtracted the
/// border and the two-space indent), so wrapping here produces the rows the
/// card will actually paint.
pub(super) fn build_preview(name: &str, output: &str, inner: usize) -> Preview {
    let budget = preview_lines(name);
    if budget == 0 {
        return Preview {
            rows: Vec::new(),
            omitted: 0,
            marker_above: false,
        };
    }
    let cleaned = strip_truncation_footer(output);
    let collapsed = collapse_carriage_returns(cleaned);
    // **Wrap first.** This is the whole defect: slicing before wrapping lets
    // one logical line blow the visual budget.
    let wrapped = wrap_text_with_ansi(&collapsed, inner.max(1));
    // Trailing blank rows are not content: a command that ends with a newline
    // should not spend one of five rows on it, and without this the "tail" of
    // a build log can be four rows of output and one blank.
    let mut visual = wrapped;
    while visual
        .last()
        .is_some_and(|line| line.trim_matches('\u{0}').trim().is_empty())
    {
        visual.pop();
    }
    let omittable = preview_lines(name) > 0 && visual.len() > budget;
    let omitted = visual.len().saturating_sub(budget);
    let tail = keeps_tail(name);
    let rows = if !omittable {
        visual
    } else if tail {
        visual.split_off(visual.len() - budget)
    } else {
        visual.truncate(budget);
        visual
    };
    Preview {
        rows,
        omitted,
        marker_above: tail,
    }
}
