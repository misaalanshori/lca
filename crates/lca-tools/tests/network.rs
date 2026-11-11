//! Network-capability tests: `net` vs `net-local` dispatch, the rebinding
//! refusal (FR-PERM-13), ad hoc grants (FR-PERM-16), the loopback OAuth
//! flow (FR-PROV-3/4), and credential namespace isolation with owner-only
//! permissions (FR-PERM-6/7, NFR-14).
//!
//! The rows are all here; the sandbox and the servers they talk to are in
//! `network/harness.rs`, declared as a plain `mod harness` (gh #570 split the
//! file when it reached 1,192 lines). A row moved is a row renamed (gh #552), so
//! only helpers moved and `cargo nextest list -p lca-tools` is unchanged.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use std::sync::{Arc, Mutex};

use lca_permissions::{GrantStore, OAuthSettings, ScopeRoots};
use lca_protocol::CapabilityError;
use lca_tools::{Capabilities, CapabilityGrants};

// An integration-test root is a crate root, so its sibling needs `#[path]`
// (gh #552/#570). The module name stays `harness`, so no helper path changed.
#[path = "network/harness.rs"]
mod harness;
use harness::*;

// Verifies: FR-PERM-4 (the host compares the target against the granted
// patterns) and FR-PERM-5 (a mismatch is denied and recorded). A
// net-local grant reaches an ordinary loopback server on a non-default
// port, which is the whole reason the capability exists (ADR-0011).
#[test]
fn net_local_reaches_a_loopback_server_and_denies_everything_else() {
    let (base, server) = canned_server("hello-capability");
    let sandbox = Sandbox::new("local");
    let caps = sandbox.caps(local_grants(&["127.0.0.1"]));

    let url = format!("{base}/probe");
    let handle = caps
        .net_request("GET", &url, &[], None)
        .expect("granted request reaches the server");
    assert_eq!(caps.net_response_status(handle).expect("status"), 200);
    assert_eq!(drain_body(&caps, handle), "hello-capability");
    caps.net_close_response(handle).expect("close");
    server.join().expect("server");

    // FR-PERM-5: a host outside every grant is refused and recorded.
    let err = caps
        .net_request("GET", "http://example.invalid/elsewhere", &[], None)
        .expect_err("denied");
    assert!(matches!(err, CapabilityError::Permission(_)), "{err:?}");
    assert!(
        caps.denial_count() >= 1,
        "the refusal is recorded (FR-PERM-5)"
    );
    // This sandbox grants net-local, so the unmatched host is a net-local
    // denial (only a `net` grant produces a `net` denial).
    assert_eq!(caps.denials()[0].capability, "net-local");
    assert!(
        caps.denials()[0].reason.contains("cannot resolve"),
        "{}",
        caps.denials()[0].reason
    );
}

// Verifies: FR-PERM-13 (a net grant whose hostname resolves into a local
// range is refused and recorded as a rebinding case, distinct from an
// ordinary denial). `localhost` under a plain `net` grant is exactly the
// catalog's scenario: the pattern matched, the resolution did not.
#[test]
fn net_refuses_local_resolution_as_rebinding() {
    let sandbox = Sandbox::new("rebind");
    let caps = sandbox.caps(net_grants(&["localhost"]));
    let err = caps
        .net_request("GET", "https://localhost/x", &[], None)
        .expect_err("refused");
    let text = err.to_string();
    assert!(text.contains("rebinding"), "distinct reason: {text}");
    assert!(caps.denial_count() == 1);
    assert!(
        caps.denials()[0].reason.contains("rebinding"),
        "recorded as a rebinding case: {}",
        caps.denials()[0].reason
    );
}

