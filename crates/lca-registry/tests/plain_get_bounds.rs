//! gh #355: `plain_get` is bounded in time and size.
//!
//! Split out of `registry.rs` when the additions crossed the 1,200-line
//! ceiling (gate 11). The rows drive the narrowest seam — `plain_get_within`
//! and `plain_get_within_capped` — so the production `plain_get` is the same
//! code path with the shipped constants, and a test that exercises the timeout
//! exercises what ships. The shipped constants themselves are pinned in
//! `tests/regressions/355-update-fetch-is-bounded.rs`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

// Verifies: gh #355 - `plain_get` is bounded in *time*. A server that accepts
// the connection and then never answers must not hang the caller: the fetch
// fails inside the bound with an error naming the host.
//
// The production bound is 30 s, which no unit test should wait out, so the
// narrowest seam is used: `plain_get_within` takes the deadline explicitly and
// `plain_get` is its 30-second wrapper. That keeps one code path and makes the
// timeout observable at a test's speed.
#[tokio::test]
async fn plain_get_gives_up_on_a_black_hole_host() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    // Accept and hold the connection open, answering nothing at all.
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((stream, _)) = listener.accept().await {
            held.push(stream);
        }
    });
    let started = std::time::Instant::now();
    let err = lca_registry::plain_get_within(
        &format!("http://{addr}/latest"),
        std::time::Duration::from_millis(150),
    )
    .await
    .expect_err("a black hole must not answer");
    let elapsed = started.elapsed();
    assert!(
        elapsed < std::time::Duration::from_secs(10),
        "the fetch gave up at its bound, not later: {elapsed:?}"
    );
    let text = err.to_string();
    assert!(
        text.contains(&addr.to_string()),
        "the error names the host it could not reach: {text}"
    );
    assert!(
        text.contains("timed out") || text.contains("did not answer"),
        "the error says what happened: {text}"
    );
}

// Verifies: gh #355 - `plain_get` is bounded in *size*. A server streaming far
// past the cap must be refused **while reading**, not after collecting the
// whole body into memory. The row proves the stop happened early by counting
// what the server managed to write: a collecting implementation drains
// everything, a bounded one stops at the cap.
#[tokio::test]
async fn plain_get_refuses_an_oversized_body_while_reading() {
    use std::sync::atomic::{AtomicU64, Ordering};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    let written = std::sync::Arc::new(AtomicU64::new(0));
    let counter = written.clone();
    // A body far larger than any cap under test, streamed in chunks with no
    // content-length (so the client cannot pre-empt it from the header).
    tokio::spawn(async move {
        use tokio::io::AsyncWriteExt;
        while let Ok((mut stream, _)) = listener.accept().await {
            let _ = stream
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n")
                .await;
            let chunk = vec![b'x'; 64 * 1024];
            for _ in 0..512 {
                // 512 * 64 KiB = 32 MiB if nobody stops us.
                let header = format!("{:x}\r\n", chunk.len());
                if stream.write_all(header.as_bytes()).await.is_err() {
                    break;
                }
                if stream.write_all(&chunk).await.is_err() {
                    break;
                }
                if stream.write_all(b"\r\n").await.is_err() {
                    break;
                }
                counter.fetch_add(chunk.len() as u64, Ordering::Relaxed);
            }
            let _ = stream.write_all(b"0\r\n\r\n").await;
        }
    });
    let cap = 1024 * 1024;
    let err = lca_registry::plain_get_within_capped(
        &format!("http://{addr}/install.sh"),
        std::time::Duration::from_secs(30),
        cap,
    )
    .await
    .expect_err("an oversized body must be refused");
    let text = err.to_string();
    assert!(
        text.contains(&addr.to_string()),
        "the error names the host: {text}"
    );
    assert!(
        text.contains("limit") || text.contains("too large") || text.contains("over"),
        "the error names the limit: {text}"
    );
    // The server must not have been allowed to stream everything: a
    // `collect`-then-check implementation reads all 32 MiB.
    let sent = written.load(Ordering::Relaxed);
    assert!(
        sent < 32 * 1024 * 1024,
        "the read stopped at the cap instead of draining the body: the server \
         managed to write {sent} bytes"
    );
}
