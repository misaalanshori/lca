//! The permission layer: the split grant store from ADR-0006 and the
//! authorize flow from `docs/flows.md`.
//!
//! The project file holds proposals with no force. Only the user grant
//! store, keyed by the canonical project path, is consulted when an action
//! runs (FR-PERM-11). Approving copies into that store behind a prompt that
//! shows what is being added, and a changed proposal set re-prompts with
//! the difference (FR-PERM-10).
//!
//! The crate root is the index: **the grant store** (`store.rs`, ADR-0006),
//! **the human seams** (`prompt.rs`: what a decision is, the swappable slots,
//! the outcome), and **the flow** (`authorize.rs`) are three modules, and each
//! re-export below is the path a caller named before the split
//! (`lca_permissions::GrantStore`, `…::SharedPrompt`, `…::authorize`).

#![forbid(unsafe_code)]

mod action;
mod authorize;
mod net;
mod prompt;
pub mod rule_entry;
mod rules;
mod scope;
pub mod sensitive;
pub mod shell;
mod store;
mod trust;

pub use action::Action;
pub use authorize::authorize;
use rules::RuleMatch;
pub use rules::{RuleDecision, RuleScope, RuleSet, RuleView, wildcard_match};
// The `fs` vocabulary and its path resolution (ADR-0005), which is its own
// file so this one stays under the workspace's 1,200-line ceiling.
pub use scope::{
    FsMode, OAuthSettings, ScopeGrant, ScopeRoots, ScopeViolation, ScopeViolationKind,
    ServeHttpSettings,
};

// The store and its prompt seam (ADR-0006, ADR-0042): re-exported so the
// module split changed no caller's path.
pub use prompt::{
    Decision, DialogPrompt, Outcome, PermissionMode, PermissionPrompt, SharedDialogs, SharedPrompt,
};
pub use store::{Error, GrantStore, ProposalDiff, Proposals, canonical_key, proposal_hash};

// ---------------------------------------------------------------------------
// Filesystem scopes (ADR-0005, capability catalog `fs`)
// ---------------------------------------------------------------------------

pub use net::{
    LocalPattern, NetPattern, PatternError, is_local_address, normalize_ip, parse_local_pattern,
    parse_net_pattern,
};

// The dialog seam's rows stay in the crate root, where they were declared
// before the split: a `mod tests` block left in the barrel keeps every test's
// path (`dialog_tests::<row>`), and gh #557's bar is that a pure move renames
// no row (AGENTS.md).
#[cfg(test)]
mod dialog_tests {
    use super::*;

    // Verifies: gh #124 - with nothing installed every question answers
    // its denied value, which is the headless contract (no modal, no
    // hang, notify silently dropped).
    #[test]
    fn an_empty_slot_denies_every_question() {
        let mut slot = SharedDialogs::default();
        assert!(!slot.confirm("t", "m"));
        assert_eq!(slot.select("t", &["a".to_string()]), None);
        assert_eq!(slot.input("l", None), None);
        slot.notify("hello", "info");
    }

    struct Scripted {
        answers: Vec<String>,
    }

    impl DialogPrompt for Scripted {
        fn confirm(&mut self, _title: &str, _message: &str) -> bool {
            true
        }
        fn select(&mut self, _title: &str, options: &[String]) -> Option<String> {
            options.first().cloned()
        }
        fn input(&mut self, _label: &str, placeholder: Option<&str>) -> Option<String> {
            Some(placeholder.unwrap_or("typed").to_string())
        }
        fn notify(&mut self, message: &str, _level: &str) {
            self.answers.push(message.to_string());
        }
    }

    // Verifies: gh #124 - an installed prompter answers, so the live
    // TUI side and the headless side share one seam.
    #[test]
    fn an_installed_prompter_answers() {
        let mut slot = SharedDialogs::default();
        slot.set(std::sync::Arc::new(std::sync::Mutex::new(Scripted {
            answers: Vec::new(),
        })));
        assert!(slot.confirm("t", "m"));
        assert_eq!(
            slot.select("t", &["a".to_string(), "b".to_string()]),
            Some("a".to_string())
        );
        assert_eq!(slot.input("l", Some("ph")), Some("ph".to_string()));
    }
}