// The catalog's scheme rules: net is HTTPS-only (even on loopback),
// net-local accepts plain HTTP (ADR-0011).
#[test]
fn scheme_rules_follow_the_capability() {
    let sandbox = Sandbox::new("scheme");
    let net = sandbox.caps(net_grants(&["api.example.com"]));
    let err = net
        .net_request("GET", "http://api.example.com/x", &[], None)
        .expect_err("plain http refused under net");
    assert!(err.to_string().contains("HTTPS"), "{err}");

    let local = sandbox.caps(local_grants(&["localhost"]));
    // http to localhost is legal for net-local; the connection itself may
    // fail (nothing listens), but the refusal is not a permission one.
    if matches!(
        local.net_request("GET", "http://localhost:1/x", &[], None),
        Err(CapabilityError::Permission(_))
    ) {
        panic!("http must be permitted for net-local");
    }
}

// Verifies: the capability catalog's `net` contract — a response body
// is a streaming reader: the first bytes reach the extension while the
// server is still sending, not once the whole body has arrived.
#[test]
fn response_bodies_stream_before_the_server_finishes() {
    let sandbox = Sandbox::new("stream-body");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let port = addr.port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten().take(1) {
            let mut stream = stream;
            use std::io::{Read, Write};
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            // Valid head, no length header: the body runs to connection
            // close, which keeps the reader streaming frames.
            let _ = stream.write_all(
                b"HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\nconnection: close\r\n\r\nAAAA",
            );
            let _ = stream.flush();
            std::thread::sleep(std::time::Duration::from_millis(600));
            let _ = stream.write_all(b"BBBB");
        }
    });

    let caps = sandbox.caps(local_grants(&["127.0.0.1"]));
    let url = format!("http://127.0.0.1:{port}/slow");
    let handle = caps
        .net_request("GET", &url, &[], None)
        .expect("request head");
    let started = std::time::Instant::now();
    let first = caps
        .net_read_body(handle, 4)
        .expect("first chunk")
        .expect("data before the server finished");
    let elapsed = started.elapsed();
    assert_eq!(&first, b"AAAA");
    assert!(
        elapsed < std::time::Duration::from_millis(500),
        "the first chunk waited for the rest of the body: {elapsed:?}"
    );
    let second = caps.net_read_body(handle, 64).expect("second chunk");
    assert_eq!(second.as_deref(), Some(b"BBBB".as_slice()));
    assert!(caps.net_read_body(handle, 64).expect("eof").is_none());
    caps.net_close_response(handle).expect("close");
}

// Verifies: NFR-21 (a `net` host wait polls cancellation). A request whose
// server accepts and never answers must return promptly after `cancel`
// instead of waiting out the connect/read window - the remaining instance
// the oauth fix closed for `oauth.await`.
#[test]
fn cancelling_a_hung_net_request_returns_promptly() {
    let sandbox = Sandbox::new("net-cancel");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let port = addr.port();
    // Accept the connection and never answer: the request blocks waiting for
    // the response head until the socket closes.
    std::thread::spawn(move || {
        let _conn = listener.incoming().flatten().next();
        std::thread::sleep(std::time::Duration::from_secs(5));
    });

    let caps = std::sync::Arc::new(sandbox.caps(local_grants(&["127.0.0.1"])));
    let canceller = caps.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(100));
        canceller.cancel();
    });
    let url = format!("http://127.0.0.1:{port}/hang");
    let started = std::time::Instant::now();
    let err = caps
        .net_request("GET", &url, &[], None)
        .expect_err("the hung request is cancelled");
    let elapsed = started.elapsed();
    assert!(
        elapsed < std::time::Duration::from_secs(2),
        "cancel did not reach the net wait: {elapsed:?} ({err})"
    );
}

// Verifies: FR-PERM-16 (a grant beyond the fixed vocabulary attaches as
// an ad hoc grant whose consent names the specific host, not a pattern
// the manifest asked for).
#[test]
fn ad_hoc_grants_reach_hosts_the_manifest_could_not_know() {
    let (base, server) = canned_server("adhoc-ok");
    let host_port = base.trim_start_matches("http://127.0.0.1:");
    let sandbox = Sandbox::new("adhoc");
    let caps = sandbox.caps(CapabilityGrants {
        adhoc_net: vec![
            lca_permissions::parse_net_pattern(&format!("127.0.0.1:{host_port}"))
                .expect("literal with port"),
        ],
        ..CapabilityGrants::default()
    });
    // The ad hoc host is http on a high port: net-local rules apply to
    // loopback ad hoc entries the same way (catalog: the user attached
    // this exact host).
    let url = format!("{base}/adhoc");
    let handle = caps
        .net_request("GET", &url, &[], None)
        .expect("the specifically-granted host is reachable");
    assert_eq!(drain_body(&caps, handle), "adhoc-ok");
    server.join().expect("server");
}

