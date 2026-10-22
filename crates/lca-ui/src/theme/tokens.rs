//! Per-role fallback chains (gh #387 gap 5).
//!
//! A theme file may name a subset of the vocabulary; every role it omits has to
//! resolve *somehow*. Before this module the answer was "whatever the previous
//! palette held, and failing that `Text`", which is ad-hoc in two ways that both
//! hurt users:
//!
//! 1. A theme built a year ago could start rendering differently the moment a
//!    role was added to the vocabulary, because the new role's value came from
//!    whichever base palette happened to be underneath.
//! 2. The fallback was never *semantic*. `toolDiffAdded` falling back to `text`
//!    is not a decision anybody made; it is what remained when nothing else
//!    matched.
//!
//! # The table is the single source of truth
//!
//! Every role declares a [`Slot`] and, unless it is the slot's root, a parent
//! role **in the same slot**. `Palette::overlay` walks that
//! chain at *load* time, so the render path keeps reading a single map value:
//! nothing gets slower, and the resolution can be tested in isolation.
//!
//! The slot rule is the load-bearing half. pi's `theme-tokens.ts` rejects a
//! fallback that crosses slots (`has fallback X in a different color slot`) and
//! it is right to: a background role falling back to a foreground role would
//! paint a band in what is supposed to be the text colour. This module refuses
//! the same thing, and `validate` fails the table rather than shipping it.
//!
//! # A user's parent override carries its children
//!
//! This is the whole point of a semantic chain, and the row that proves the
//! chain is real rather than a lookup that happens to work on the defaults:
//! setting `accent` in a theme file moves `mdLink`, `mdCode`, `mdListBullet`,
//! `syntaxKeyword`, `customMessageLabel` and `bashMode` with it, because those
//! children name `accent` as their parent.
//!
//! # The roots
//!
//! Two roles are slot roots — `text` (foreground) and `selectedBg` (background) —
//! and they have no fallback. A theme that names neither resolves the rest to
//! [`Color::Default`], which is not a gap: `Default` *means* "the terminal's own
//! foreground / no background at all", and that is the honest answer for a theme
//! that declined to say. The `system` theme depends on exactly that.

use super::{Color, Role};

/// Which colour channel a role paints.
///
/// A fallback never crosses this boundary; see the module docs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    /// Painted as a foreground (`SGR 38`).
    Foreground,
    /// Painted as a background (`SGR 48`).
    Background,
}

/// One role's place in the vocabulary.
#[derive(Clone, Copy, Debug)]
pub struct Token {
    /// The role this entry describes.
    pub role: Role,
    /// The channel it paints.
    pub slot: Slot,
    /// The role it inherits from, or `None` for a slot root.
    pub fallback: Option<Role>,
}

