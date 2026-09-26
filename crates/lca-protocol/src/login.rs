//! The provider login surface's neutral types (ADR-0033).
//!
//! The extension supplies display-ready options and consumes the user's
//! answers; the host renders them and persists the opaque settings it is
//! handed. Nothing here is provider-shaped beyond a display string.

use std::collections::BTreeMap;

/// One login choice the host renders.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LoginOption {
    /// Stable id the host passes back in [`LoginAnswer::choice`].
    pub id: String,
    /// Display name (e.g. `OpenRouter`).
    pub name: String,
    /// `api-key`, `oauth`, or `custom`.
    pub kind: String,
    /// The endpoint host, shown in the `net` consent when known.
    pub host: String,
    /// Field ids to prompt for: `api-key`, `base-url`, `model`.
    pub fields: Vec<String>,
    /// Free-form extras (e.g. a curated model list).
    pub extras: BTreeMap<String, String>,
}

/// The user's answers to one option.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LoginAnswer {
    /// The chosen option id.
    pub choice: String,
    /// Field id -> value (`api-key` is the secret; the host masks it).
    pub values: BTreeMap<String, String>,
}

impl LoginAnswer {
    /// One field's value, when present.
    pub fn value(&self, field: &str) -> Option<&str> {
        self.values.get(field).map(String::as_str)
    }
}