// Verifies: FR-PROV-3 (begin returns the redirect URL with a loopback
// handle) and FR-PROV-4 (bound on the local interface only).
#[test]
fn oauth_begin_binds_loopback_and_the_callback_delivers_parameters() {
    let sandbox = Sandbox::new("oauth");
    let caps = sandbox.caps(CapabilityGrants {
        net: vec![lca_permissions::parse_net_pattern("auth.example.com").expect("host")],
        oauth: Some(OAuthSettings {
            redirect_path: "/callback".to_string(),
            timeout_seconds: 30,
        }),
        ..CapabilityGrants::default()
    });

    let (redirect, flow) = caps.oauth_begin("/callback").expect("begin");
    assert!(
        redirect.starts_with("http://127.0.0.1:"),
        "loopback only (FR-PROV-4): {redirect}"
    );
    assert!(
        !redirect.starts_with("http://0.0.0.0"),
        "never all interfaces"
    );

    // The "browser" arrives: an ordinary GET with the authorization code.
    // redirect: http://127.0.0.1:PORT/callback
    let rest = redirect.trim_start_matches("http://").to_string();
    let (hostport, path) = rest.split_once('/').expect("redirect has a path");
    let hostport = hostport.to_string();
    let path = path.to_string();
    let delayed = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(80));
        let mut stream = std::net::TcpStream::connect(&hostport).expect("connect");
        use std::io::Write;
        let request =
            format!("GET /{path}?code=auth-code&state=xyz HTTP/1.1\r\nHost: {hostport}\r\n\r\n");
        stream.write_all(request.as_bytes()).expect("write");
        let mut response = String::new();
        use std::io::Read;
        stream.read_to_string(&mut response).expect("read");
        response
    });
    let params = caps.oauth_await(flow).expect("callback arrives");
    delayed.join().expect("browser thread");
    assert!(
        params.contains(&("code".to_string(), "auth-code".to_string())),
        "{params:?}"
    );
    assert!(
        params.contains(&("state".to_string(), "xyz".to_string())),
        "{params:?}"
    );
    caps.oauth_end(flow).expect("end");
}

// Verifies: R4(c) - a manually pasted callback wakes the waiting flow
// exactly as the loopback listener would, so a login completes where the
// browser never reached `127.0.0.1` (or its auto-open failed).
#[test]
fn a_manually_delivered_callback_wakes_a_blocked_oauth_wait() {
    let sandbox = Sandbox::new("oauth-manual");
    let caps = sandbox.caps(CapabilityGrants {
        oauth: Some(OAuthSettings {
            redirect_path: "/callback".to_string(),
            timeout_seconds: 30,
        }),
        ..CapabilityGrants::default()
    });

    let (_redirect, flow) = caps.oauth_begin("/callback").expect("begin");
    let engine = std::sync::Arc::new(caps);
    let waiter = engine.clone();
    let join = std::thread::spawn(move || waiter.oauth_await(flow));
    std::thread::sleep(std::time::Duration::from_millis(150));
    engine
        .oauth_deliver_manual(vec![
            ("code".to_string(), "manual-code".to_string()),
            ("state".to_string(), "s-1".to_string()),
        ])
        .expect("the manual callback delivers");
    let params = join.join().expect("the waiter joins").expect("arrives");
    assert!(
        params.contains(&("code".to_string(), "manual-code".to_string())),
        "the delivered params reach the wait: {params:?}"
    );
    engine.oauth_end(flow).expect("end");

    // A delivery with nothing waiting is a legible error, not a panic.
    let result = engine.oauth_deliver_manual(vec![("code".into(), "x".into())]);
    assert!(result.is_err(), "no live waiter: {result:?}");
}