/// Every role, its slot, and its parent.
///
/// Order follows [`Role::ALL`] so a reader can diff the two by eye; the
/// `no_orphan_roles` row enforces that the sets match exactly.
pub const TOKENS: &[Token] = &[
    Token {
        role: Role::Accent,
        slot: Slot::Foreground,
        fallback: Some(Role::Text),
    },
    Token {
        role: Role::Border,
        slot: Slot::Foreground,
        fallback: Some(Role::Text),
    },
    Token {
        role: Role::BorderAccent,
        slot: Slot::Foreground,
        fallback: Some(Role::Border),
    },
    Token {
        role: Role::BorderMuted,
        slot: Slot::Foreground,
        fallback: Some(Role::Border),
    },
    Token {
        role: Role::Success,
        slot: Slot::Foreground,
        fallback: Some(Role::Text),
    },
    Token {
        role: Role::Error,
        slot: Slot::Foreground,
        fallback: Some(Role::Text),
    },
    Token {
        role: Role::Warning,
        slot: Slot::Foreground,
        fallback: Some(Role::Text),
    },
    Token {
        role: Role::Muted,
        slot: Slot::Foreground,
        fallback: Some(Role::Text),
    },
    Token {
        role: Role::Dim,
        slot: Slot::Foreground,
        fallback: Some(Role::Muted),
    },
    Token {
        role: Role::Text,
        slot: Slot::Foreground,
        fallback: None,
    },
    Token {
        role: Role::ThinkingText,
        slot: Slot::Foreground,
        fallback: Some(Role::Muted),
    },
    Token {
        role: Role::ScrollbarTrack,
        slot: Slot::Foreground,
        fallback: Some(Role::Muted),
    },
    Token {
        role: Role::ScrollbarThumb,
        slot: Slot::Foreground,
        fallback: Some(Role::Text),
    },
    Token {
        role: Role::SearchMatchText,
        slot: Slot::Foreground,
        fallback: Some(Role::Text),
    },
    Token {
        role: Role::UserMessageText,
        slot: Slot::Foreground,
        fallback: Some(Role::Text),
    },
    Token {
        role: Role::CustomMessageText,
        slot: Slot::Foreground,
        fallback: Some(Role::Text),
    },
    Token {
        role: Role::CustomMessageLabel,
        slot: Slot::Foreground,
        fallback: Some(Role::Accent),
    },
    Token {
        role: Role::ToolTitle,
        slot: Slot::Foreground,
        fallback: Some(Role::Text),
    },
    Token {
        role: Role::ToolOutput,
        slot: Slot::Foreground,
        fallback: Some(Role::Muted),
    },
    Token {
        role: Role::MdHeading,
        slot: Slot::Foreground,
        fallback: Some(Role::Text),
    },
    Token {
        role: Role::MdLink,
        slot: Slot::Foreground,
        fallback: Some(Role::Accent),
    },
    Token {
        role: Role::MdLinkUrl,
        slot: Slot::Foreground,
        fallback: Some(Role::Muted),
    },
    Token {
        role: Role::MdCode,
        slot: Slot::Foreground,
        fallback: Some(Role::Accent),
    },
    Token {
        role: Role::MdCodeBlock,
        slot: Slot::Foreground,
        fallback: Some(Role::Text),
    },
    Token {
        role: Role::MdCodeBlockBorder,
        slot: Slot::Foreground,
        fallback: Some(Role::BorderMuted),
    },
    Token {
        role: Role::MdQuote,
        slot: Slot::Foreground,
        fallback: Some(Role::Muted),
    },
    Token {
        role: Role::MdQuoteBorder,
        slot: Slot::Foreground,
        fallback: Some(Role::BorderMuted),
    },
    Token {
        role: Role::MdHr,
        slot: Slot::Foreground,
        fallback: Some(Role::BorderMuted),
    },
    Token {
        role: Role::MdListBullet,
        slot: Slot::Foreground,
        fallback: Some(Role::Accent),
    },
    Token {
        role: Role::ToolDiffAdded,
        slot: Slot::Foreground,
        fallback: Some(Role::Success),
    },
    Token {
        role: Role::ToolDiffRemoved,
        slot: Slot::Foreground,
        fallback: Some(Role::Error),
    },
    Token {
        role: Role::ToolDiffContext,
        slot: Slot::Foreground,
        fallback: Some(Role::Muted),
    },
    Token {
        role: Role::SyntaxComment,
        slot: Slot::Foreground,
        fallback: Some(Role::Muted),
    },
    Token {
        role: Role::SyntaxKeyword,
        slot: Slot::Foreground,
        fallback: Some(Role::Accent),
    },
    Token {
        role: Role::SyntaxFunction,
        slot: Slot::Foreground,
        fallback: Some(Role::Text),
    },
    Token {
        role: Role::SyntaxVariable,
        slot: Slot::Foreground,
        fallback: Some(Role::Text),
    },
    Token {
        role: Role::SyntaxString,
        slot: Slot::Foreground,
        fallback: Some(Role::Success),
    },
    Token {
        role: Role::SyntaxNumber,
        slot: Slot::Foreground,
        fallback: Some(Role::Warning),
    },
    Token {
        role: Role::SyntaxType,
        slot: Slot::Foreground,
        fallback: Some(Role::MdCode),
    },
    Token {
        role: Role::SyntaxOperator,
        slot: Slot::Foreground,
        fallback: Some(Role::Text),
    },
    Token {
        role: Role::SyntaxPunctuation,
        slot: Slot::Foreground,
        fallback: Some(Role::Muted),
    },
    Token {
        role: Role::ThinkingOff,
        slot: Slot::Foreground,
        fallback: Some(Role::BorderMuted),
    },
    Token {
        role: Role::ThinkingMinimal,
        slot: Slot::Foreground,
        fallback: Some(Role::ThinkingOff),
    },
    Token {
        role: Role::ThinkingLow,
        slot: Slot::Foreground,
        fallback: Some(Role::ThinkingMinimal),
    },
    Token {
        role: Role::ThinkingMedium,
        slot: Slot::Foreground,
        fallback: Some(Role::ThinkingLow),
    },
    Token {
        role: Role::ThinkingHigh,
        slot: Slot::Foreground,
        fallback: Some(Role::ThinkingMedium),
    },
    Token {
        role: Role::ThinkingXhigh,
        slot: Slot::Foreground,
        fallback: Some(Role::ThinkingHigh),
    },
    Token {
        role: Role::ThinkingMax,
        slot: Slot::Foreground,
        fallback: Some(Role::ThinkingXhigh),
    },
    Token {
        role: Role::BashMode,
        slot: Slot::Foreground,
        fallback: Some(Role::Accent),
    },
    Token {
        role: Role::SelectedBg,
        slot: Slot::Background,
        fallback: None,
    },
    Token {
        role: Role::SearchMatchBg,
        slot: Slot::Background,
        fallback: Some(Role::SelectedBg),
    },
    Token {
        role: Role::UserMessageBg,
        slot: Slot::Background,
        fallback: Some(Role::SelectedBg),
    },
    Token {
        role: Role::CustomMessageBg,
        slot: Slot::Background,
        fallback: Some(Role::SelectedBg),
    },
    Token {
        role: Role::SidePanel,
        slot: Slot::Background,
        fallback: Some(Role::SelectedBg),
    },
    Token {
        role: Role::ToolPendingBg,
        slot: Slot::Background,
        fallback: Some(Role::SelectedBg),
    },
    Token {
        role: Role::ToolSuccessBg,
        slot: Slot::Background,
        fallback: Some(Role::ToolPendingBg),
    },
    Token {
        role: Role::ToolErrorBg,
        slot: Slot::Background,
        fallback: Some(Role::ToolPendingBg),
    },
];

