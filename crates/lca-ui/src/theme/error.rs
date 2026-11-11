//! Why a theme file (or this crate's own token table) was refused (gh #575).
//!
//! # Why this is a type and not a `String`
//!
//! The notice a user sees for a broken theme has **two** audiences, and they
//! need different sentences:
//!
//! - a **file** fault is the user's to fix ("the colour to use is here"),
//! - a **token-table** fault is a bug in this crate: the shipped table is
//!   internal, so no edit in the user's theme can cause it, and telling them
//!   "your theme is ignored" would send them to the wrong file.
//!
//! Before this type both arrived as one `String` and `theme::parse` printed
//! `theme <name> ignored (<reason>)` for both - so a shipped-table defect
//! surfaced as an accusation against the user's theme file. The distinction
//! is what makes the type earn its place (the house rule: a typed error exists
//! where a caller *branches*, not merely where a string is long).
//!
//! # Copy
//!
//! `Display` is the sentence that follows the role or the file in a notice,
//! and it stays the wording callers already matched on (`is not a table of
//! roles`, `has an invalid color`) - a type is not a licence to reword the
//! product.

use crate::theme::Role;

/// Why a theme file could not be applied.
///
/// The `Display`/`Error` pairs are written out rather than derived: `thiserror`
/// is a workspace dependency but **not one of `lca-ui`'s**, and adding it for
/// four variants would be a dependency for a derive that costs twenty lines.
/// The type is still `impl std::error::Error` for every caller that asks.
#[derive(Debug)]
pub enum ThemeError {
    /// The file is not TOML.
    Toml(toml::de::Error),
    /// TOML, but not a table of roles (or of `[colors]` roles).
    NotATable,
    /// A role's value is not a string.
    RoleNotAString {
        /// The role as the file spelled it.
        role: Role,
    },
    /// A role's value does not parse as a colour.
    InvalidColor {
        /// The role as the file spelled it.
        role: Role,
        /// The value, verbatim, so the user can find it.
        value: String,
    },
    /// **This crate's own fallback table is broken** (gh #387 gap 5).
    ///
    /// A programming defect, never the user's: `theme::parse` reports it
    /// differently on purpose, because no theme file can cause it.
    TokenTable(String),
}

impl From<toml::de::Error> for ThemeError {
    fn from(err: toml::de::Error) -> Self {
        Self::Toml(err)
    }
}

impl std::fmt::Display for ThemeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // The wording callers already matched on, kept verbatim.
            Self::Toml(err) => write!(f, "{err}"),
            Self::NotATable => write!(f, "the theme file is not a table of roles"),
            Self::RoleNotAString { role } => write!(f, "role `{}` is not a string", role.key()),
            Self::InvalidColor { role, value } => {
                write!(f, "role `{}` has an invalid color `{value}`", role.key())
            }
            Self::TokenTable(reason) => write!(f, "{reason}"),
        }
    }
}

impl std::error::Error for ThemeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Toml(err) => Some(err),
            _ => None,
        }
    }
}

impl ThemeError {
    /// Whether the fault is **this crate's**, not the user's file.
    ///
    /// The whole reason the type exists: a broken shipped token table must not
    /// be reported as "your theme was ignored", and a notice can say which
    /// audience it is talking to.
    #[must_use]
    pub fn is_programming_defect(&self) -> bool {
        matches!(self, Self::TokenTable(_))
    }
}