// Verifies: FR-PROV-3 against the connect-then-wait race - a client
// that connects and then takes its time writing the request (a
// descheduled thread on a loaded runner, a browser's preconnect) still
// gets its callback served. One-shot reads closed the connection under
// such a client and its write died with EPIPE: the exact failure that
// flaked the OAuth login on macOS CI.
#[test]
fn oauth_callback_survives_a_client_that_connects_before_it_writes() {
    let sandbox = Sandbox::new("oauth-slow-client");
    let caps = sandbox.caps(CapabilityGrants {
        oauth: Some(OAuthSettings {
            redirect_path: "/callback".to_string(),
            timeout_seconds: 30,
        }),
        ..CapabilityGrants::default()
    });

    let (redirect, flow) = caps.oauth_begin("/callback").expect("begin");
    let rest = redirect.trim_start_matches("http://").to_string();
    let (hostport, path) = rest.split_once('/').expect("redirect has a path");
    let hostport = hostport.to_string();
    let path = path.to_string();
    let client = std::thread::spawn(move || {
        let stream = std::net::TcpStream::connect(&hostport).expect("connect");
        // Long past the listener's original one-shot patience: the
        // thread "loses the scheduler" between connect and write.
        std::thread::sleep(std::time::Duration::from_millis(5500));
        let mut stream = stream;
        use std::io::{Read, Write};
        let request =
            format!("GET /{path}?code=slow-code&state=slow HTTP/1.1\r\nHost: {hostport}\r\n\r\n");
        stream
            .write_all(request.as_bytes())
            .expect("write survives the wait");
        let mut response = String::new();
        stream.read_to_string(&mut response).expect("read");
        response
    });
    let params = caps.oauth_await(flow).expect("callback arrives");
    let response = client.join().expect("client thread");
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(
        params.contains(&("code".to_string(), "slow-code".to_string())),
        "{params:?}"
    );
    caps.oauth_end(flow).expect("end");
}

// Verifies: FR-PROV-3's denied path: no oauth grant, no listener.
#[test]
fn oauth_without_a_grant_never_binds() {
    let sandbox = Sandbox::new("oauth-denied");
    let caps = sandbox.caps(CapabilityGrants::default());
    let err = caps.oauth_begin("/callback").expect_err("denied");
    assert!(
        matches!(
            err,
            CapabilityError::NotGranted(_) | CapabilityError::Permission(_)
        ),
        "{err:?}"
    );
    assert_eq!(caps.denial_count(), 1, "recorded");
}

// Verifies: FR-PERM-6 (each extension reads only its own namespace; the
// namespace is its identity, never guest input), FR-PERM-7 (a foreign
// namespace has no address to read: the attempt is denied), and
// NFR-14 (credentials are stored with owner-only permissions).
#[test]
fn credentials_are_namespace_isolated_with_owner_only_permissions() {
    let sandbox = Sandbox::new("creds");
    let alice = sandbox.caps(CapabilityGrants {
        credentials: true,
        ..CapabilityGrants::default()
    });
    alice.credentials_set("token", "s3cret").expect("set");
    assert_eq!(
        alice.credentials_get("token").expect("get"),
        Some("s3cret".to_string())
    );

    // A different extension's identity sees nothing: the namespace comes
    // from the extension's own name (FR-PERM-7: no cross-namespace read).
    let bob = Capabilities::new(
        "someone-else",
        CapabilityGrants {
            credentials: true,
            ..CapabilityGrants::default()
        },
        ScopeRoots {
            workspace: sandbox.root.join("workspace"),
            private: sandbox.root.join("private"),
            home_config: sandbox.root.join("config"),
            temp: sandbox.root.join("tmp"),
            state_dir: sandbox.root.join("data"),
        },
        Arc::new(Mutex::new(Allow)),
        Arc::new(Mutex::new(
            GrantStore::open(&sandbox.root.join("grants.json")).expect("store"),
        )),
        sandbox.root.join("workspace"),
        None,
    );
    assert_eq!(
        bob.credentials_get("token").expect("get"),
        None,
        "namespace isolation"
    );

    alice.credentials_delete("token").expect("delete");
    assert_eq!(alice.credentials_get("token").expect("get"), None);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        alice.credentials_set("again", "v").expect("set");
        let path = sandbox.root.join("data/credentials/probe.json");
        let mode = std::fs::metadata(&path).expect("file").permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "owner-only (NFR-14)");
    }
}

