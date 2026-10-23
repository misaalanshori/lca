//! The compaction card: a summary collapses to one self-describing line (gh #393).
//!
//! Split from `render.rs` for the 1,200-line ceiling, and because the card's
//! rules are their own thing.
//!
//! **The defect this exists for.** A compaction summary was pushed as
//! `Entry::Raw("[compaction] {summary} (via {strategy})")` - fully expanded,
//! always. A long session compacts several times, and each summary is a wall of
//! prose the user has already read (it summarises what they watched happen). By
//! the third compaction the transcript is mostly summary, which is the opposite of
//! what a transcript is for.
//!
//! # Collapsed is not gone
//!
//! The card names itself, the strategy, the record range it replaced, and the
//! counts. A user who never expands still learns that their context was
//! summarised and roughly how much was lost. Hiding the *existence* of a
//! compaction would be a worse bug than the wall: the reader would have no way to
//! know their context had been rewritten.
//!
//! # The collapse is the existing one, not a third mechanism
//!
//! The repo already had two solutions in this area. Neither fits:
//!
//! - `preview.rs` (gh #391) slices **tool output** to a row budget, keeping a head
//!   or a tail. It answers "which rows of this blob survive", not "does this entry
//!   show its body" - and a compaction card's body is prose, not a log tail.
//! - `nested.rs` (gh #404) collides a **checklist** to `COLLAPSED_CHILDREN` rows
//!   inside a parent card. It is a budget over a list of children, not over one
//!   entry's body.
//!
//! What *does* fit is the **tool-card lifecycle** in `transcript/mod.rs`: a
//! per-entry `Option<bool>` override over a global default (`tools_expanded`),
//! toggled by a click through `EntryHit`, with the render cache invalidated. This
//! card reuses exactly that shape - `Entry::Compaction { expanded: Option<bool>,
//! .. }` - so there is one lifecycle in the transcript, not two. The only thing it
//! borrows from #391 is the row-budget idea, and it does not need it: a collapsed
//! card is a fixed number of rows by construction.

use crate::theme::{Role, Theme};
use crate::transcript::Entry;
use crate::transcript::render::render_key_hint;
use lca_tui::engine::text::{visible_width, wrap_text_with_ansi};

/// What a compaction card knows (gh #393).
///
/// Everything here comes from the persisted `Record::Compaction`, so the card is
/// a *view* of the record - the stored record is unchanged. `None` for the live
/// path, which has no record to read (see [`Card::live`]).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Card {
    /// The strategy that produced the summary (`llm`, `mechanical`, …). Empty
    /// when the live event did not say - it carries a *reason* instead.
    pub strategy: String,
    /// The summary text. Empty on the live path, whose event carries none.
    pub summary: String,
    /// First replaced record id, empty when unknown.
    pub replaced_from: String,
    /// Last replaced record id, empty when unknown.
    pub replaced_to: String,
    /// How many entries the summary stands in for, when known.
    pub replaced_count: Option<usize>,
    /// The live reason (`manual`/`threshold`/`overflow`), empty on replay.
    pub reason: String,
    /// Whether the compaction actually wrote a record. `false` is not a neutral
    /// fact: the context is unchanged, and the card must say so.
    pub success: bool,
    /// Whether this card is the live marker rather than a replayed record.
    pub live: bool,
}

impl Card {
    /// A card for a replayed `Record::Compaction`.
    #[must_use]
    pub fn replay(
        strategy: &str,
        summary: &str,
        replaced_from: &str,
        replaced_to: &str,
        replaced_count: Option<usize>,
    ) -> Self {
        Self {
            strategy: strategy.to_string(),
            summary: summary.to_string(),
            replaced_from: replaced_from.to_string(),
            replaced_to: replaced_to.to_string(),
            replaced_count,
            reason: String::new(),
            success: true,
            live: false,
        }
    }

    /// The live marker for a finished compaction (gh #496's shape).
    ///
    /// It has no summary: `TurnEvent::CompactionEnded` is `{reason, success}` and
    /// `lca-ui` has no session store to read the record from. So the live card
    /// states the *event* and the replayed card states the *content* - the same
    /// split #496 documented, now with the event collapsed to one line each.
    #[must_use]
    pub fn live(reason: &str, success: bool) -> Self {
        Self {
            strategy: String::new(),
            summary: String::new(),
            replaced_from: String::new(),
            replaced_to: String::new(),
            replaced_count: None,
            reason: reason.to_string(),
            success,
            live: true,
        }
    }

    /// A card for a replayed `Record::BranchSummary`.
    ///
    /// A branch summary is the same wall-of-text problem in a different record: it
    /// carries prose about an abandoned path, which the user has already read.
    #[must_use]
    pub fn branch(summary: &str) -> Self {
        Self {
            strategy: "branch".to_string(),
            summary: summary.to_string(),
            replaced_from: String::new(),
            replaced_to: String::new(),
            replaced_count: None,
            reason: String::new(),
            success: true,
            live: false,
        }
    }

    /// Whether this card has a body worth expanding.
    ///
    /// A live marker has none - there is no summary to reveal - so it must not
    /// advertise an expand key it cannot honour.
    #[must_use]
    pub fn has_body(&self) -> bool {
        !self.summary.is_empty()
    }

