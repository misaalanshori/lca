//! The nested-call checklist inside a parent card (gh #404 gap 1).
//!
//! Split from `render.rs` for the 1,200-line ceiling, and because the rules here
//! are their own thing.
//!
//! **Why a checklist and not child cards.** A `codemode` script that greps
//! twenty times means one intent and twenty actions. Twenty-one cards bury the
//! parent's *result* — the thing the user actually wants — under its own
//! mechanics. pi renders the same shape for the same reason.
//!
//! **Why the tail.** #391 established the rule for a single card: a shell keeps
//! the last rows because that is where the summary is. A checklist is the same
//! argument — what just happened is what the user is watching — so this reuses
//! it rather than inventing a second convention. `COLLAPSED_CHILDREN` is the
//! budget, the marker sits **above** the kept rows, and expanding shows all.

use crate::theme::{Role, Theme};
use crate::transcript::render::render_key_hint;
use lca_tui::engine::text::wrap_text_with_ansi;

/// One nested call, as the parent's checklist draws it (gh #404 gap 1).
///
/// Deliberately not the protocol's [`lca_protocol::record::NestedCallRecord`]:
/// that is the *persisted* shape (a bounded content head, no id), and the UI
/// needs a stable identity for a row that may still be running. The conversion
/// happens where the event arrives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildCall {
    /// What was called.
    pub name: String,
    /// Where it got to.
    pub status: ChildStatus,
    /// The head of its content, for a failed child's indented error.
    pub head: String,
}

/// A nested call's state, which decides its glyph.
///
/// One variant per state the tree already has, so the glyph table is total and a
/// new state cannot be added without deciding what it looks like.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildStatus {
    /// Still running.
    Running,
    /// Finished cleanly.
    Ok,
    /// Failed.
    Error,
    /// Refused by permission or a hook.
    Denied,
    /// Ran out of time.
    Timeout,
}

impl ChildStatus {
    /// The glyph. A **glyph**, not only a colour: NFR-28 requires a non-colour
    /// cue, and pi's own table is these four marks.
    #[must_use]
    pub fn glyph(self) -> char {
        match self {
            ChildStatus::Ok => '✓',
            ChildStatus::Error | ChildStatus::Timeout => '✗',
            ChildStatus::Denied => '⊘',
            ChildStatus::Running => '…',
        }
    }

    /// Whether this state should show its head as an indented error.
    #[must_use]
    pub fn is_failure(self) -> bool {
        matches!(
            self,
            ChildStatus::Error | ChildStatus::Denied | ChildStatus::Timeout
        )
    }

    /// The word a colour-blind reader gets, where the glyph alone is ambiguous.
    #[must_use]
    pub fn word(self) -> &'static str {
        match self {
            ChildStatus::Ok => "ok",
            ChildStatus::Error => "failed",
            ChildStatus::Denied => "denied",
            ChildStatus::Timeout => "timed out",
            ChildStatus::Running => "running",
        }
    }
}

impl From<lca_protocol::ToolResultStatus> for ChildStatus {
    fn from(status: lca_protocol::ToolResultStatus) -> ChildStatus {
        match status {
            lca_protocol::ToolResultStatus::Ok => ChildStatus::Ok,
            lca_protocol::ToolResultStatus::Error => ChildStatus::Error,
            lca_protocol::ToolResultStatus::Denied => ChildStatus::Denied,
            lca_protocol::ToolResultStatus::Timeout => ChildStatus::Timeout,
        }
    }
}

/// How many children a collapsed checklist keeps. pi's figure: the tail is what
/// just happened, and eight fits a card without swallowing the transcript.
pub const COLLAPSED_CHILDREN: usize = 8;

/// How many **visual rows** a failed child's error may occupy when expanded.
///
/// A row budget rather than a character budget, because the wrap decides the
/// characters: a character number is a different height at every width, which is
/// exactly the defect #391 fixed for previews (`preview.rs` carries that story).
/// Two rows orients without becoming a second transcript; the child's full
/// output belongs in its own view.
const CHILD_ERROR_ROWS: usize = 2;