// Verifies: FR-PERM-3 (calling an import for an undeclared capability
// returns a permission error and records the attempt) for the network
// family: no net grant at all.
#[test]
fn undeclared_net_is_a_recorded_permission_error() {
    let sandbox = Sandbox::new("undeclared");
    let caps = sandbox.caps(CapabilityGrants::default());
    let err = caps
        .net_request("GET", "https://example.com/", &[], None)
        .expect_err("denied");
    assert!(matches!(err, CapabilityError::NotGranted(_)), "{err:?}");
    assert_eq!(caps.denial_count(), 1);
}

// Verifies: FR-PERM-18 (an ad hoc grant attached during a session is
// honored for subsequent calls without a restart) - the SAME engine
// that just refused the host accepts it the moment the user approves
// it in the grant store, because the store is read live (ADR-0022's
// persistence half feeding FR-PERM-18's session half).
#[test]
fn an_adhoc_grant_attached_mid_session_takes_effect_without_a_restart() {
    let sandbox = Sandbox::new("adhoc-live");
    let (url, _server) = canned_server("hello");
    let caps = sandbox.caps(CapabilityGrants {
        net: vec![lca_permissions::parse_net_pattern("api.example.com").expect("pattern")],
        ..Default::default()
    });

    // Before: loopback is not in any declared pattern, and there is no
    // net-local grant, so the request is refused and recorded.
    let err = caps
        .net_request("GET", &url, &[], None)
        .expect_err("refused before consent");
    assert!(
        err.to_string().contains("matches no granted")
            || matches!(err, CapabilityError::NotGranted(_)),
        "{err}"
    );

    // The user attaches it (what FR-PERM-16's modal will do): keyed by
    // the engine's own project path, its workspace root here.
    caps.grant_store()
        .lock()
        .expect("store")
        .approve_net_pattern(&sandbox.root.join("workspace"), "127.0.0.1")
        .expect("approve");

    // ...and the SAME engine honors it immediately: no restart, no
    // new process, next call only.
    let handle = caps
        .net_request("GET", &url, &[], None)
        .expect("honored after consent");
    caps.net_close_response(handle).expect("close");
}

// Verifies: the denial journal `ext info` counts is one valid JSON object per
// line even when the denied parameter carries quotes and a newline (the old
// hand-built line could be split or corrupted by guest input).
#[test]
fn denial_journal_escapes_hostile_parameters() {
    let sandbox = Sandbox::new("journal");
    let grants = CapabilityGrants {
        fs: vec![
            lca_permissions::ScopeGrant::parse("workspace", lca_permissions::FsMode::Read)
                .expect("grant"),
        ],
        fs_declared: true,
        ..Default::default()
    };
    let caps = sandbox.caps(grants);

    let _ = caps.fs_read("workspace", "../escape\"\nsecond");
    let journal = sandbox.root.join("data/extensions/probe/denials.jsonl");
    let text = std::fs::read_to_string(&journal).expect("journal written");
    assert_eq!(text.lines().count(), 1, "one denial, one line: {text:?}");
    let value: serde_json::Value = serde_json::from_str(text.trim_end()).expect("valid JSON");
    assert_eq!(value["capability"], "fs");
    assert!(
        value["parameter"]
            .as_str()
            .unwrap_or_default()
            .contains("escape"),
        "{value}"
    );
}

