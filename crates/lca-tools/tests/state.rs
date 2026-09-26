//! The `state` bag (ADR-0030): round-trip across instances, identity
//! namespaces, key safety, and the size caps.
//!
//! Verifies: FR-PERM-6, FR-PERM-7.

use std::sync::{Arc, Mutex};

use lca_permissions::{Decision, GrantStore, PermissionPrompt, ProposalDiff, ScopeRoots};
use lca_protocol::CapabilityError;
use lca_tools::{Capabilities, CapabilityGrants, STATE_VALUE_MAX_BYTES};

struct Deny;

impl PermissionPrompt for Deny {
    fn ask(&mut self, _action: &lca_permissions::Action) -> Decision {
        Decision::Denied
    }
    fn review_proposals(&mut self, _diff: &ProposalDiff) -> bool {
        false
    }
}

fn engine(name: &str, root: &std::path::Path) -> Capabilities {
    Capabilities::new(
        name,
        CapabilityGrants::default(),
        ScopeRoots {
            workspace: root.join("project"),
            private: root.join("private"),
            home_config: root.join("config"),
            temp: root.join("tmp"),
            state_dir: root.join("data"),
        },
        Arc::new(Mutex::new(Deny)),
        Arc::new(Mutex::new(
            GrantStore::open(&root.join("grants.json")).expect("store"),
        )),
        root.join("project"),
        None,
    )
}

#[test]
fn state_persists_across_instances_and_is_namespaced() {
    let root = lca_testkit::scratch_path("state-roundtrip");
    {
        let cap = engine("alpha", &root);
        assert_eq!(cap.state_read("last-model").expect("read"), None);
        cap.state_write("last-model", b"gpt-4o").expect("write");
        assert_eq!(
            cap.state_read("last-model").expect("read").as_deref(),
            Some(&b"gpt-4o"[..])
        );
        assert_eq!(
            cap.state_list().expect("list"),
            vec![("last-model".to_string(), 6)]
        );
    }

    // A fresh instance - the trap-isolation rule gives every call a new
    // instance - still sees the data.
    let cap = engine("alpha", &root);
    assert_eq!(
        cap.state_read("last-model").expect("read").as_deref(),
        Some(&b"gpt-4o"[..])
    );

    // Another extension has its own namespace: the same key reads empty.
    let other = engine("beta", &root);
    assert_eq!(other.state_read("last-model").expect("read"), None);

    // A key that tries to carry a path is refused and recorded.
    assert!(matches!(
        cap.state_write("../escape", b"x").unwrap_err(),
        CapabilityError::Invalid(_)
    ));
    assert!(
        cap.denials()
            .iter()
            .any(|denial| denial.capability == "state"),
        "the bad key is recorded for `lca ext info`"
    );

    // Delete is idempotent.
    cap.state_delete("last-model").expect("delete");
    cap.state_delete("last-model").expect("delete again");
    assert_eq!(cap.state_read("last-model").expect("read"), None);
}

#[test]
fn a_state_value_over_the_cap_is_refused() {
    let root = lca_testkit::scratch_path("state-cap");
    let cap = engine("alpha", &root);
    let big = vec![0u8; STATE_VALUE_MAX_BYTES as usize + 1];
    let err = cap.state_write("big", &big).unwrap_err();
    assert!(
        matches!(err, CapabilityError::Invalid(ref detail) if detail.contains("cap")),
        "over the cap: {err:?}"
    );
}
