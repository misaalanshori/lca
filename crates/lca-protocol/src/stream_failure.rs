//! How a provider stream run failed, in the vocabulary the core's
//! `ProviderError` speaks (`docs/headless.md`'s classes).
//!
//! **One definition for every wire kit** (gh #576). It used to exist twice - in
//! `lca-wire-openai` and in `lca-wire-anthropic` - with the same three fields and
//! each kit's own `From<CapabilityError>`. The two `From` impls agreed, which was
//! luck rather than design: nothing pinned them equal, so a policy edit in one kit
//! would silently diverge from the other. This type is the single home, and the
//! one `From` here is the policy's only implementation.
//!
//! It lives in `lca-protocol` beside [`CapabilityError`] because the *reason* for
//! existing is cross-crate: `lca-subscription` named it through
//! `lca_wire_openai` and depended on that kit for nothing else.
//!
//! # Why the fields are public
//!
//! 45 struct-literal sites build a `StreamFailure` across `crates/`,
//! `extensions/` and `tests/`, and they are deliberately not swept by this cycle:
//! a mechanical rewrite of 45 sites would bury the guard fix that landed beside
//! this type. `#[non_exhaustive]` would break every one of them for no gain, so
//! the fields stay public and the constructors below are the shape new code (and
//! any site touched for another reason) uses. This is stated here so the next
//! reader knows it is deliberate rather than unfinished.

use std::fmt;

use crate::CapabilityError;

/// How a provider call failed: the message the user reads, the headless
/// envelope class (`docs/headless.md`), and whether a retry could help
/// (FR-CORE-6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamFailure {
    /// Human-readable message.
    pub message: String,
    /// Class for the headless envelope (`docs/headless.md`).
    pub class: &'static str,
    /// Whether a retry could help (FR-CORE-6).
    pub retryable: bool,
}

impl StreamFailure {
    /// Build one from its three facts.
    ///
    /// The class is `&'static str` rather than an enum because the vocabulary is
    /// the headless envelope's (`auth`, `invalid`, `transport`), documented as
    /// strings in `docs/headless.md`, and a kit may introduce a new one without an
    /// ABI change.
    pub fn new(message: impl Into<String>, class: &'static str, retryable: bool) -> Self {
        StreamFailure {
            message: message.into(),
            class,
            retryable,
        }
    }

    /// A configuration or request problem: never retried, and its class keeps it
    /// out of `transport` (gh #356: a permanent auth failure retried three times
    /// with backoff and then exited on the wrong code).
    pub fn invalid(message: impl Into<String>) -> Self {
        StreamFailure::new(message, "invalid", false)
    }

    /// A credential refusal: never retried, and classed `auth` so the caller can
    /// offer a re-login rather than a retry.
    pub fn auth(message: impl Into<String>) -> Self {
        StreamFailure::new(message, "auth", false)
    }

    /// A failure a retry could heal: network, timeout, a 5xx.
    pub fn transport(message: impl Into<String>) -> Self {
        StreamFailure::new(message, "transport", true)
    }
}

impl fmt::Display for StreamFailure {
    /// The message **verbatim**: no prefix, no class.
    ///
    /// The class is a separate field by design - the headless envelope carries it
    /// beside the message, and smuggling it into the text would make the same fact
    /// appear twice and let a caller's `class` and its message disagree.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for StreamFailure {}

impl From<CapabilityError> for StreamFailure {
    /// The **one** capability-to-stream classification table.
    ///
    /// A refusal is a configuration problem, not a flaky network: `Permission`,
    /// `NotGranted`, `NotFound` and `Invalid` are `("invalid", false)`, so they are
    /// never retried and never reach a caller that would back off. `Io` and
    /// `Timeout` are `("transport", true)`: the network may heal.
    ///
    /// This used to be two copies (one per wire kit) that happened to agree. One
    /// copy is the point of the type's move, and
    /// `the_capability_error_classification_table_is_pinned` in
    /// `tests/regressions/576-*` asserts every variant's pair so an accidental
    /// agreement cannot become an accidental divergence.
    fn from(err: CapabilityError) -> Self {
        use CapabilityError as E;
        let (class, retryable) = match &err {
            E::Permission(_) | E::NotGranted(_) | E::NotFound(_) | E::Invalid(_) => {
                ("invalid", false)
            }
            E::Io(_) | E::Timeout(_) => ("transport", true),
        };
        StreamFailure {
            message: err.to_string(),
            class,
            retryable,
        }
    }
}
