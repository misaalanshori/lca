//! Permission emission (gh #258 §3C, acceptance 4): rule matching,
//! trust, and grant decisions emit under the feature. Feature-gated:
//! without `debug-trace` there is no subscriber to capture with.

#![cfg(feature = "debug-trace")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code

use lca_permissions::{Action, Decision, GrantStore, PermissionPrompt, ProposalDiff};

struct QuietPrompt;

impl PermissionPrompt for QuietPrompt {
    fn ask(&mut self, _action: &Action) -> Decision {
        Decision::Denied
    }

    fn review_proposals(&mut self, _diff: &ProposalDiff) -> bool {
        true
    }
}

fn capture(filter: &str, run: impl FnOnce()) -> (String, String) {
    let root = lca_testkit::scratch_path("debug-perm-emit");
    let paths = lca_debug::DebugPaths::in_dir(&root);
    let config = lca_debug::DebugConfig {
        paths: paths.clone(),
        filter: Some(filter.to_string()),
    };
    let subscriber = lca_debug::subscriber(&config).expect("subscriber");
    tracing::subscriber::with_default(subscriber, run);
    (
        std::fs::read_to_string(&paths.log).unwrap_or_default(),
        std::fs::read_to_string(&paths.wire).unwrap_or_default(),
    )
}

fn read_action(project: &std::path::Path, name: &str) -> Action {
    Action::ReadPath {
        path: project.join(name),
    }
}

// Verifies: gh #258 - the evaluation pipeline emits its path
// (deny, cut, sensitive-escalation) with the action shape.
#[test]
fn evaluation_pipeline_emits_its_path() {
    let root = lca_testkit::scratch_path("debug-perm-pipe");
    let project = root.join("project");
    std::fs::create_dir_all(&project).expect("mkdir");
    let (log, wire) = capture("lca_permissions=trace", || {
        let mut store = GrantStore::open(&root.join("grants.json")).expect("open");
        let mut prompt = QuietPrompt;
        // The fatigue cut.
        let _ = lca_permissions::authorize(
            &mut store,
            &project,
            &read_action(&project, "src/main.rs"),
            None,
            &mut prompt,
            None,
        );
        // The sensitive escalation.
        let _ = lca_permissions::authorize(
            &mut store,
            &project,
            &read_action(&project, ".env"),
            None,
            &mut prompt,
            None,
        );
        // A deny rule.
        store
            .add_rule(
                &project,
                lca_permissions::RuleScope::Project,
                lca_permissions::RuleDecision::Deny,
                "*.pem",
            )
            .expect("deny");
        let _ = lca_permissions::authorize(
            &mut store,
            &project,
            &read_action(&project, "server.pem"),
            None,
            &mut prompt,
            None,
        );
    });
    for event in ["read.cut", "sensitive.ask", "rule.deny"] {
        assert!(log.contains(event), "pipeline emits {event}:\n{log}");
    }
    assert!(wire.is_empty(), "permissions never touch the wire file");
}

// Verifies: gh #258 - yolo approvals emit their pattern synthesis.
#[test]
fn yolo_approvals_emit() {
    let root = lca_testkit::scratch_path("debug-perm-yolo");
    let project = root.join("project");
    std::fs::create_dir_all(&project).expect("mkdir");
    let (log, _) = capture("lca_permissions=trace", || {
        let mut store = GrantStore::open(&root.join("grants.json")).expect("open");
        store.set_permission_mode(lca_permissions::PermissionMode::Yolo);
        let mut prompt = QuietPrompt;
        let _ = lca_permissions::authorize(
            &mut store,
            &project,
            &Action::Shell {
                command: "cargo test".into(),
                cwd: project.clone(),
            },
            None,
            &mut prompt,
            None,
        );
    });
    assert!(log.contains("yolo.approve"), "yolo emits:\n{log}");
}
