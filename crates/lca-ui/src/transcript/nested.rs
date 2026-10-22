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

use super::{Entry, ToolStatus, Transcript};
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

impl Transcript {
    /// Record a tool call that carries a parent link (gh #404 gap 1).
    ///
    /// The link is kept so an **orphan** - a child whose parent card is absent -
    /// is still drawn as a top-level card. Dropping it would make a broken
    /// linkage silently hide a call, which is worse than the flood this cycle
    /// fixes.
    pub fn start_tool_with_parent(
        &mut self,
        name: impl Into<String>,
        args: impl Into<String>,
        parent_link: Option<String>,
    ) {
        self.start_tool_with_id(name, args, parent_link, None);
    }

    /// [`Transcript::start_tool_with_parent`], also carrying the call's own id.
    ///
    /// The id is what makes the suppression exact. A nested call produces a
    /// `ToolStarted` *and* a `ToolFinished` (the turn emits both, so a headless
    /// observer sees the pair); this transcript answers the first by adding a
    /// checklist row and remembering the id, and the second by settling that row
    /// instead of creating a card. Matching on "the most recent running card"
    /// would be wrong: the parent is still running.
    pub fn start_tool_with_id(
        &mut self,
        name: impl Into<String>,
        args: impl Into<String>,
        parent_link: Option<String>,
        call_id: Option<String>,
    ) {
        let name = name.into();
        // gh #404 gap 1: a child whose parent card is present is a checklist
        // row, not a card. The row starts in `Running` and its `ToolFinished`
        // settles it (see `finish_tool_with_children`). A child whose parent is
        // **absent** falls through to a card of its own - the orphan fallback,
        // because hiding a call entirely is worse than the flood.
        if let Some(parent) = parent_link.clone()
            && !parent.is_empty()
            && self.entries.iter().any(
                |entry| matches!(entry, Entry::Tool { call_id: Some(id), .. } if *id == parent),
            )
        {
            self.invalidate_cache();
            for entry in self.entries.iter_mut().rev() {
                if let Entry::Tool {
                    call_id: Some(id),
                    children,
                    ..
                } = entry
                    && *id == parent
                {
                    children.push(ChildCall {
                        name: name.clone(),
                        status: ChildStatus::Running,
                        head: String::new(),
                    });
                    break;
                }
            }
            // Remember the child's id so its finish settles this row rather than
            // creating a card. The verdict rides in `suppressed`.
            self.suppressed
                .insert(call_id.filter(|id| !id.is_empty()), name);
            return;
        }
        self.entries.push(Entry::Tool {
            name,
            args: args.into(),
            status: ToolStatus::Running,
            result: None,
            diff: None,
            manual: false,
            expanded: None,
            duration_ms: None,
            started_at: Some(std::time::Instant::now()),
            excluded: false,
            children: Vec::new(),
            parent_link,
            call_id,
        });
    }

    /// Attach children to the most recent running card (gh #404 gap 1).
    ///
    /// Used by the live path while a parent is still running - its children land
    /// one at a time as each nested call finishes.
    pub fn push_children(&mut self, children: Vec<ChildCall>) {
        self.invalidate_last();
        for entry in self.entries.iter_mut().rev() {
            if let Entry::Tool { children: slot, .. } = entry {
                slot.extend(children);
                return;
            }
        }
    }

    /// Finish the most recent running tool call, carrying the nested calls it
    /// made (gh #404 gap 1).
    ///
    /// A separate method rather than a wider `finish_tool_full`: the nested set
    /// is only ever attached from the one place that has it (`ToolResult.nested`
    /// on the turn event), and a caller that has no children should not have to
    /// pass `Vec::new()`.
    pub fn finish_tool_with_children(
        &mut self,
        status: ToolStatus,
        result: Option<String>,
        diff: Option<String>,
        duration_ms: Option<u64>,
        children: Vec<ChildCall>,
    ) {
        self.finish_tool_with_id(status, result, diff, duration_ms, children, None);
    }

    /// [`Transcript::finish_tool_with_children`] for a call whose id is known.
    ///
    /// gh #404 gap 1: a **suppressed** child's finish settles the checklist row
    /// its start created, rather than creating or settling a card. Without the
    /// id the finish would look like an ordinary result and land on the parent -
    /// which is still running, so it would be the one amended.
    pub fn finish_tool_with_id(
        &mut self,
        status: ToolStatus,
        result: Option<String>,
        diff: Option<String>,
        duration_ms: Option<u64>,
        children: Vec<ChildCall>,
        call_id: Option<String>,
    ) {
        self.invalidate_cache();
        // gh #404 gap 1: when the card already collected live child rows, the
        // persisted set is the same rows by another route - keep the live ones.
        let has_live_rows = self
            .entries
            .iter()
            .rev()
            .find_map(|entry| match entry {
                Entry::Tool { children, .. } => Some(!children.is_empty()),
                Entry::Assistant { .. } | Entry::User(_) => None,
                _ => None,
            })
            .unwrap_or(false);
        let children = if has_live_rows { Vec::new() } else { children };
        // gh #404 gap 1: settle a suppressed child's checklist row.
        if let Some(name) = self.suppressed.remove(&call_id) {
            let child_status: ChildStatus = match status {
                ToolStatus::Running => ChildStatus::Running,
                ToolStatus::Ok => ChildStatus::Ok,
                ToolStatus::Error => ChildStatus::Error,
                ToolStatus::Denied => ChildStatus::Denied,
                ToolStatus::Timeout => ChildStatus::Timeout,
            };
            let head = result.unwrap_or_default();
            for entry in self.entries.iter_mut().rev() {
                if let Entry::Tool { children, .. } = entry
                    && let Some(row) = children
                        .iter_mut()
                        .rev()
                        .find(|row| row.name == name && row.status == ChildStatus::Running)
                {
                    row.status = child_status;
                    row.head = head;
                    return;
                }
            }
            return;
        }
        for entry in self.entries.iter_mut().rev() {
            if let Entry::Tool {
                status: s,
                result: r,
                diff: d,
                duration_ms: ms,
                children: kids,
                ..
            } = entry
                && *s == ToolStatus::Running
            {
                *s = status;
                *r = result;
                *d = diff;
                *ms = duration_ms;
                kids.extend(children);
                return;
            }
        }
    }
}
