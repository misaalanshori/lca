//! The startup header's three lines (gh #399): the logo beside the version, the
//! key hints, and the nudge.
//!
//! # The composition, and where it came from
//!
//! The owner supplied it (`pi-opencode_uiux-study/owner_notes.md`):
//!
//! ```text
//! █ █▀▀ █▀▄ v0.6.1 (abi 0.6)
//! █ █▄▄ █▀█ esc interrupt · ctrl+c/ctrl+d clear/exit · / commands · ! bash · ctrl+o more
//! ▀▀▀▀▀▀▀▀▀ Press ctrl+o to show full startup help and loaded resources.
//! LCA can explain its own features and look up its docs.
//! Ask it how to use or extend LCA.
//! ```
//!
//! So: the mark's three rows sit **left of** the three text lines, one text line
//! per mark row. Row 1 carries the version, row 2 the hints, row 3 the nudge. That
//! is the same shape as pi's `BuiltInHeader` (`interactive-mode.ts:1022-1032`),
//! where the logo's first line carries the version and its second the first line of
//! hints - the third is LCA's, because the owner asked for the nudge to ride there.
//!
//! # Every key comes from the registry
//!
//! #410's lesson: a hard-coded `ctrl+c` is a hint that can lie. Nothing here spells
//! a key. [`hints`] asks [`KeybindingsManager::display_keys`] for each action, so a
//! `/reload` rebind reaches the first thing a user reads, and an action that has
//! been unbound is **omitted** rather than rendered as a blank key press. `/` and
//! `!` are literal characters, not bindings - they are the editor's own prefixes -
//! so they are passed through as text and still belong on the line.
//!
//! # Three rungs, and why the middle one exists
//!
//! The owner's hint line is 79 columns; beside the nine-column mark and its space
//! that asks for 89. Below that, this header does not wrap it - it drops to the
//! wordmark and the hints on their own rows, and below that prints the mark with
//! just the version. A wrapped header reads as a rendering fault, which is exactly
//! what the brief forbids.
//!
//! The rungs are chosen from the **longest row of the block that would be printed**
//! at that width, not from a constant: a rebind that shortens every hint line makes
//! the full composition fit where it before did not, and the header should follow
//! that rather than a number someone typed.

use super::logo::{Mark, mark_for, mark_rows, mark_width};
use super::theme::Theme;
use lca_tui::engine::keybindings::KeybindingsManager;
use lca_tui::engine::text::visible_width;

/// The wordmark's width, which is the middle rung's cost.
const WORDMARK_WIDTH: usize = 3;

/// The product identity the header prints: what `lca --version` reports, from the
/// same source, so the two can never disagree.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VersionInfo {
    /// The product version (`0.6.1`, or `0.6.1.b<sha>` on the unstable line).
    pub product: String,
    /// The ABI line (`0.6`).
    pub abi: String,
}

impl VersionInfo {
    /// Build one from the two strings `lca --version` and the CLI already hold.
    pub fn new(product: impl Into<String>, abi: impl Into<String>) -> VersionInfo {
        VersionInfo {
            product: product.into(),
            abi: abi.into(),
        }
    }

    /// `v0.6.1 (abi 0.6)`.
    pub fn label(&self) -> String {
        format!("v{} (abi {})", self.product, self.abi)
    }
}

/// One hint on the line: the keys to press and what they do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hint {
    /// The key text, already resolved from live bindings (`ctrl+c/ctrl+d`).
    pub keys: String,
    /// What the keys do.
    pub description: String,
}

/// The hint line, every key resolved from `keybindings` (gh #410).
///
/// The set is the owner's five. Three of them are actions and come from the
/// registry; `app.exit` is *two presses of `app.clear`*, which is what the binding's
/// own description says and what the interface does - so it is derived from the same
/// key rather than naming `ctrl+d`, which nothing binds. `/` and `!` are the
/// editor's literal prefixes.
///
/// An action that resolves to no keys is skipped, so an unbind shortens the line
/// rather than printing ` interrupt`.
pub fn hints(keybindings: &KeybindingsManager) -> Vec<Hint> {
    let mut out = Vec::new();
    let mut push = |keys: String, description: &str| {
        if !keys.trim().is_empty() {
            out.push(Hint {
                keys,
                description: description.to_string(),
            });
        }
    };
    push(keybindings.display_keys("app.interrupt"), "interrupt");
    let clear = keybindings.display_keys("app.clear");
    if !clear.trim().is_empty() {
        // `app.clear`'s description is "Clear the input, twice to exit", so the
        // second press is what exits. Derived, so a rebind moves both halves.
        push(format!("{clear} {clear}"), "clear/exit");
    }
    push("/".to_string(), "commands");
    push("!".to_string(), "bash");
    push(keybindings.display_keys("app.tools.expand"), "more");
    out
}

/// The hint line as one string: `esc interrupt · ctrl+c ctrl+c clear/exit · …`.
pub fn hint_line(keybindings: &KeybindingsManager) -> String {
    hints(keybindings)
        .iter()
        .map(|hint| format!("{} {}", hint.keys, hint.description))
        .collect::<Vec<_>>()
        .join(" · ")
}

