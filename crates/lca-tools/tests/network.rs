//! Network-capability tests: `net` vs `net-local` dispatch, the rebinding
//! refusal (FR-PERM-13), ad hoc grants (FR-PERM-16), the loopback OAuth
//! flow (FR-PROV-3/4), and credential namespace isolation with owner-only
//! permissions (FR-PERM-6/7, NFR-14).

use std::net::TcpListener as StdTcpListener;
use std::sync::{Arc, Mutex};

use lca_permissions::{GrantStore, OAuthSettings, PermissionPrompt, ProposalDiff, ScopeRoots};
use lca_protocol::CapabilityError;
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

struct Sandbox {
    root: std::path::PathBuf,
}

impl Sandbox {
    fn new(name: &str) -> Sandbox {
        let root = std::env::temp_dir().join(format!("lca-net-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for dir in [
            "workspace",
            "private",
            "config",
            "data",
            "tmp",
            "data/credentials",
        ] {
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

fn net_grants(hosts: &[&str]) -> CapabilityGrants {
    CapabilityGrants {
        net: hosts
            .iter()
            .map(|h| lca_permissions::parse_net_pattern(h).expect("pattern"))
            .collect(),
        ..CapabilityGrants::default()
    }
}

fn local_grants(addresses: &[&str]) -> CapabilityGrants {
    CapabilityGrants {
        net_local: addresses
            .iter()
            .map(|a| lca_permissions::parse_local_pattern(a).expect("pattern"))
            .collect(),
        ..CapabilityGrants::default()
    }
}

/// A loopback HTTP server for one canned response, on its own runtime
/// thread (the capability's calls are synchronous by design).
fn canned_server(body: &'static str) -> (String, std::thread::JoinHandle<()>) {
    let listener = StdTcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let handle = std::thread::spawn(move || {
        if let Some(mut stream) = listener.incoming().flatten().next() {
            use std::io::{Read, Write};
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });
    (format!("http://{addr}"), handle)
}

fn drain_body(caps: &Capabilities, handle: u32) -> String {
    let mut out = Vec::new();
    while let Some(chunk) = caps.net_read_body(handle, 4096).expect("body chunk") {
        out.extend_from_slice(&chunk);
    }
    String::from_utf8_lossy(&out).into_owned()
}

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
// namespace is its identity, never guest input) and NFR-14 (credentials
// are stored with owner-only permissions).
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
