//! Wire emission (gh #258 §3D, acceptance 6): outbound requests
//! log method, redacted URL, status, and latency — never headers,
//! bodies, or secrets. Feature-gated.

#![cfg(feature = "debug-trace")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code
use std::net::TcpListener as StdTcpListener;
use std::sync::{Arc, Mutex};

use lca_permissions::{GrantStore, PermissionPrompt, ProposalDiff, ScopeRoots};
use lca_tools::{Capabilities, CapabilityGrants};

struct Allow;
impl PermissionPrompt for Allow {
    fn ask(&mut self, _: &lca_permissions::Action) -> lca_permissions::Decision {
        lca_permissions::Decision::Always
    }
    fn review_proposals(&mut self, _: &ProposalDiff) -> bool {
        false
    }
}

fn sandbox(tag: &str) -> (std::path::PathBuf, Capabilities) {
    let root = lca_testkit::scratch_path(&format!("lca-wire-{tag}"));
    let _ = std::fs::remove_dir_all(&root);
    for dir in ["workspace", "private", "config", "data", "tmp"] {
        std::fs::create_dir_all(root.join(dir)).expect("mkdir");
    }
    let caps = Capabilities::new(
        "probe",
        CapabilityGrants {
            net_local: vec![lca_permissions::parse_local_pattern("127.0.0.1").expect("pattern")],
            ..CapabilityGrants::default()
        },
        ScopeRoots {
            workspace: root.join("workspace"),
            private: root.join("private"),
            home_config: root.join("config"),
            temp: root.join("tmp"),
            state_dir: root.join("data"),
        },
        Arc::new(Mutex::new(Allow)),
        Arc::new(Mutex::new(
            GrantStore::open(&root.join("grants.json")).expect("store"),
        )),
        root.join("workspace"),
        None,
    );
    (root, caps)
}

fn canned_server() -> (String, std::thread::JoinHandle<()>) {
    let listener = StdTcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let handle = std::thread::spawn(move || {
        if let Some(mut stream) = listener.incoming().flatten().next() {
            use std::io::{Read, Write};
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            let _ = stream
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nhi");
        }
    });
    (format!("http://{addr}"), handle)
}

// Verifies: gh #258 - a request emits method plus redacted URL, the
// response emits status plus latency, secrets never appear.
#[test]
fn requests_emit_redacted_responses_emit_status() {
    let root = lca_testkit::scratch_path("debug-net-emit");
    let paths = lca_debug::DebugPaths::in_dir(&root);
    let config = lca_debug::DebugConfig {
        paths: paths.clone(),
        filter: Some("lca_net=trace".to_string()),
    };
    let subscriber = lca_debug::subscriber(&config).expect("subscriber");
    tracing::subscriber::with_default(subscriber, || {
        let (_dir, caps) = sandbox("wire");
        let (base, server) = canned_server();
        let handle = caps
            .net_request(
                "GET",
                &format!("{base}/probe?token=hunter2supersecret&page=2"),
                &[("x-custom", "s3cr3t-value-long-enough")],
                Some(b"{\"password\":\"hunter2supersecret\"}"),
            )
            .expect("request");
        assert_eq!(caps.net_response_status(handle).expect("status"), 200);
        let _ = server.join();
    });
    let wire = std::fs::read_to_string(&paths.wire).unwrap_or_default();
    assert!(wire.contains("http.request"), "requests emit:\n{wire}");
    assert!(wire.contains("http.response"), "responses emit:\n{wire}");
    assert!(wire.contains("status=200"), "status lands:\n{wire}");
    assert!(!wire.contains("hunter2supersecret"), "secrets never emit");
    assert!(!wire.contains("s3cr3t-value"), "headers never emit");
    assert!(wire.contains("page=2"), "innocent parameters stay:\n{wire}");
}