// Verifies: V2 / ADR-0031 (the host sets one generic User-Agent when the
// caller sets none, and a caller's own wins).
#[test]
fn the_net_gate_sets_a_default_user_agent_and_preserves_a_caller_one() {
    let sandbox = Sandbox::new("ua");

    let (base, server) = ua_echo_server();
    let caps = sandbox.caps(local_grants(&["127.0.0.1"]));
    let handle = caps
        .net_request("GET", &format!("{base}/x"), &[], None)
        .expect("request");
    let body = drain_body(&caps, handle);
    caps.net_close_response(handle).expect("close");
    let _ = server.join().expect("server");
    assert!(body.starts_with("lca/"), "host default UA: {body}");
    assert!(body.contains("abi-0.6"), "names the ABI line: {body}");

    let (base, server) = ua_echo_server();
    let caps = sandbox.caps(local_grants(&["127.0.0.1"]));
    let handle = caps
        .net_request(
            "GET",
            &format!("{base}/x"),
            &[("user-agent", "mine/1")],
            None,
        )
        .expect("request");
    let body = drain_body(&caps, handle);
    caps.net_close_response(handle).expect("close");
    let _ = server.join().expect("server");
    assert_eq!(body, "mine/1", "a caller-set UA is preserved");
}

// Verifies: gh #145 (pi's `HTTP_PROXY` row) — with `HTTP_PROXY` set, a
// plain-http request travels through the proxy in absolute form, and
// the target still answers.
#[test]
fn proxy_env_routes_plain_http_through_the_proxy() {
    let (target, server) = canned_server("via-proxy");
    let (proxy_addr, seen, proxy) = forward_proxy(1);
    let _env = ProxyEnv::set(&[("HTTP_PROXY", &format!("http://{proxy_addr}"))]);
    let sandbox = Sandbox::new("proxy-used");
    let caps = sandbox.caps(local_grants(&["127.0.0.1"]));

    let handle = caps
        .net_request("GET", &format!("{target}/proxied"), &[], None)
        .expect("proxied request reaches the target");
    assert_eq!(drain_body(&caps, handle), "via-proxy");
    caps.net_close_response(handle).expect("close");
    server.join().expect("target server");
    proxy.join().expect("proxy");

    let seen = seen.lock().expect("log").clone();
    assert_eq!(seen.len(), 1, "exactly one proxied request: {seen:?}");
    assert!(
        seen[0].starts_with("GET http://"),
        "absolute form at the proxy: {:?}",
        seen[0]
    );
}

// Verifies: gh #145 (pi's `NO_PROXY` row) — a `NO_PROXY` host bypasses
// the proxy entirely: the target answers and the proxy sees nothing.
#[test]
fn no_proxy_bypasses_the_proxy() {
    let (target, server) = canned_server("direct");
    let (proxy_addr, seen, proxy) = forward_proxy(0);
    let _env = ProxyEnv::set(&[
        ("HTTP_PROXY", &format!("http://{proxy_addr}")),
        ("NO_PROXY", "127.0.0.1"),
    ]);
    let sandbox = Sandbox::new("proxy-bypassed");
    let caps = sandbox.caps(local_grants(&["127.0.0.1"]));

    let handle = caps
        .net_request("GET", &format!("{target}/direct"), &[], None)
        .expect("bypassed request reaches the target");
    assert_eq!(drain_body(&caps, handle), "direct");
    caps.net_close_response(handle).expect("close");
    server.join().expect("target server");
    proxy.join().expect("proxy");

    assert!(
        seen.lock().expect("log").is_empty(),
        "the proxy saw nothing"
    );
}

// Verifies: gh #145 — the lowercase `http_proxy` form is honored too
// (curl convention); only the variable name differs from the row above.
#[test]
fn lowercase_proxy_env_is_honored() {
    let (target, server) = canned_server("via-lowercase-proxy");
    let (proxy_addr, seen, proxy) = forward_proxy(1);
    let _env = ProxyEnv::set(&[("http_proxy", &format!("http://{proxy_addr}"))]);
    let sandbox = Sandbox::new("proxy-lowercase");
    let caps = sandbox.caps(local_grants(&["127.0.0.1"]));

    let handle = caps
        .net_request("GET", &format!("{target}/proxied"), &[], None)
        .expect("proxied request reaches the target");
    assert_eq!(drain_body(&caps, handle), "via-lowercase-proxy");
    caps.net_close_response(handle).expect("close");
    server.join().expect("target server");
    proxy.join().expect("proxy");
    assert_eq!(seen.lock().expect("log").len(), 1);
}

