//! Sensitive-read escalation (gh #215): filename patterns whose reads
//! prompt even in-workspace, amending FR-PERM-27's fatigue cut.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code

use std::path::Path;

use lca_permissions::sensitive::is_sensitive;

fn sensitive(name: &str) -> bool {
    is_sensitive(Path::new(name))
}

// Verifies: gh #215 - credential filenames escalate.
#[test]
fn secret_filenames_are_sensitive() {
    for name in [
        ".env",
        ".env.local",
        "prod.env",
        ".envrc",
        "server.pem",
        "server.key",
        "bundle.pkcs12",
        "bundle.pfx",
        "bundle.p12",
        "id_rsa",
        "id_rsa.pub",
        "id_ed25519",
        "id_ecdsa",
        "id_dsa",
        "credentials.json",
        "service-account-prod.json",
        ".npmrc",
        ".pypirc",
        ".netrc",
    ] {
        assert!(sensitive(name), "{name} escalates");
    }
}

// Verifies: gh #215 - safe templates stay silent.
#[test]
fn safe_templates_and_ordinary_files_stay_silent() {
    for name in [
        ".env.example",
        ".env.sample",
        ".env.template",
        ".env.dist",
        "src/main.rs",
        "Cargo.toml",
        "README.md",
        "my_id_rsa",
        "mycredentials.json",
        "setup.cfg",
    ] {
        assert!(!sensitive(name), "{name} stays silent");
    }
}

use std::path::PathBuf;

use lca_permissions::{Action, Decision, GrantStore, PermissionPrompt, ProposalDiff};

struct ScriptedPrompt {
    answers: Vec<Decision>,
    asked: usize,
}

impl PermissionPrompt for ScriptedPrompt {
    fn ask(&mut self, _action: &Action) -> Decision {
        self.asked += 1;
        self.answers.pop().unwrap_or(Decision::Denied)
    }

    fn review_proposals(&mut self, _diff: &ProposalDiff) -> bool {
        true
    }
}

fn decide(project: &std::path::Path, name: &str, answers: Vec<Decision>) -> (bool, usize, bool) {
    let root = project
        .ancestors()
        .nth(1)
        .expect("scratch layout")
        .to_path_buf();
    let mut store = GrantStore::open(&root.join("grants.json")).expect("open");
    let mut prompt = ScriptedPrompt { answers, asked: 0 };
    let action = Action::ReadPath {
        path: project.join(name),
    };
    let outcome = lca_permissions::authorize(&mut store, project, &action, None, &mut prompt, None)
        .expect("authorize");
    (outcome.allowed(), prompt.asked, outcome.prompted)
}

fn scratch_project(name: &str) -> PathBuf {
    let project = lca_testkit::scratch_path(name).join("project");
    std::fs::create_dir_all(&project).expect("mkdir");
    project
}

// Verifies: gh #215 - a sensitive in-workspace read prompts (the
// fatigue cut no longer covers it); an ordinary read stays silent.
#[test]
fn sensitive_reads_prompt_ordinary_reads_stay_silent() {
    let project = scratch_project("sensitive-ask");
    let (allowed, asked, prompted) = decide(&project, ".env", vec![Decision::Once]);
    assert!(allowed && prompted && asked == 1, ".env prompts");
    let (allowed, asked, _) = decide(&project, "src/main.rs", vec![]);
    assert!(allowed && asked == 0, "ordinary reads stay silent");
    let (allowed, asked, _) = decide(&project, ".env.example", vec![]);
    assert!(allowed && asked == 0, "safe templates stay silent");
}

// Verifies: gh #215 - an explicit deny rule still refuses a sensitive
// read silently (no prompt); an explicit allow rule still permits it.
#[test]
fn deny_rules_win_allow_rules_permit() {
    let project = scratch_project("sensitive-rules");
    let root = project.ancestors().nth(1).expect("layout").to_path_buf();
    let mut store = GrantStore::open(&root.join("grants.json")).expect("open");
    store
        .add_rule(
            &project,
            lca_permissions::RuleScope::Project,
            lca_permissions::RuleDecision::Deny,
            "*.env",
        )
        .expect("deny rule");
    let mut prompt = ScriptedPrompt {
        answers: vec![],
        asked: 0,
    };
    let outcome = lca_permissions::authorize(
        &mut store,
        &project,
        &Action::ReadPath {
            path: project.join(".env"),
        },
        None,
        &mut prompt,
        None,
    )
    .expect("authorize");
    assert!(
        !outcome.allowed() && prompt.asked == 0,
        "deny refuses silently"
    );

    store
        .add_rule(
            &project,
            lca_permissions::RuleScope::Project,
            lca_permissions::RuleDecision::Allow,
            "*.pem",
        )
        .expect("allow rule");
    let outcome = lca_permissions::authorize(
        &mut store,
        &project,
        &Action::ReadPath {
            path: project.join("server.pem"),
        },
        None,
        &mut prompt,
        None,
    )
    .expect("authorize");
    assert!(outcome.allowed() && prompt.asked == 0, "allow permits");
}

// Verifies: gh #215 - folder trust never covers credential filenames:
// a sensitive in-workspace read prompts even in a trusted project.
#[test]
fn trust_does_not_cover_sensitive_reads() {
    let project = scratch_project("sensitive-trust");
    let root = project.ancestors().nth(1).expect("layout").to_path_buf();
    let mut store = GrantStore::open(&root.join("grants.json")).expect("open");
    store.set_trusted(&project, true).expect("trust");
    assert!(
        GrantStore::open(&root.join("grants.json"))
            .expect("reopen")
            .is_trusted(&project),
        "project is trusted"
    );
    let (allowed, asked, prompted) = decide(&project, ".env", vec![Decision::Once]);
    assert!(
        allowed && prompted && asked == 1,
        "trust does not silence .env"
    );
}