    /// The one-line headline: what happened, in the reader's words.
    ///
    /// Deliberately not the summary's first line: a summary's opening sentence is
    /// prose about the code, which does not tell the reader that their context was
    /// rewritten.
    #[must_use]
    pub fn headline(&self) -> String {
        if self.live && !self.success {
            // A failure is not a compaction: nothing was rewritten.
            return format!(
                "compaction failed ({}) - the context is unchanged",
                self.reason
            );
        }
        let mut parts: Vec<String> = Vec::new();
        if self.live {
            parts.push(format!("summarised ({})", self.reason));
        } else if self.strategy == "branch" {
            parts.push("branch summary".to_string());
        } else if self.strategy.is_empty() {
            parts.push("summarised".to_string());
        } else {
            parts.push(format!("summarised via {}", self.strategy));
        }
        if let Some(count) = self.replaced_count {
            if count > 0 {
                parts.push(format!(
                    "{count} entr{} replaced",
                    if count == 1 { "y" } else { "ies" }
                ));
            }
        } else if !self.replaced_from.is_empty() && !self.replaced_to.is_empty() {
            // The record ids are what the log names; a reader who greps the
            // session file recognises them.
            parts.push(format!(
                "{}..{}",
                short_id(&self.replaced_from),
                short_id(&self.replaced_to)
            ));
        }
        parts.join(" · ")
    }
}

/// The first eight characters of a record id, which is what distinguishes two
/// ids at a glance. Ids shorter than that are returned whole.
fn short_id(id: &str) -> &str {
    match id.char_indices().nth(8) {
        Some((at, _)) => &id[..at],
        None => id,
    }
}

/// Draw a compaction card.
///
/// Collapsed, this is a label row, the headline, and an expand hint - three rows
/// whatever the summary's length. Expanded, the summary follows, wrapped to the
/// card's inner width.
pub(super) fn render_card(
    card: &Card,
    expanded: bool,
    width: u16,
    theme: &Theme,
    keybindings: &lca_tui::engine::keybindings::KeybindingsManager,
    out: &mut Vec<String>,
) {
    let width = width as usize;
    // The same `customMessageBg` band #496 used, so the card still reads as a
    // system message rather than as assistant prose.
    let bg = theme.bg(Role::CustomMessageBg);
    out.push(band_row("", width, &bg));

    let label = if card.live && !card.success {
        "[compaction failed]"
    } else {
        "[compaction]"
    };
    let headline = format!(
        "{} {}",
        (theme.role(Role::CustomMessageLabel))(label),
        (theme.role(Role::CustomMessageText))(&card.headline())
    );
    for line in wrap_text_with_ansi(&headline, width.saturating_sub(3).max(1)) {
        out.push(band_row(&format!(" {line}"), width, &bg));
    }

    if expanded && card.has_body() {
        for line in wrap_text_with_ansi(
            &(theme.role(Role::CustomMessageText))(&card.summary),
            width.saturating_sub(3).max(1),
        ) {
            out.push(band_row(&format!(" {line}"), width, &bg));
        }
    }

    // The hint names the key the interface will actually accept (gh #410), and
    // only when there is something to reveal.
    if card.has_body() {
        let key = render_key_hint(theme, keybindings, "app.tools.expand", None);
        let shape = if key.is_empty() {
            if expanded {
                "collapse".to_string()
            } else {
                "expand".to_string()
            }
        } else if expanded {
            format!("collapse ({key})")
        } else {
            format!("expand ({key})")
        };
        let row = format!("   {}", (theme.role(Role::Muted))(&shape));
        out.push(band_row(&row, width, &bg));
    }

    out.push(band_row("", width, &bg));
}

/// A full-width band row, padded and styled (the shape `render.rs` uses).
fn band_row(row: &str, width: usize, bg: &crate::theme::StyleFn) -> String {
    let pad = width.saturating_sub(visible_width(row));
    bg(&format!("{row}{}", " ".repeat(pad)))
}

impl crate::transcript::Transcript {
    /// A compaction card's effective expansion (gh #393). `None` for anything
    /// that is not a compaction card.
    ///
    /// A live marker has no summary to reveal, so it reports `None`: it must not
    /// advertise an expand key it cannot honour.
    pub fn compaction_card_expanded(&self, index: usize) -> Option<bool> {
        let Entry::Compaction { card, expanded } = self.entries.get(index)? else {
            return None;
        };
        card.has_body()
            .then(|| expanded.unwrap_or(self.tools_expanded))
    }

    /// Toggle one compaction card's expansion by entry index (gh #393's click
    /// path), the same per-entry override a tool card uses.
    pub fn toggle_entry_compaction(&mut self, index: usize) -> bool {
        let global = self.tools_expanded;
        let Some(Entry::Compaction { card, expanded }) = self.entries.get_mut(index) else {
            return false;
        };
        if !card.has_body() {
            return false;
        }
        let effective = expanded.unwrap_or(global);
        *expanded = Some(!effective);
        self.invalidate_cache();
        true
    }
}