// Verifies: gh #94 (the `net` engine's HTTPS path works offline). A full
// TLS handshake, an HTTP response over the session, and the body drains
// through `net_read_body` - the path the env-gated smoke used to own.
#[test]
fn https_through_net_completes_a_loopback_handshake_offline() {
    let _env = ProxyEnv::set(&[]);
    let (base, server) = canned_tls_server("tls-ok");
    let sandbox = Sandbox::new("tls-positive");
    let caps = adhoc_tls_caps(&sandbox, &base);
    let url = format!("{base}/tls");
    let handle = caps
        .net_request("GET", &url, &[], None)
        .expect("https through net reaches the loopback server");
    assert_eq!(drain_body(&caps, handle), "tls-ok");
    server.join().expect("server");
}

// Verifies: gh #94 (an untrusted certificate is a clean error, never a
// panic and never silent acceptance). Without the anchor the fixture
// server's chain does not verify; the failure must surface as an
// `Io`-class capability error naming the certificate problem.
#[test]
fn an_untrusted_loopback_certificate_fails_cleanly_not_silently() {
    let _env = ProxyEnv::set(&[]);
    let (base, server) = canned_tls_server("unreachable");
    let host_port = base.trim_start_matches("https://127.0.0.1:");
    let sandbox = Sandbox::new("tls-negative");
    let caps = sandbox.caps(CapabilityGrants {
        adhoc_net: vec![
            lca_permissions::parse_net_pattern(&format!("127.0.0.1:{host_port}"))
                .expect("literal with port"),
        ],
        ..CapabilityGrants::default()
    });
    let url = format!("{base}/tls");
    let err = caps
        .net_request("GET", &url, &[], None)
        .expect_err("no anchor trusts the fixture server");
    assert!(
        !matches!(err, CapabilityError::Permission(_)),
        "a TLS failure is not a permission refusal: {err}"
    );
    let text = err.to_string().to_lowercase();
    assert!(
        text.contains("certificate") || text.contains("tls") || text.contains("ssl"),
        "the error names the certificate problem: {err}"
    );
    drop(server);
}

// Verifies: gh #94 (the `enforce_http` class stays pinned). Speaking
// HTTPS at a plain-HTTP server fails in the TLS layer - the scheme
// still routes to TLS instead of downgrading or misrouting.
#[test]
fn https_to_a_plain_http_server_fails_in_tls_not_in_routing() {
    let _env = ProxyEnv::set(&[]);
    let (base, server) = canned_server("plain");
    let host_port = base.trim_start_matches("http://127.0.0.1:");
    let sandbox = Sandbox::new("tls-mismatch");
    let caps = sandbox.caps(CapabilityGrants {
        adhoc_net: vec![
            lca_permissions::parse_net_pattern(&format!("127.0.0.1:{host_port}"))
                .expect("literal with port"),
        ],
        ..CapabilityGrants::default()
    });
    let url = format!("https://127.0.0.1:{host_port}/tls");
    let err = caps
        .net_request("GET", &url, &[], None)
        .expect_err("plain http is not TLS");
    assert!(
        !matches!(err, CapabilityError::Permission(_)),
        "past the scheme check, into TLS: {err}"
    );
    server.join().expect("server");
}

// ---------------------------------------------------------------------------
// gh #476: a capability call made from inside a runtime must not panic
// ---------------------------------------------------------------------------

