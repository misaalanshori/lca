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

// CHARACTERIZATION - the shape ADR-0035 changes. `provider_models` takes
// no settings today, so the discovered list can only come from the
// extension's own store; `complete` by contrast receives the host-persisted
// settings on every call in its request `extras`. That asymmetry is what
// D2 fixes: `list-models` gains the same settings flow.
#[test]
fn list_models_takes_no_settings_so_the_store_is_its_only_source() {
    let cap = sandbox("characterize");
    cap.credentials_set("api_key", "sk-x").expect("key");
    // What `login-submit` handed the host to persist (ADR-0033).
    cap.credentials_set("models", "store-a,store-b")
        .expect("models");

    let handle = openai_compatible::OpenAiCompat::new(cap.clone());
    let offered: Vec<String> = handle
        .provider_models()
        .expect("models")
        .into_iter()
        .map(|model| model.id)
        .collect();
    assert!(
        offered.iter().any(|id| id == "store-a"),
        "PRE-CHANGE SHAPE: the only way to see a discovered list is the \
         extension's own store, because `list-models` receives nothing. \
         Once ADR-0035 lands, this becomes the passed settings and this \
         test moves with it - do not delete it. Offered: {offered:?}"
    );
    let _ = std::fs::remove_dir_all(lca_testkit::scratch_path("lca-models-characterize"));
}