/// The table entry for `role`, if the table covers it.
#[must_use]
pub fn token(role: Role) -> Option<&'static Token> {
    TOKENS.iter().find(|token| token.role == role)
}

/// The role at the bottom of `slot`'s chain, if it has one.
///
/// A slot root has no parent, so it is where every chain in that slot ends.
/// Exposed for the NFR-24 guard, which needs this fact as a *value* rather than
/// as a line of source (`cargo fmt` reflows struct literals, and a substring
/// match on the unformatted form goes red for no reason).
#[must_use]
pub fn root_of(slot: Slot) -> Option<Role> {
    TOKENS
        .iter()
        .find(|token| token.slot == slot && token.fallback.is_none())
        .map(|token| token.role)
}

/// How many hops a chain may take before the walk gives up.
///
/// A correct table never needs more than the vocabulary's length; the bound
/// exists so a malformed table degrades to `Default` instead of hanging. This is
/// the production-safe half of cycle handling — [`validate`] is the half that
/// fails loudly, at load, where somebody can act on it.
const MAX_HOPS: usize = 64;

/// Resolve `role` through its declared chain, against a partially-filled map.
///
/// Returns the first ancestor present in `have`. A role with no resolvable
/// ancestor in its chain comes back as [`Color::Default`] — the terminal's own
/// value, which is exactly what a theme that named neither the role nor anything
/// above it declined to specify.
#[must_use]
pub fn resolve_chain(role: Role, have: &dyn Fn(Role) -> Option<Color>) -> Color {
    let mut current = Some(role);
    let mut hops = 0;
    while let Some(role) = current {
        if let Some(color) = have(role) {
            return color;
        }
        let Some(entry) = token(role) else {
            return Color::Default;
        };
        hops += 1;
        if hops > MAX_HOPS {
            return Color::Default;
        }
        current = entry.fallback;
    }
    Color::Default
}

