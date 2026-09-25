//! Finding 7 (high, `../lca-issues.md`): DNS-rebinding protection was a
//! time-of-check/time-of-use hole - the host resolved and checked the
//! address, then handed the URL to a connector that resolved again. ADR-0025
//! pins the checked address so the connect uses it. This file guards the
//! user-visible half (a `net` hostname resolving into a local range is
//! refused and recorded as a rebinding case); the pinning itself is guarded
//! by the unit test `a_pinned_host_resolves_to_the_checked_address` beside
//! the resolver in `crates/lca-tools/src/capabilities.rs`, which cannot move
//! here because the resolver is private.
//!
//! Verifies: FR-PERM-13, ADR-0025 (defect 7).

use std::sync::{Arc, Mutex};

use lca_permissions::{GrantStore, PermissionPrompt, ProposalDiff, ScopeRoots};
use lca_tools::{Capabilities, CapabilityGrants};

struct Allow;

impl PermissionPrompt for Allow {
    fn ask(&mut self, _action: &lca_permissions::Action) -> lca_permissions::Decision {
        lca_permissions::Decision::Always
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
        let root = std::env::temp_dir().join(format!(
            "lca-regression-rebind-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        for dir in ["workspace", "private", "config", "tmp", "data"] {
            std::fs::create_dir_all(root.join(dir)).expect("mkdir");
        }
        Sandbox { root }
    }

    fn caps(&self, grants: CapabilityGrants) -> Capabilities {
        Capabilities::new(
            "probe",
            grants,
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

#[test]
fn net_refuses_local_resolution_as_rebinding() {
    let sandbox = Sandbox::new("rebind");
    let caps = sandbox.caps(CapabilityGrants {
        net: vec![lca_permissions::parse_net_pattern("localhost").expect("pattern")],
        ..CapabilityGrants::default()
    });
    let err = caps
        .net_request("GET", "https://localhost/x", &[], None)
        .expect_err("refused");
    let text = err.to_string();
    assert!(text.contains("rebinding"), "distinct reason: {text}");
    assert_eq!(caps.denial_count(), 1);
    assert!(
        caps.denials()[0].reason.contains("rebinding"),
        "recorded as a rebinding case: {}",
        caps.denials()[0].reason
    );
}
