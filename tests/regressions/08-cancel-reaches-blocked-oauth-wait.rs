//! Released defect (0.1.3): `oauth_await` blocked in one long
//! `mpsc::recv_timeout`, so the host's cancellation flag was never consulted
//! and a user cancel waited out the whole callback window instead of the
//! NFR-21 budget. The wait now polls `Capabilities::cancel` in short slices.
//! This file guards the user-visible half (a blocked wait returns promptly
//! after a cancel); the WASM epoch path is guarded by
//! `crates/lca-ext-host/tests/provider/identity.rs`
//! (`interrupting_a_blocked_oauth_wait_returns_promptly`).
//!
//! Verifies: NFR-21, FR-CONC-1.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use lca_permissions::{
    Decision, GrantStore, OAuthSettings, PermissionPrompt, ProposalDiff, ScopeRoots,
};
use lca_tools::{Capabilities, CapabilityGrants};

struct Allow;

impl PermissionPrompt for Allow {
    fn ask(&mut self, _action: &lca_permissions::Action) -> Decision {
        Decision::Always
    }
    fn review_proposals(&mut self, _diff: &ProposalDiff) -> bool {
        false
    }
}

struct Sandbox {
    root: std::path::PathBuf,
}

impl Sandbox {
    fn new(name: &str) -> Sandbox {
        let root = lca_testkit::scratch_path(&format!("regression-oauth-cancel-{name}"));
        let _ = std::fs::remove_dir_all(&root);
        for dir in ["workspace", "private", "config", "tmp", "data"] {
            std::fs::create_dir_all(root.join(dir)).expect("mkdir");
        }
        Sandbox { root }
    }

    fn caps(&self) -> Capabilities {
        Capabilities::new(
            "probe",
            CapabilityGrants {
                oauth: Some(OAuthSettings {
                    redirect_path: "/callback".to_string(),
                    timeout_seconds: 30,
                }),
                ..CapabilityGrants::default()
            },
            ScopeRoots {
                workspace: self.root.join("workspace"),
                private: self.root.join("private"),
                home_config: self.root.join("config"),
                temp: self.root.join("tmp"),
                state_dir: self.root.join("data"),
            },
            Arc::new(Mutex::new(Allow)),
            Arc::new(Mutex::new(
                GrantStore::open(&self.root.join("grants.json")).expect("store"),
            )),
            self.root.join("workspace"),
            None,
        )
    }
}

/// The window the fixture configures, quoted in the failure message so the two
/// outcomes are distinguishable at a glance.
const CAPS_TIMEOUT_SECS: u64 = 30;

#[test]
fn cancelling_a_blocked_oauth_wait_returns_within_the_budget() {
    let sandbox = Sandbox::new("cancel");
    let caps = Arc::new(sandbox.caps());
    let (_url, handle) = caps.oauth_begin("/callback").expect("begin");
    let waiter = caps.clone();
    let join = std::thread::spawn(move || waiter.oauth_await(handle));
    // Let the wait actually block in the receive, then cancel from here.
    std::thread::sleep(Duration::from_millis(150));
    caps.cancel();
    let result = join.join().expect("the waiter thread joins");
    assert!(result.is_err(), "a cancelled wait reports an error");
    // Gh #609: the claim is **which** thing ended the wait, and the product says it in
    // words. A cancelled flow reports `oauth flow N cancelled`; a flow that nobody
    // cancelled ends on its own window with `no callback within 30s`. So the assertion
    // is the message, not a stopwatch - the two outcomes are a load-independent
    // difference, while the `< 2 s` this replaces was a claim about the scheduler (and
    // the defect it guards only shows after the **30 s** window, which is the bound
    // that matters). A wait that ignored the cancel reddens here 30 s later with the
    // timeout's own sentence.
    let message = result.expect_err("an error").to_string();
    assert!(
        message.contains("cancelled"),
        "the wait ended because of the cancel, not the {}s window: {message}",
        CAPS_TIMEOUT_SECS
    );
}