/// `Press <expand> to show full startup help.` Empty when the action is unbound, so
/// the nudge never names a key that does nothing.
pub fn press_hint(keybindings: &KeybindingsManager) -> String {
    let key = keybindings.display_keys("app.tools.expand");
    if key.trim().is_empty() {
        return String::new();
    }
    format!("Press {key} to show full startup help.")
}

/// The onboarding nudge: LCA is its own help system (`owner_notes.md`).
pub fn nudge() -> String {
    "LCA can explain its own features and look up its docs. Ask it how to use or extend LCA."
        .to_string()
}

/// Where the header's rows are placed on the three-line block.
///
/// The mark's rows are paired with the text lines **in order**, so the design
/// survives a mark that is not three rows tall (the wordmark is one, and a
/// future glyph set could be two).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Layout {
    /// Mark left of all three text lines: the owner's composition.
    Beside,
    /// Wordmark, then the three text lines beneath it.
    Stacked,
    /// Too narrow for the text rows at all: mark and version only.
    VersionOnly,
}

/// The block the header prints, already themed.
///
/// Pure apart from reading `keybindings` and `theme`, so every rung is a row on a
/// fake width rather than something only a real terminal can show.
pub fn compose(
    theme: &Theme,
    keybindings: &KeybindingsManager,
    version: &VersionInfo,
    width: usize,
    apple_terminal: bool,
) -> Vec<String> {
    let hint = hint_line(keybindings);
    let press = press_hint(keybindings);

    // The text lines, in the order the owner drew them. Row 3 carries both the
    // expand nudge and the onboarding line, as the sketch does.
    let mut text = vec![version.label()];
    if !hint.is_empty() {
        text.push(hint);
    }
    let mut third = Vec::new();
    if !press.is_empty() {
        third.push(press);
    }
    third.push(nudge());
    text.push(third.join(" "));

    let layout = layout_for(width, apple_terminal, &text);
    // The mark is re-asked for the chosen layout, because the beside rung has a
    // wider floor than the stacked one and Apple_Terminal never takes half blocks.
    let mark = match layout {
        Layout::Beside => mark_for(width, apple_terminal),
        _ if width >= WORDMARK_WIDTH => Mark::Wordmark,
        _ => Mark::None,
    };
    let lines = if matches!(layout, Layout::VersionOnly) {
        vec![text[0].clone()]
    } else {
        text
    };

    match layout {
        Layout::Beside => beside(theme, mark, &lines),
        Layout::Stacked => {
            let mut out = mark_rows(theme, mark);
            out.extend(lines.iter().map(|line| paint_text(theme, line)));
            out
        }
        Layout::VersionOnly => {
            let mut out = mark_rows(theme, mark);
            out.extend(lines.iter().map(|line| paint_text(theme, line)));
            out
        }
    }
}

/// The owner's composition: the mark's rows in a left column, the text beside it.
///
/// The column is measured from the rows actually printed, not from the grid's
/// width: the wordmark is three cells where the half-block mark is nine, and padding
/// by the grid would leave six dead columns between the fallback and the version.
fn beside(theme: &Theme, mark: Mark, lines: &[String]) -> Vec<String> {
    let rows = mark_rows(theme, mark);
    let column = rows.iter().map(|row| visible_width(row)).max().unwrap_or(0);
    lines
        .iter()
        .enumerate()
        .map(|(index, line)| {
            // A shorter mark still reserves the column, so the text stays in one
            // block instead of stepping left on the last row.
            let left = rows.get(index).cloned().unwrap_or_default();
            let gap = " ".repeat(column.saturating_sub(visible_width(&left)) + 1);
            format!("{left}{gap}{}", paint_text(theme, line))
        })
        .collect()
}

/// Which rung fits `width`, given the rows that rung would print.
///
/// The beside rung is measured against the widest row it produces, so a rebind that
/// shortens the hints can promote the full composition back - the threshold is a
/// measurement, not a constant someone typed.
///
/// The lower rungs check the **version** row rather than the hint row on purpose: a
/// hint line that wraps in a 40-column terminal is normal text wrapping, while a
/// truncated version would be a lie about which build is running. So the ladder's
/// floor is "can the version be shown whole", and below that still prints it whole.
fn layout_for(width: usize, apple_terminal: bool, text: &[String]) -> Layout {
    let beside_mark = if apple_terminal {
        WORDMARK_WIDTH
    } else {
        mark_width()
    };
    let widest = text
        .iter()
        .map(|line| visible_width(line))
        .max()
        .unwrap_or(0);
    // The `+1` is the gap column beside the mark.
    if width > widest + beside_mark {
        return Layout::Beside;
    }
    let version_width = visible_width(&text[0]);
    if width >= version_width.max(WORDMARK_WIDTH) {
        return Layout::Stacked;
    }
    Layout::VersionOnly
}

/// One text line painted the way pi prints its header: `theme.fg("dim", …)`.
///
/// `theme.dim` is the accessor with the dim *attribute* on it; `role(Role::Dim)`
/// carries only the colour, which is the subtle difference a row caught (the
/// header receded by hue but not by weight). The whole block is dim so it sits
/// behind the conversation, exactly as pi's does. The mark keeps its own colours
/// regardless - see [`super::logo`], which never reads a role.
fn paint_text(theme: &Theme, line: &str) -> String {
    (theme.dim)(line)
}

#[cfg(test)]
#[path = "startup_header_tests.rs"]
mod tests;