//
// The measurement is the process surviving. `catch_unwind` observes a panic
// without taking the test binary down with it.
#[test]
fn a_capability_request_from_inside_a_multi_thread_runtime_does_not_panic() {
    let sandbox = Sandbox::new("gh476-in-runtime");
    let caps = flow_caps(&sandbox);
    let url = refusing_server();
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("test runtime")
            .block_on(async {
                // Inside the poll: the exact shape that panicked.
                caps.oauth_flow_net_request("GET", &url, &[], None)
            })
    }));
    let result = match outcome {
        Ok(result) => result,
        Err(_) => panic!(
            "a capability call from inside a runtime must return, not unwind \
             (gh #476: this killed the whole TUI session)"
        ),
    };
    // And it must actually have done the work, not silently returned an error: a
    // fix that turns "crash" into "no-op" is a worse bug.
    let handle = result.expect("the request reached the refusing server");
    let status = caps.net_response_status(handle).expect("status");
    let _ = caps.net_close_response(handle);
    assert_eq!(status, 401);
}

// Verifies: gh #476 - **the current-thread case is the one that cannot work**, and
// the reason is structural rather than a gap left open.
//
// A current-thread runtime can neither `block_in_place` nor `block_on` from inside
// its own poll - there is no other thread to move the work to, and the poll is
// already using the only one. `tokio::task::block_in_place` panics there, and so
// does `block_on`; no arrangement of this function can conjure a second thread
// without requiring `Send` on the future, which one caller (the response-body
// reader, whose future borrows a body across loop iterations) cannot satisfy.
//
// The case that *does* occur in the shipped binary is a current-thread runtime
// whose frame calls **into** the capability layer from a plain `block_on` -
// `lca_core::drive_blocking` builds exactly that - and it takes the
// `handle.block_on` path, which is legal there because it is not a poll. That is
// the row below. This row pins the *other* shape so a future reader knows the
// limit is deliberate:
#[test]
fn a_current_thread_poll_cannot_block_and_that_is_a_named_limit() {
    let sandbox = Sandbox::new("gh476-current-thread-limit");
    let caps = flow_caps(&sandbox);
    let url = refusing_server();
    // Documenting the boundary as a test: from inside a current-thread poll the
    // call panics with tokio's own explanation. If a future refactor finds a way
    // to make this succeed, this row fails and the limit is worth re-documenting.
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime")
            .block_on(async { caps.oauth_flow_net_request("GET", &url, &[], None) })
    }));
    assert!(
        outcome.is_err(),
        "a current-thread poll cannot block on itself; if this now succeeds, the \
         documented limit in `Capabilities::drive` is stale"
    );
}

// Verifies: gh #476 - **the limit on a current-thread runtime, named.**
//
// A `current_thread` runtime can drive exactly one thing at a time, so a nested
// `block_on` on it panics whether it is called from a poll or from that runtime's
// own frame. Measured against the code before this fix as well as after: this
// shape never worked, and no shipped path takes it (`main` is multi-threaded, and
// `tui/runner.rs`'s current-thread turn runtime reaches provider code through
// `lca_core::drive_blocking`, never back through this seam).
//
// The row exists so the limit is *stated* rather than left as a tokio trap a
// reader has to decode. The two shapes that do ship - a multi-threaded poll and a
// plain thread - are the rows above and below this one.
#[test]
fn a_nested_block_on_a_current_thread_runtime_is_a_named_limit() {
    let sandbox = Sandbox::new("gh476-ct-limit");
    let caps = flow_caps(&sandbox);
    let url = refusing_server();
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("current-thread runtime")
            .block_on(async { caps.oauth_flow_net_request("GET", &url, &[], None) })
    }));
    assert!(
        outcome.is_err(),
        "a current-thread runtime cannot nest a blocking capability call; if this \
         now succeeds, the limit documented in `Capabilities::drive` is stale"
    );
}

// Verifies: gh #476 design point 1 - **the non-runtime path still works.**
// The blocking-thread caller is the common one (extensions, headless); a fix
// that only made the runtime case safe would be a regression here.
#[test]
fn a_capability_request_from_a_plain_thread_still_works() {
    let sandbox = Sandbox::new("gh476-plain-thread");
    let caps = flow_caps(&sandbox);
    let url = refusing_server();
    let handle = caps
        .oauth_flow_net_request("GET", &url, &[], None)
        .expect("a plain-thread request reaches the server");
    let status = caps.net_response_status(handle).expect("status");
    let _ = caps.net_close_response(handle);
    assert_eq!(status, 401, "the refusing server answered");
}
