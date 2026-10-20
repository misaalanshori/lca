//! gh #355 (NFR-24): the self-update fetch paths are bounded in time and size.
//!
//! `latest_release()` and `install_binary()` called `plain_get` and inherited
//! "hang forever, allocate everything": a stalled release feed hung
//! `lca update --check` and `--changelog` indefinitely, and a hostile mirror
//! serving a multi-gigabyte installer body exhausted memory *before* the
//! installer's verify-then-move path could run. The catalog fetch was the same
//! shape, one more caller of the same function.
//!
//! These rows drive the **shipped** `lca_registry::plain_get` — the production
//! constants, not a test-injected deadline — against local hostile servers, so
//! a regression in the default bound fails here rather than only in a unit row
//! that supplies its own numbers.
//!
//! A black-hole host is simulated by accepting the connection and never
//! answering. Real time is unavoidable for the deadline row: the assertion is
//! that it finishes inside a wall-clock window far below "forever", which is
//! the property that was broken.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Accept connections and never write a byte back.
async fn black_hole() -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        // Hold every accepted stream open for the life of the test: the point
        // is a host that says nothing, not one that refuses.
        let mut held = Vec::new();
        while let Ok((stream, _)) = listener.accept().await {
            held.push(stream);
        }
    });
    addr
}

// Verifies: gh #355 acceptance 1 and 2 - a caller of `plain_get` can no longer
// hang.
//
// This row deliberately hits the **production** constants: it calls
// `lca_registry::plain_get` (which is `PLAIN_GET_TIMEOUT` / `PLAIN_GET_MAX_BYTES`,
// no test override) against a local hostile host. The live `lca update --check`
// drive cannot be pointed at a black hole — the release URL is a hardcoded
// const and the registry client honors no proxy variable — so this row is what
// pins the shipped default rather than a number a test supplied. The production deadline is [`lca_registry::PLAIN_GET_TIMEOUT`]; this
// row asserts the fetch finishes inside a generous multiple of it rather than
// waiting for the exact second, because the property under test is "bounded at
// all", and a tight equality would be a flaky clock assertion.
#[tokio::test]
async fn the_update_feed_cannot_hang_a_caller() {
    let addr = black_hole().await;
    let started = Instant::now();
    let err = lca_registry::plain_get(&format!("http://{addr}/releases/latest"))
        .await
        .expect_err("a black-hole host must not answer");
    let elapsed = started.elapsed();
    assert!(
        elapsed <= lca_registry::PLAIN_GET_TIMEOUT + Duration::from_secs(15),
        "the shipped deadline bounded the fetch: it returned after {elapsed:?}"
    );
    let text = err.to_string();
    assert!(
        text.contains(&addr.to_string()),
        "the error names the host that did not answer: {text}"
    );
    assert!(
        text.contains("did not answer") || text.contains("timed out"),
        "the error says what happened, not a transport code: {text}"
    );
}

// Verifies: gh #355 acceptance 1 - a caller can no longer allocate unboundedly.
// The server streams far past the shipped cap; the fetch must refuse, and the
// count of bytes the server managed to write must stay below what an
// unbounded collect would have drained.
#[tokio::test]
async fn an_installer_body_over_the_cap_is_refused_while_reading() {
    use tokio::io::AsyncWriteExt;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    let written = Arc::new(AtomicU64::new(0));
    let counter = written.clone();
    let total: u64 = 256 * 1024 * 1024; // 256 MiB: 32x the shipped cap.
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let _ = stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n",
                )
                .await;
            let chunk = vec![b'x'; 64 * 1024];
            let mut sent: u64 = 0;
            while sent < total {
                let header = format!("{:x}\r\n", chunk.len());
                if stream.write_all(header.as_bytes()).await.is_err()
                    || stream.write_all(&chunk).await.is_err()
                    || stream.write_all(b"\r\n").await.is_err()
                {
                    // The client stopped reading: exactly what the cap should
                    // cause.
                    return;
                }
                sent += chunk.len() as u64;
                counter.store(sent, Ordering::Relaxed);
            }
            let _ = stream.write_all(b"0\r\n\r\n").await;
        }
    });
    let err = lca_registry::plain_get(&format!("http://{addr}/install.sh"))
        .await
        .expect_err("an oversized body must be refused");
    let text = err.to_string();
    assert!(
        text.contains(&addr.to_string()),
        "the refusal names the host: {text}"
    );
    assert!(
        text.contains("limit"),
        "the refusal names the limit: {text}"
    );
    let sent = written.load(Ordering::Relaxed);
    assert!(
        sent < total,
        "the read stopped before draining the body: the server wrote {sent} of {total} bytes"
    );
    // A collect-everything implementation reads the whole 256 MiB; the cap is
    // 8 MiB, so anything near the total is a regression.
    assert!(
        sent < 64 * 1024 * 1024,
        "the stop happened near the cap, not after megabytes of slack: {sent} bytes"
    );
}

// Verifies: gh #355 - a body declaring an oversized length is refused from the
// header, before any of it is read. This is the cheaper of the two checks and
// the one a plain mirror (which sends `Content-Length`) hits first.
#[tokio::test]
async fn a_declared_oversized_length_is_refused_from_the_header() {
    use tokio::io::AsyncWriteExt;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            // Declare 2 GiB, then send nothing at all: a client that trusts the
            // body would wait; one that checks the header refuses immediately.
            let _ = stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 2147483648\r\nconnection: close\r\n\r\n",
                )
                .await;
            // Hold the connection so a body-reading client hangs.
            tokio::time::sleep(Duration::from_secs(60)).await;
        }
    });
    let started = Instant::now();
    let err = lca_registry::plain_get(&format!("http://{addr}/install.sh"))
        .await
        .expect_err("a 2 GiB declared body must be refused");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "the header check refused without waiting for the body: {:?}",
        started.elapsed()
    );
    assert!(
        err.to_string().contains("limit"),
        "the refusal names the limit: {err}"
    );
}

// Verifies: gh #355 - the bounds did not break the ordinary case. A small body
// over the same code path still arrives intact (a fix that refuses everything
// would satisfy the hostile rows and be useless).
#[tokio::test]
async fn an_ordinary_feed_still_arrives() {
    use tokio::io::AsyncWriteExt;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let payload = br#"{"tag_name":"phase6-0.2.0"}"#;
            let head = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                payload.len()
            );
            let _ = stream.write_all(head.as_bytes()).await;
            let _ = stream.write_all(payload).await;
        }
    });
    let body = lca_registry::plain_get(&format!("http://{addr}/releases/latest"))
        .await
        .expect("an ordinary feed arrives");
    assert_eq!(body, br#"{"tag_name":"phase6-0.2.0"}"#);
}
