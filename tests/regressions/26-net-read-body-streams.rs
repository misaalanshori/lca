//! Regression: the `net` body reader buffered a streaming response until
//! `max` bytes or EOF, so an SSE provider stream painted only at the end
//! (owner issue #4: no incremental streaming). Found by driving the real
//! binary against a slow mock in tmux.
//!
//! The reader must return the first data frame promptly, not wait for the
//! body to finish. `net_read_body` now breaks after the first data frame.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lca_permissions::{Decision, GrantStore, PermissionPrompt, ProposalDiff, ScopeRoots};
use lca_tools::{Capabilities, CapabilityGrants};

struct Allow;
impl PermissionPrompt for Allow {
    fn ask(&mut self, _: &lca_permissions::Action) -> Decision {
        Decision::Always
    }
    fn review_proposals(&mut self, _: &ProposalDiff) -> bool {
        false
    }
}

fn caps(name: &str) -> Capabilities {
    let root = lca_testkit::scratch_path(&format!("lca-regr-26-{name}"));
    let _ = std::fs::remove_dir_all(&root);
    for dir in ["workspace", "private", "config", "data", "tmp"] {
        std::fs::create_dir_all(root.join(dir)).expect("mkdir");
    }
    let grants = CapabilityGrants {
        net_local: vec![lca_permissions::parse_local_pattern("127.0.0.1").expect("pattern")],
        ..CapabilityGrants::default()
    };
    Capabilities::new(
        "probe",
        grants,
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
    )
}

/// A server that writes `first`, flushes, waits, writes `second`, closes.
fn slow_stream_server(first: &'static str, second: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    std::thread::spawn(move || {
        if let Some(mut stream) = listener.incoming().flatten().next() {
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            // No content-length; `connection: close` delimits the body at
            // EOF, so the reader sees each write as it arrives.
            let head =
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n";
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(first.as_bytes());
            let _ = stream.flush();
            std::thread::sleep(Duration::from_millis(1200));
            let _ = stream.write_all(second.as_bytes());
            let _ = stream.flush();
        }
    });
    format!("http://{addr}")
}

#[test]
fn the_net_body_reader_returns_the_first_frame_before_the_stream_ends() {
    let caps = caps("streams");
    let url = slow_stream_server("PART-ONE", "PART-TWO");
    let handle = caps
        .net_request("GET", &url, &[], None)
        .expect("the loopback grant reaches the server");

    let start = Instant::now();
    let first = caps
        .net_read_body(handle, 4096)
        .expect("first chunk")
        .expect("some bytes");
    let elapsed = start.elapsed();
    assert_eq!(String::from_utf8_lossy(&first), "PART-ONE");
    assert!(
        elapsed < Duration::from_millis(1000),
        "the reader waited for the whole body ({elapsed:?}) instead of \
         returning the first frame"
    );

    let mut rest = Vec::new();
    while let Some(chunk) = caps.net_read_body(handle, 4096).expect("chunk") {
        rest.extend_from_slice(&chunk);
    }
    assert_eq!(String::from_utf8_lossy(&rest), "PART-TWO");
}
