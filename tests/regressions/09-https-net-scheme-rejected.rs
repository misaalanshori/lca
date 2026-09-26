//! Released defect (0.1.1–0.1.3): ADR-0025's pinned-DNS connector wrapped a
//! caller-supplied `HttpConnector` without clearing `enforce_http`, so the
//! inner connector rejected every `https` request before TLS with "invalid
//! URL, scheme is not http". The mock-provider tests speak `http://127.0.0.1`
//! (which `enforce_http` allows), so CI never exercised an extension's HTTPS
//! path; the env-gated real-provider smoke is the only test that hit it.
//!
//! This guards the user-visible half: an extension's HTTPS `net` request
//! reaches the socket — here a closed loopback port, so it fails with a
//! connection error — instead of dying at the scheme check. The connector
//! itself is built in `crates/lca-tools/src/capabilities.rs`.
//!
//! Verifies: FR-PERM-11, FR-PERM-13, ADR-0025.

use std::sync::{Arc, Mutex};

use lca_permissions::{
    Decision, GrantStore, PermissionPrompt, ProposalDiff, ScopeRoots, parse_local_pattern,
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
        let root = lca_testkit::scratch_path(&format!("regression-https-scheme-{name}"));
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
fn an_https_net_request_reaches_the_socket_not_the_scheme_check() {
    let sandbox = Sandbox::new("scheme");
    let caps = sandbox.caps(CapabilityGrants {
        net_local: vec![parse_local_pattern("127.0.0.1").expect("loopback pattern")],
        ..CapabilityGrants::default()
    });
    // Bind to reserve a loopback port, then drop the listener so the connect
    // is refused immediately. The defect failed before the socket, naming the
    // scheme; the fix fails at the connection.
    let port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        listener.local_addr().expect("local addr").port()
    };
    let err = caps
        .net_request("GET", &format!("https://127.0.0.1:{port}/"), &[], None)
        .expect_err("the closed port refuses the connection");
    let text = err.to_string();
    assert!(
        !text.contains("scheme is not http"),
        "an extension's HTTPS request must reach the socket, got: {text}"
    );
}
