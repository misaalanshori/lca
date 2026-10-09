//! Rules, folder trust, and session grants (ADR-0039).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code

use lca_permissions::{Action, GrantStore, RuleDecision, RuleScope};
use std::path::Path;
use std::path::PathBuf;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lca-rules-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create scratch");
    dir
}

fn shell(project: &Path, command: &str) -> Action {
    Action::Shell {
        command: command.to_string(),
        cwd: project.to_path_buf(),
    }
}

// Verifies: FR-PERM-21 (a deny rule refuses without a prompt and beats allow)
#[test]
fn a_deny_rule_refuses_without_a_prompt_and_beats_allow() {
    let root = scratch("deny");
    let project = root.join("proj");
    std::fs::create_dir_all(&project).unwrap();
    let path = root.join("grants.json");
    let mut store = GrantStore::open(&path).unwrap();
    store
        .add_rule(&project, RuleScope::Global, RuleDecision::Allow, "git *")
        .unwrap();
    store
        .add_rule(&project, RuleScope::Global, RuleDecision::Deny, "git push*")
        .unwrap();
    assert!(!store.is_allowed(&project, &shell(&project, "git push origin main")));
    assert!(store.rule_denied(&project, &shell(&project, "git push origin main")));
    // The allow rule still covers a different git command.
    assert!(store.is_allowed(&project, &shell(&project, "git status")));
    assert!(!store.rule_denied(&project, &shell(&project, "git status")));
    let _ = std::fs::remove_dir_all(&root);
}

// Verifies: FR-PERM-20 (trust auto-approves only in-workspace commands)
#[test]
fn folder_trust_auto_approves_only_workspace_scoped_commands() {
    let root = scratch("trust");
    let project = root.join("proj");
    std::fs::create_dir_all(&project).unwrap();
    let mut store = GrantStore::open(&root.join("grants.json")).unwrap();
    // Without trust, nothing is auto-approved.
    assert!(!store.is_allowed(&project, &shell(&project, "cargo build")));
    store.trust_for_session(&project);
    assert!(store.is_allowed(&project, &shell(&project, "cargo build")));
    assert!(store.is_allowed(&project, &shell(&project, "rm -rf target")));
    // Outside the workspace, the analyzer reviews it.
    assert!(!store.is_allowed(&project, &shell(&project, "rm -rf /tmp/x")));
    assert!(!store.is_allowed(&project, &shell(&project, "git push")));
    let _ = std::fs::remove_dir_all(&root);
}

// Verifies: FR-PERM-22 (session grants do not persist)
#[test]
fn session_grants_do_not_persist() {
    let root = scratch("session");
    let project = root.join("proj");
    std::fs::create_dir_all(&project).unwrap();
    let path = root.join("grants.json");
    {
        let mut store = GrantStore::open(&path).unwrap();
        store.trust_for_session(&project);
        store
            .add_rule(&project, RuleScope::Session, RuleDecision::Allow, "cat *")
            .unwrap();
        store
            .add_rule(&project, RuleScope::Project, RuleDecision::Allow, "ls *")
            .unwrap();
        assert!(store.is_trusted_for_session(&project));
    }
    // A fresh store (a restart) keeps the project rule but not the session
    // trust or the session rule.
    let store = GrantStore::open(&path).unwrap();
    assert!(!store.is_trusted_for_session(&project));
    assert!(!store.is_allowed(&project, &shell(&project, "cat x")));
    assert!(store.is_allowed(&project, &shell(&project, "ls x")));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn rules_are_listed_by_scope() {
    let root = scratch("list");
    let project = root.join("proj");
    std::fs::create_dir_all(&project).unwrap();
    let mut store = GrantStore::open(&root.join("grants.json")).unwrap();
    store
        .add_rule(&project, RuleScope::Global, RuleDecision::Deny, "rm -rf /")
        .unwrap();
    store
        .add_rule(&project, RuleScope::Session, RuleDecision::Allow, "cargo *")
        .unwrap();
    let rules = store.rules(&project);
    assert_eq!(rules.len(), 2);
    assert!(
        rules
            .iter()
            .any(|r| r.scope == RuleScope::Global && r.decision == RuleDecision::Deny)
    );
    assert!(
        rules
            .iter()
            .any(|r| r.scope == RuleScope::Session && r.decision == RuleDecision::Allow)
    );
    store.clear_session_rules();
    assert_eq!(store.rules(&project).len(), 1);
    let _ = std::fs::remove_dir_all(&root);
}

// Verifies: FR-PERM-28 (`-na` treats the project as untrusted for the
// process, ignoring stored trust until lifted)
#[test]
fn forced_distrust_ignores_stored_trust_until_lifted() {
    let root = scratch("force");
    let project = root.join("proj");
    std::fs::create_dir_all(&project).unwrap();
    let mut store = GrantStore::open(&root.join("grants.json")).unwrap();
    store.set_trusted(&project, true).unwrap();
    assert!(store.is_trusted_here(&project));
    store.set_force_untrusted(true);
    assert!(!store.is_trusted(&project), "stored trust ignored");
    assert!(!store.is_trusted_here(&project), "session view too");
    assert!(
        !store.is_allowed(&project, &shell(&project, "cargo build")),
        "workspace commands review again"
    );
    store.set_force_untrusted(false);
    assert!(store.is_trusted_here(&project), "lifting restores trust");
    let _ = std::fs::remove_dir_all(&root);
}

// Verifies: FR-PERM-28 (a session refusal suppresses the ask without
// touching stored state)
#[test]
fn a_session_refusal_suppresses_the_ask() {
    let root = scratch("refuse");
    let project = root.join("proj");
    let other = root.join("other");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(&other).unwrap();
    let mut store = GrantStore::open(&root.join("grants.json")).unwrap();
    assert!(!store.is_refused_for_session(&project));
    store.distrust_for_session(&project);
    assert!(store.is_refused_for_session(&project));
    assert!(
        !store.is_refused_for_session(&other),
        "scoped to the project"
    );
    assert!(!store.is_trusted_here(&project), "still untrusted");
    let _ = std::fs::remove_dir_all(&root);
}
