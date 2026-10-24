//! The startup collision summary (gh #400 gap 3): what `ExtensionRegistry` already
//! decided, carried to the place that can show it.
//!
//! The registry records a `CollisionReport` whenever a registration
//! loses a name - a project copy shadowing an installed one, an installed one
//! shadowing the bundled one, a tool losing to a built-in. **Nothing read it.** The
//! fact was known, correct, and discarded, so a user who installed an extension and
//! found the bundled one answering had no way to learn why.
//!
//! This module is the projection, not the decision: the registry still decides who
//! wins (FR-EXT-11), and `from_reports` only restates the outcome in the shape the
//! header paints. That split is deliberate - a notice must never become the thing
//! that changes what loads.

/// One collision, as a row can say it: a name, what kind of thing it is, who kept
/// it and who did not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollisionEntry {
    /// The contested name.
    pub name: String,
    /// `"extension"`, `"tool"`, `"command"` or `"prompt"` - the registry's own word
    /// for what was contested.
    pub kind: String,
    /// The side that kept the name.
    pub winner: String,
    /// The side that was skipped for this session.
    pub loser: String,
}

/// Every collision this session's registry recorded.
///
/// `Default` is empty, which is what makes "no collision, no row" the natural
/// state rather than a special case.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CollisionSummary {
    /// One entry per recorded collision, in registration order.
    pub entries: Vec<CollisionEntry>,
}

impl CollisionSummary {
    /// Build a summary from `(name, kind, winner, loser)` rows.
    ///
    /// The registry's own `CollisionReport` is converted in `lca-cli` (which
    /// depends on `lca-core`); this constructor keeps `lca-ui` free of that
    /// dependency, which is the same split `GenerationUs` uses (ADR-0036).
    pub fn from_rows(rows: Vec<CollisionEntry>) -> CollisionSummary {
        CollisionSummary { entries: rows }
    }

    /// Whether anything collided. The one question every painter asks first.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// How many collisions were recorded.
    pub fn len(&self) -> usize {
        self.entries.len()
    }
}