/// Draw the checklist rows for `children` into `out`.
///
/// `expanded` is the card's own state (a click or Ctrl+O), so a checklist
/// expands with the card it lives in - one keystroke, not a second one.
pub(super) fn render_checklist(
    children: &[ChildCall],
    expanded: bool,
    inner: usize,
    theme: &Theme,
    keybindings: &lca_tui::engine::keybindings::KeybindingsManager,
    out: &mut Vec<String>,
) {
    if children.is_empty() {
        return;
    }
    // The tail is what the user is watching; the marker names what precedes it.
    let shown: &[ChildCall] = if expanded || children.len() <= COLLAPSED_CHILDREN {
        children
    } else {
        &children[children.len() - COLLAPSED_CHILDREN..]
    };
    let omitted = children.len() - shown.len();
    if omitted > 0 {
        let key = render_key_hint(theme, keybindings, "app.tools.expand", None);
        let shape = if key.is_empty() {
            format!("… ({omitted} earlier, ctrl+o)")
        } else {
            format!("… ({omitted} earlier, {key})")
        };
        out.push(format!("  {}", (theme.role(Role::Muted))(&shape)));
    }
    for child in shown {
        let glyph = child.status.glyph();
        // Colour by state, but the glyph carries it on its own (NFR-28): the
        // plain theme strips the colour and the row still reads.
        let styled_glyph = match child.status {
            ChildStatus::Ok => (theme.success)(&glyph.to_string()),
            ChildStatus::Error | ChildStatus::Timeout => (theme.error)(&glyph.to_string()),
            ChildStatus::Denied => (theme.warn)(&glyph.to_string()),
            ChildStatus::Running => (theme.dim)(&glyph.to_string()),
        };
        let name = truncate(&child.name, inner.saturating_sub(6));
        out.push(format!(
            "  {styled_glyph} {}",
            (theme.role(Role::ToolOutput))(&name)
        ));
        // A failed child's error, indented under it and bounded. Only when
        // expanded: collapsed, the glyph is the whole message, and eight
        // indented errors would be a second transcript.
        if expanded && child.status.is_failure() {
            let head = child.head.trim();
            if !head.is_empty() {
                // Wrap first, then slice rows - the #391 rule, applied here so
                // the error cannot spend the card's height on a long line.
                let rows = wrap_text_with_ansi(head, inner.saturating_sub(7).max(1));
                let omitted = rows.len().saturating_sub(CHILD_ERROR_ROWS);
                for row in rows.iter().take(CHILD_ERROR_ROWS) {
                    out.push(format!("       {}", (theme.role(Role::Muted))(row)));
                }
                if omitted > 0 {
                    out.push(format!(
                        "       {}",
                        (theme.role(Role::Muted))(&format!("… ({omitted} more lines)"))
                    ));
                }
            }
        }
    }
}

/// Cut `text` to `max` characters, marking the cut.
fn truncate(text: &str, max: usize) -> String {
    let mut out = String::new();
    for (count, ch) in text.chars().enumerate() {
        if count >= max {
            out.push('…');
            return out;
        }
        out.push(ch);
    }
    out
}

/// Whether `head` is boilerplate the **codemode wrapper** produced, for the
/// parent card's own output.
///
/// Narrow on purpose (design point 7). pi strips `Script completed` / `Wall
/// time:` / `Output:` from any codemode parent; LCA strips only these three
/// exact shapes, because a script's own output could begin with any of those
/// words and cutting author output is worse than showing a banner.
///
/// The shapes are the ones `lca-tools`' codemode runner writes:
/// `Script completed` on its own line, `Wall time: <n>s`, and a bare `Output:`
/// label. `Wall time: not a number` is *not* one of them, which is what the row
/// asserting that behaviour pins.
pub(super) fn is_wrapper_boilerplate(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return false;
    }
    if trimmed == "Script completed" || trimmed == "Output:" {
        return true;
    }
    // `Wall time: 1.2s` and `Wall time: 1m 30s`; not `Wall time: not a number`.
    if let Some(rest) = trimmed.strip_prefix("Wall time:") {
        let rest = rest.trim();
        return !rest.is_empty()
            && rest.chars().all(|ch| {
                ch.is_ascii_digit() || ch == '.' || ch == 's' || ch == 'm' || ch == 'h' || ch == ' '
            });
    }
    false
}

/// Drop wrapper boilerplate from a codemode parent's output.
///
/// Leading and trailing blank lines go too, so the card does not open on the
/// gap the stripped line left behind.
pub(super) fn strip_wrapper_boilerplate(output: &str) -> String {
    let kept: Vec<&str> = output
        .lines()
        .filter(|line| !is_wrapper_boilerplate(line))
        .collect();
    kept.join("\n").trim_matches('\n').to_string()
}
