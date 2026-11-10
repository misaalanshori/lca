//! gh #557 target 2 - `lib.rs` split into `prompt.rs` + `store.rs` without
//! moving a crate-root path.
//!
//! The crate root used to hold the grant store, the prompt contracts, the
//! proposal vocabulary and the dialog seam's rows in one 1,048-line file. The
//! split is by concern:
//!
//! - `prompt.rs` - `PermissionMode`, `Decision`, `PermissionPrompt`,
//!   `SharedPrompt`, `DialogPrompt`, `SharedDialogs`, `Outcome`;
//! - `store.rs` - `Proposals`, `ProposalDiff`, `GrantStore`, `SessionGrants`,
//!   `canonical_key`, `proposal_hash`, `Error`.
//!
//! Two rows, because a line count cannot see either:
//!
//! 1. **The crate root is an index** - it declares modules, re-exports, and
//!    nothing else. A split that leaves a `pub struct` behind is the same file
//!    growing back (the cycle-6 lesson this issue cites).
//! 2. **The store still behaves through the crate-root path** - `open`,
//!    `approve_pattern`, `is_allowed`, `proposal_diff`, `proposal_hash`,
//!    `canonical_key`, `authorize` - measured, not inspected.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failure.

use std::path::PathBuf;

/// A scratch project directory and store path, outside any real project.
fn scratch(tag: &str) -> (PathBuf, PathBuf) {
    let root = std::env::temp_dir().join(format!("lca-557-root-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("proj")).expect("create the scratch project");
    (root.join("proj"), root.join("grants.json"))
}

// Verifies: gh #557 target 2 - the crate root declares modules and re-exports,
// and defines nothing of its own except the crate docs and the dialog seam's
// two rows (which stayed to keep their paths).
//
// The split's whole point is that `lib.rs` stops being a place where code
// accumulates; a single `pub struct` left behind turns it back into one.
#[test]
fn the_crate_root_is_an_index() {
    // A direct read, not the module walk: the assertion is about *this file*,
    // and the walk would concatenate the very children the split moved to.
    let root = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/crates/lca-permissions/src/lib.rs"
    ))
    .expect("the crate root is readable");
    for item in [
        "pub struct ",
        "pub enum ",
        "pub trait ",
        "pub type ",
        "pub fn ",
    ] {
        assert!(
            !root.contains(item),
            "lib.rs declares no `{item}`: it re-exports from store.rs/prompt.rs"
        );
    }
    // And it does re-export what the split moved, so the paths survive.
    for surface in [
        "pub use prompt::{",
        "pub use store::{",
        "pub use authorize::authorize;",
        "pub use action::Action;",
    ] {
        assert!(
            root.contains(surface),
            "the crate root re-exports: {surface}"
        );
    }
}

// Verifies: gh #557 target 2 - the grant store's observable behaviour is
// unchanged through the paths a caller names, and the authorize flow still
// refuses when the prompt refuses.
#[test]
fn the_store_still_behaves_through_the_crate_root() {
    use lca_permissions::{
        Action, Decision, Error, GrantStore, PermissionMode, PermissionPrompt, ProposalDiff,
        Proposals, authorize, canonical_key, proposal_hash,
    };

    let (project, store_path) = scratch("store");
    let mut store = GrantStore::open(&store_path).expect("open a fresh store");
    assert_eq!(store.permission_mode(), PermissionMode::Ask);
    store.set_permission_mode(PermissionMode::Yolo);
    assert_eq!(store.permission_mode(), PermissionMode::Yolo);
    // Back to Ask: the refusal row below is about a prompt answering, not
    // about yolo short-circuiting it (ADR-0042).
    store.set_permission_mode(PermissionMode::Ask);

    let action = Action::Shell {
        command: "cargo build".to_string(),
        cwd: project.clone(),
    };
    assert!(
        !store.is_allowed(&project, &action),
        "a fresh store grants nothing"
    );
    store
        .approve_pattern(&project, "cargo build")
        .expect("the pattern persists");
    assert!(
        store.is_allowed(&project, &action),
        "an approved pattern allows"
    );
    assert_eq!(store.patterns(&project), vec!["cargo build".to_string()]);

    let mut proposals = Proposals::default();
    proposals.insert("read:src/**".to_string(), "the source tree".to_string());
    assert!(!store.proposal_diff(&project, &proposals).is_empty());
    store
        .apply_proposals(&project, &proposals)
        .expect("approve the set");
    assert!(
        store.proposal_diff(&project, &proposals).is_empty(),
        "an approved set does not re-prompt"
    );
    assert_eq!(
        store.proposal_patterns(&project),
        vec!["read:src/**".to_string()]
    );

    // The two free functions the split moved keep their values.
    assert!(canonical_key(&project).ends_with("proj"));
    let mut changed = proposals.clone();
    changed.insert("write:out/**".to_string(), "x".to_string());
    assert_ne!(proposal_hash(&proposals), proposal_hash(&changed));
    assert_eq!(proposal_hash(&proposals), proposal_hash(&proposals));

    // Deny beats allow, and the authorize flow refuses when the prompt does.
    store
        .add_rule(
            &project,
            lca_permissions::RuleScope::Project,
            lca_permissions::RuleDecision::Allow,
            "cargo build",
        )
        .expect("allow rule");
    assert!(store.is_allowed(&project, &action));

    struct Refuse;
    impl PermissionPrompt for Refuse {
        fn ask(&mut self, _action: &Action) -> Decision {
            Decision::Denied
        }
        fn review_proposals(&mut self, _diff: &ProposalDiff) -> bool {
            false
        }
    }
    // A *write* outside the workspace: a read outside it is auto-allowed by
    // design (`reads_outside_the_workspace_do_not_prompt`), so a read would
    // not reach the prompt this row is about.
    let outside = Action::WritePath {
        path: PathBuf::from("/etc/passwd"),
    };
    let outcome = authorize(&mut store, &project, &outside, None, &mut Refuse, None)
        .expect("authorize does not fail on a refusal");
    assert!(!outcome.allowed, "the prompt's refusal is the answer");
    assert!(outcome.prompted, "and it was asked");

    // A session trust covers an in-workspace command (`is_trusted_here` is
    // the store's own read, so this is the round trip gh #557 moved).
    store.trust_for_session(&project);
    assert!(store.is_trusted_for_session(&project));
    assert!(store.is_trusted_here(&project));
    assert!(store.is_allowed(&project, &action));

    // `Error` is still the crate's error type, by that path.
    let bad = store.approve_net_pattern(&project, "not a host");
    assert!(
        matches!(bad, Err(Error::Pattern(_))),
        "invalid net pattern is a Pattern error"
    );

    let _ = std::fs::remove_dir_all(project.parent().unwrap_or(&project));
}