/// The nearest **ancestor** of `role` present in `have`, or `None`.
///
/// Ancestor, not "the chain including me": `Palette::overlay` checks the role
/// itself first, and folding that in here would make a role its own fallback.
/// This is the function that gives a parent override its children - setting
/// `accent` answers `mdLink`, `mdCode`, `syntaxKeyword` and the rest - which is
/// the difference between a semantic chain and a lookup that only works while
/// the defaults happen to line up.
#[must_use]
pub fn nearest_named(role: Role, have: &dyn Fn(Role) -> Option<Color>) -> Option<Color> {
    let mut current = token(role).and_then(|token| token.fallback);
    let mut hops = 0;
    while let Some(role) = current {
        if let Some(color) = have(role) {
            return Some(color);
        }
        hops += 1;
        if hops > MAX_HOPS {
            return None;
        }
        current = token(role).and_then(|token| token.fallback);
    }
    None
}

/// Check the table is well-formed, naming the first fault.
///
/// Public because `Palette::overlay` calls it at load: a malformed table must
/// fail loudly with the role named, not paint a wrong colour on screen.
///
/// Four invariants, each of which has been a real class of bug somewhere:
///
/// 1. **Total**: every role in [`Role::ALL`] has exactly one entry, and no entry
///    names a role outside the vocabulary. A missing entry is the "orphan role"
///    that would make a new token silently unstyleable.
/// 2. **Roots exist**: each slot has a role with no parent, or the chain has no
///    floor and every role in the slot resolves to `Default`.
/// 3. **Same slot**: a fallback never crosses channels.
/// 4. **Acyclic**: no walk revisits a role.
///
/// # Errors
///
/// The message names the offending role (and, for a cycle, the whole loop), so a
/// caller can report it rather than guess.
pub fn validate() -> Result<(), String> {
    for role in Role::ALL {
        let entries = TOKENS.iter().filter(|token| token.role == *role).count();
        if entries != 1 {
            return Err(format!(
                "role `{}` has {entries} table entries, expected 1",
                role.key()
            ));
        }
    }
    for entry in TOKENS {
        if !Role::ALL.contains(&entry.role) {
            return Err(format!(
                "the table names `{}`, which is not in the vocabulary",
                entry.role.key()
            ));
        }
    }
    for slot in [Slot::Foreground, Slot::Background] {
        if !TOKENS
            .iter()
            .any(|token| token.slot == slot && token.fallback.is_none())
        {
            return Err(format!("slot {slot:?} has no root role"));
        }
    }
    for entry in TOKENS {
        let Some(parent) = entry.fallback else {
            continue;
        };
        let Some(parent_entry) = token(parent) else {
            return Err(format!(
                "role `{}` falls back to `{}`, which is not in the table",
                entry.role.key(),
                parent.key()
            ));
        };
        if parent_entry.slot != entry.slot {
            return Err(format!(
                "role `{}` falls back to `{}` in a different slot ({:?} vs {:?})",
                entry.role.key(),
                parent.key(),
                entry.slot,
                parent_entry.slot
            ));
        }
    }
    // Cycle detection: walk each chain, remembering where we have been.
    for role in Role::ALL {
        let mut seen = vec![*role];
        let mut current = token(*role).and_then(|token| token.fallback);
        while let Some(next) = current {
            if seen.contains(&next) {
                let loop_ = seen
                    .iter()
                    .skip_while(|seen| **seen != next)
                    .chain(std::iter::once(&next))
                    .map(|role| role.key())
                    .collect::<Vec<_>>()
                    .join(" -> ");
                return Err(format!("fallback cycle: {loop_}"));
            }
            seen.push(next);
            current = token(next).and_then(|token| token.fallback);
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "tokens_tests.rs"]
mod tests;
