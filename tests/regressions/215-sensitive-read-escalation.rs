//! GitHub #215: credential filenames read without prompting under
//! FR-PERM-27's fatigue cut, so an untrusted prompt could exfiltrate
//! `.env` and private keys to a provider with no confirmation.
//! Direction (brief): sensitive reads prompt even in-workspace; deny
//! still wins silently; yolo auto-approves with a log notice.
//!
//! Verifies: end to end through `authorize` — a sensitive read opens
//! the prompt, an ordinary read does not, and a deny rule refuses
//! silently.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

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

// Verifies: gh #215 - the fatigue cut no longer covers credential
// filenames (today `.env` authorizes silently).
#[test]
fn sensitive_in_workspace_reads_prompt() {
    let root = lca_testkit::scratch_path("gh215-sensitive");
    let project = root.join("project");
    std::fs::create_dir_all(&project).expect("mkdir");
    let mut store = GrantStore::open(&root.join("grants.json")).expect("open");

    let mut prompt = ScriptedPrompt {
        answers: vec![Decision::Once],
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
    assert!(outcome.allowed() && outcome.prompted, ".env prompts");
    assert_eq!(prompt.asked, 1);

    let outcome = lca_permissions::authorize(
        &mut store,
        &project,
        &Action::ReadPath {
            path: project.join("src/main.rs"),
        },
        None,
        &mut prompt,
        None,
    )
    .expect("authorize");
    assert!(outcome.allowed(), "ordinary reads pass");
    assert_eq!(prompt.asked, 1, "ordinary reads stay silent");
}
