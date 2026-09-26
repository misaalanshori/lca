//! Characterization of `list-models`' **current** shape, written before
//! the D2 change so the change cannot move it silently.
//!
//! The asymmetry D2 names: `complete` receives the host-persisted settings
//! on every call (in its request `extras`), so the discovered model list -
//! which `login-submit` puts there - is visible to completion but not to
//! `list-models`, which reads only the environment. The fix makes the two
//! symmetric (ADR-0035). When it lands, this file changes with it, and the
//! two-mode parity tests take over.
//!
//! Verifies: ADR-0033 (the settings `login-submit` hands back), and pins
//! the shape ADR-0035 is about to change.

use std::sync::Arc;

use lca_ext_abi::ExtensionDispatch;
use lca_permissions::{Decision, GrantStore, PermissionPrompt, ProposalDiff, ScopeRoots};

struct Always;
impl PermissionPrompt for Always {
    fn ask(&mut self, _action: &lca_permissions::Action) -> Decision {
        Decision::Always
    }
    fn review_proposals(&mut self, _diff: &ProposalDiff) -> bool {
        false
    }
}

fn sandbox(name: &str) -> Arc<lca_tools::Capabilities> {
    let root = lca_testkit::scratch_path(&format!("lca-models-{name}"));
    let _ = std::fs::remove_dir_all(&root);
    for dir in ["project", "private", "config", "data", "tmp"] {
        std::fs::create_dir_all(root.join(dir)).expect("mkdir");
    }
    let roots = ScopeRoots {
        workspace: root.join("project"),
        private: root.join("private"),
        home_config: root.join("config"),
        temp: root.join("tmp"),
        state_dir: root.join("data"),
    };
    let mut engine = lca_tools::Capabilities::new(
        "openai-compatible",
        openai_compatible::manifest_grants(),
        roots,
        Arc::new(std::sync::Mutex::new(Always)),
        Arc::new(std::sync::Mutex::new(
            GrantStore::open(&root.join("grants.json")).expect("open"),
        )),
        root.join("project"),
        None,
    );
    engine.set_resources(openai_compatible::resources());
    Arc::new(engine)
}

// The shape ADR-0035 settled. `list-models` takes the same settings flow
// `complete` does, so the discovered model list - which `login-submit`
// hands the host to persist - is visible to both, from one source, with no
// second store. This is the characterization test from before the change,
// moved onto the new shape rather than deleted.
#[test]
fn list_models_reads_the_passed_settings_not_a_store_of_its_own() {
    let cap = sandbox("settings");
    cap.credentials_set("api_key", "sk-x").expect("key");
    // What `login-submit` handed the host to persist (ADR-0033).
    cap.credentials_set("models", "store-a,store-b")
        .expect("models");

    let handle = openai_compatible::OpenAiCompat::new(cap.clone());
    // With no settings passed, the extension cannot know better than the
    // fallback it can reach.
    let fallback: Vec<String> = handle
        .provider_models(&[])
        .expect("models")
        .into_iter()
        .map(|model| model.id)
        .collect();
    assert!(fallback.iter().any(|id| id == "store-a"), "{fallback:?}");

    // The passed settings are the source of truth and override anything the
    // extension might have stored: this is the symmetry with `complete`.
    let passed = [("models".to_string(), "passed-x,passed-y".to_string())];
    let offered: Vec<String> = handle
        .provider_models(&passed)
        .expect("models")
        .into_iter()
        .map(|model| model.id)
        .collect();
    assert!(
        offered.iter().any(|id| id == "passed-x") && offered.iter().any(|id| id == "passed-y"),
        "the passed list is what is offered: {offered:?}"
    );
    assert!(
        !offered.iter().any(|id| id == "store-a"),
        "the passed settings override the store rather than merging with it: {offered:?}"
    );
    let _ = std::fs::remove_dir_all(lca_testkit::scratch_path("lca-models-settings"));
}
