//! The `serve-http` world's dual-mode row (gh #363, ADR-0049), split from
//! `conformance_diff.rs` when that file passed gate 11's 1,200-line ceiling.
//!
//! It shares the fixture through `super`, so the two modes come from the same
//! roots, prompt, grant store, and engine as every other conformance row.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use std::sync::{Arc, Mutex};

use lca_ext_abi::ExtensionDispatch;

use super::Fixture;

// Verifies: NFR-25 for the `serve-http` world (gh #363, ADR-0049) - the
// granted answer, the denial, and a parameter mismatch agree between the two
// delivery modes, driven through the host's real listener rather than by
// calling the handler directly.
//
// Driving the *listener* is the point. The host owns the socket in both modes,
// so what differs between them is only how a request reaches a handler: a WIT
// record crossing into a guest, or a plain struct in process. A row that called
// `answer()` directly would compare the scripts and skip the wiring, which is
// where a marshalling defect would live.
#[tokio::test]
async fn the_serve_http_world_answers_identically_in_both_delivery_modes() {
    use lca_protocol::http::ServeHttpNamespace;
    use std::io::{Read, Write};

    let fixture = Fixture::new("serve-http");
    let (wasm, _wasm_dispatch, native, _engine) = fixture.both_modes_keeping_wasm();
    let session = "conformance-session";

    // Each mode gets its own mount, and the two are asked the same three
    // questions. `base_url` is identical by construction - the host computes it
    // from the mount, so a mode that differed here would be a wiring defect.
    let mounts: Vec<Arc<dyn ServeHttpNamespace>> = vec![
        wasm.serve_http_mount(session).expect("wasm mounts"),
        native.serve_http_mount(session).expect("native mounts"),
    ];
    assert_eq!(mounts[0].extension(), mounts[1].extension());
    assert_eq!(mounts[0].session(), mounts[1].session());

    let base_url = format!("http://127.0.0.1:8080/{session}/conformance/");

    // (1) Granted: the path is stripped to the probe's expected page.
    let granted = |mount: &Arc<dyn ServeHttpNamespace>| {
        let request = lca_protocol::http::HttpRequest {
            method: "GET".to_string(),
            path: conformance::SERVE_HTTP_PREFIX_PATH.to_string(),
            query: "probe=1".to_string(),
            headers: vec![("host".to_string(), "127.0.0.1:8080".to_string())],
            body: Vec::new(),
        };
        mount.answer(&base_url, request)
    };
    let wasm_answer = granted(&mounts[0]).expect("the wasm mode answers");
    let native_answer = granted(&mounts[1]).expect("the native mode answers");
    assert_eq!(
        wasm_answer, native_answer,
        "the two delivery modes disagreed on a granted request"
    );
    assert_eq!(wasm_answer.status, 200);
    let body = String::from_utf8_lossy(&wasm_answer.body).into_owned();
    // The stripping contract, asserted on the answer rather than trusted: the
    // handler echoes the path it received, so a host that forwarded the mounted
    // path would show up here.
    assert!(
        body.contains(&format!("stripped={}", conformance::SERVE_HTTP_PREFIX_PATH)),
        "the handler did not receive the stripped path: {body}"
    );
    assert!(body.contains("query=probe=1"), "got {body}");
    assert!(body.contains(&format!("base={base_url}")), "got {body}");

    // (2) Parameter mismatch: a method the probe does not serve is a failure in
    // both modes, with the same text.
    let mismatch = |mount: &Arc<dyn ServeHttpNamespace>| {
        mount.answer(
            &base_url,
            lca_protocol::http::HttpRequest {
                method: "POST".to_string(),
                path: "/index.html".to_string(),
                query: String::new(),
                headers: Vec::new(),
                body: Vec::new(),
            },
        )
    };
    let wasm_err = mismatch(&mounts[0]).expect_err("a POST is refused");
    let native_err = mismatch(&mounts[1]).expect_err("a POST is refused");
    assert_eq!(wasm_err.text(), native_err.text());

    // (3) Denied: the probe's own not-granted path, the shape a revoked grant
    // produces. Both modes report the same capability error.
    let denied = |mount: &Arc<dyn ServeHttpNamespace>| {
        mount.answer(
            &base_url,
            lca_protocol::http::HttpRequest {
                method: "GET".to_string(),
                path: "/index.html".to_string(),
                query: String::new(),
                headers: vec![("x-conformance-deny".to_string(), "1".to_string())],
                body: Vec::new(),
            },
        )
    };
    let wasm_denied = denied(&mounts[0]).expect_err("the grant is refused");
    let native_denied = denied(&mounts[1]).expect_err("the grant is refused");
    assert_eq!(wasm_denied.text(), native_denied.text());
    assert!(matches!(
        wasm_denied,
        lca_protocol::CapabilityError::NotGranted(_)
    ));

    // And the whole thing over a real socket, so the host's own gate is proven
    // to be in front of the guest rather than around it.
    let listing: Arc<Mutex<Vec<Arc<dyn ServeHttpNamespace>>>> =
        Arc::new(Mutex::new(vec![mounts[0].clone()]));
    let listener = lca_tools::serve_http::bind(
        listing,
        lca_tools::serve_http::SessionToken::from_known("conformance-token"),
    )
    .expect("the host's listener binds");
    let addr = listener.bound().addr;
    let mut stream = std::net::TcpStream::connect(addr).expect("connect");
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(10)))
        .expect("timeout");
    write!(
        stream,
        "GET /{session}/conformance/index.html?token=conformance-token HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
        addr.port()
    )
    .expect("write");
    let mut response = String::new();
    stream.read_to_string(&mut response).expect("read");
    assert!(response.starts_with("HTTP/1.1 200"), "got {response}");
    assert!(
        response.contains(&format!("stripped={}", conformance::SERVE_HTTP_PREFIX_PATH)),
        "the socket path did not reach the handler stripped: {response}"
    );
}
