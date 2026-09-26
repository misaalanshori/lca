//! Finding 4 (high, `../lca-issues.md`): the production capability engines
//! were built with a deny-everything prompt, so an extension's own
//! `process`/`pty` call auto-denied in the TUI and never reached the user.
//! The fix introduced a swappable `SharedPrompt` that the interface installs
//! its modal into. This file pins the engine-level mechanism (the engine
//! consults its shared prompt instead of denying); the CLI-side wiring lives
//! in `extension_capabilities`, where a regression would need a terminal and
//! is covered by `lca-cli`'s own suite.
//!
//! Verifies: the capability catalog's process/pty consent path (defect 4).

use std::sync::{Arc, Mutex};

use lca_permissions::{
    Decision, FsMode, GrantStore, PermissionPrompt, ProposalDiff, ScopeGrant, ScopeRoots,
    SharedPrompt,
};
use lca_protocol::CapabilityError;
use lca_tools::{Capabilities, CapabilityGrants};

struct Recording {
    asked: Arc<Mutex<Vec<String>>>,
}

impl PermissionPrompt for Recording {
    fn ask(&mut self, action: &lca_permissions::Action) -> Decision {
        self.asked.lock().expect("asked").push(action.display());
        Decision::Denied
    }

    fn review_proposals(&mut self, _diff: &ProposalDiff) -> bool {
        false
    }
}

#[test]
fn an_extension_process_call_asks_the_shared_prompt() {
    let root = lca_testkit::scratch_path("regression-prompt");
    let _ = std::fs::remove_dir_all(&root);
    let workspace = root.join("workspace");
    for dir in [
        &workspace,
        &root.join("private"),
        &root.join("config"),
        &root.join("tmp"),
        &root.join("data"),
    ] {
        std::fs::create_dir_all(dir).expect("mkdir");
    }

    let asked = Arc::new(Mutex::new(Vec::new()));
    let shared = SharedPrompt::default();
    shared.set(Arc::new(Mutex::new(Recording {
        asked: asked.clone(),
    })));
    let caps = Capabilities::new(
        "probe",
        CapabilityGrants {
            fs: vec![ScopeGrant::parse("workspace", FsMode::ReadWrite).expect("grant")],
            fs_declared: true,
            process: true,
            ..CapabilityGrants::default()
        },
        ScopeRoots {
            workspace: workspace.clone(),
            private: root.join("private"),
            home_config: root.join("config"),
            temp: root.join("tmp"),
            state_dir: root.join("data"),
        },
        Arc::new(Mutex::new(shared)),
        Arc::new(Mutex::new(
            GrantStore::open(&root.join("grants.json")).expect("store"),
        )),
        workspace.clone(),
        None,
    );

    // A binary that cannot exist: the prompt is consulted before the spawn,
    // so the outcome is the prompt's denial, not a spawn failure.
    let err = caps
        .process_spawn("lca-no-such-binary", &[], "workspace")
        .expect_err("denied by the prompt");
    assert!(matches!(err, CapabilityError::Permission(_)), "{err:?}");
    assert_eq!(
        asked.lock().expect("asked").len(),
        1,
        "the prompt was asked, not silently denied"
    );
}
