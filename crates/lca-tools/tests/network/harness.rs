//! The network tests' harness: the sandbox, the canned HTTP/TLS/echo servers,
//! the proxy helper, and the grant constructors the rows below build from.
//!
//! Split out of `network.rs` for gate 11's 1,200-line ceiling (gh #570). **No
//! row moved**: every `#[test]` stays in `network.rs`, so the test set is
//! byte-identical - an `#[path]` sibling of a test module would have added a
//! module segment and renamed the rows (gh #552's lesson).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use std::net::TcpListener as StdTcpListener;
use std::sync::{Arc, Mutex};

use lca_permissions::{GrantStore, OAuthSettings, PermissionPrompt, ProposalDiff, ScopeRoots};
use lca_tools::{Capabilities, CapabilityGrants};

pub(crate) struct Allow;
impl PermissionPrompt for Allow {
    fn ask(&mut self, _: &lca_permissions::Action) -> lca_permissions::Decision {
        lca_permissions::Decision::Always
    }
    fn review_proposals(&mut self, _: &ProposalDiff) -> bool {
        false
    }
}

pub(crate) struct Sandbox {
    pub(crate) root: std::path::PathBuf,
}

impl Sandbox {
    pub(crate) fn new(name: &str) -> Sandbox {
        let root = lca_testkit::scratch_path(&format!("lca-net-{name}"));
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

    pub(crate) fn caps(&self, grants: CapabilityGrants) -> Capabilities {
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

pub(crate) fn net_grants(hosts: &[&str]) -> CapabilityGrants {
    CapabilityGrants {
        net: hosts
            .iter()
            .map(|h| lca_permissions::parse_net_pattern(h).expect("pattern"))
            .collect(),
        ..CapabilityGrants::default()
    }
}

pub(crate) fn local_grants(addresses: &[&str]) -> CapabilityGrants {
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
pub(crate) fn canned_server(body: &'static str) -> (String, std::thread::JoinHandle<()>) {
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

pub(crate) fn drain_body(caps: &Capabilities, handle: u32) -> String {
    let mut out = Vec::new();
    while let Some(chunk) = caps.net_read_body(handle, 4096).expect("body chunk") {
        out.extend_from_slice(&chunk);
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A loopback server that echoes the request's `User-Agent` value as its
/// body (for the V2 default-UA test).
pub(crate) fn ua_echo_server() -> (String, std::thread::JoinHandle<String>) {
    let listener = StdTcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let handle = std::thread::spawn(move || {
        use std::io::{Read, Write};
        let (mut stream, _) = listener.accept().expect("accept");
        let mut buf = [0u8; 4096];
        let n = stream.read(&mut buf).expect("read");
        let request = String::from_utf8_lossy(&buf[..n]).to_string();
        let ua = request
            .lines()
            .find(|line| line.to_ascii_lowercase().starts_with("user-agent:"))
            .and_then(|line| line.split_once(':'))
            .map(|(_, value)| value.trim().to_string())
            .unwrap_or_default();
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            ua.len(),
            ua
        );
        let _ = stream.write_all(response.as_bytes());
        ua
    });
    (format!("http://{addr}"), handle)
}

// A minimal forwarding HTTP proxy for the gh #145 rows: accepts up to
// `max` connections, relays each to its absolute-URI target, and logs
// every request line it sees. Plain blocking threads, like the canned
// servers above; `max = 0` accepts nothing (the bypass row).
pub(crate) fn forward_proxy(
    max: usize,
) -> (String, Arc<Mutex<Vec<String>>>, std::thread::JoinHandle<()>) {
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let seen_for_thread = seen.clone();
    let listener = StdTcpListener::bind("127.0.0.1:0").expect("proxy bind");
    let addr = listener.local_addr().expect("proxy addr");
    // Nonblocking accept with a deadline: without the gh #145
    // implementation nothing ever connects, and the test must fail on
    // its assertions, not hang the suite on `join`.
    listener.set_nonblocking(true).expect("nonblocking");
    let handle = std::thread::spawn(move || {
        use std::io::{Read, Write};
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        let mut served = 0usize;
        while served < max && std::time::Instant::now() < deadline {
            let stream = match listener.accept() {
                Ok((stream, _)) => stream,
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                    continue;
                }
                Err(_) => break,
            };
            served += 1;
            let mut client = stream;
            // The listener is nonblocking; the session is not (Linux
            // and macOS disagree about inheritance, so say it).
            let _ = client.set_nonblocking(false);
            let mut head = Vec::new();
            let mut byte = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") && head.len() < 65536 {
                match client.read(&mut byte) {
                    Ok(1) => head.push(byte[0]),
                    _ => break,
                }
            }
            let text = String::from_utf8_lossy(&head).into_owned();
            let request_line = text.lines().next().unwrap_or_default().to_string();
            seen_for_thread
                .lock()
                .expect("log")
                .push(request_line.clone());
            let target = request_line
                .split_whitespace()
                .nth(1)
                .unwrap_or_default()
                .trim_start_matches("http://")
                .to_string();
            let (authority, _) = target.split_once('/').unwrap_or((&target, ""));
            if let Ok(mut upstream) = std::net::TcpStream::connect(authority) {
                let _ = upstream.write_all(&head);
                let mut upstream_read = upstream.try_clone().expect("clone");
                let mut client_write = client.try_clone().expect("clone");
                let relay = std::thread::spawn(move || {
                    let _ = std::io::copy(&mut upstream_read, &mut client_write);
                });
                let _ = std::io::copy(&mut client, &mut upstream);
                let _ = relay.join();
            }
        }
    });
    (addr.to_string(), seen, handle)
}

/// Holds the process proxy environment for one test (gh #145): the
/// `env_lock` discipline serializes environment users, and drop
/// restores whatever was there before.
pub(crate) struct ProxyEnv {
    pub(crate) saved: Vec<(String, Option<std::ffi::OsString>)>,
    pub(crate) _guard: std::sync::MutexGuard<'static, ()>,
}

impl ProxyEnv {
    pub(crate) fn set(vars: &[(&str, &str)]) -> ProxyEnv {
        let guard = lca_testkit::fixture::env_lock();
        // SAFETY: the lock is held for the whole fixture life; see
        // `TestEnv` for the discipline.
        let mut saved = Vec::new();
        for (key, value) in vars {
            saved.push((key.to_string(), std::env::var_os(key)));
            unsafe { std::env::set_var(key, value) };
        }
        // A proxy test must not inherit a developer's real proxy (or a
        // previous test's): clear every proxy variable not set above.
        // The comparison is case-insensitive because the Windows
        // environment is: clearing `http_proxy` there deletes the
        // just-set `HTTP_PROXY` (one slot), and the suite bypasses.
        for key in [
            "HTTP_PROXY",
            "http_proxy",
            "HTTPS_PROXY",
            "https_proxy",
            "ALL_PROXY",
            "all_proxy",
            "NO_PROXY",
            "no_proxy",
        ] {
            if vars.iter().all(|(set, _)| !set.eq_ignore_ascii_case(key)) {
                saved.push((key.to_string(), std::env::var_os(key)));
                unsafe { std::env::remove_var(key) };
            }
        }
        ProxyEnv {
            saved,
            _guard: guard,
        }
    }
}

impl Drop for ProxyEnv {
    fn drop(&mut self) {
        // SAFETY: same lock discipline as in `set`.
        for (key, value) in self.saved.drain(..) {
            unsafe {
                match value {
                    Some(value) => std::env::set_var(&key, value),
                    None => std::env::remove_var(&key),
                }
            }
        }
    }
}

// The offline TLS rows (gh #94): the `net` capability's HTTPS path
// against a loopback TLS server, no network beyond the machine. The
// server speaks one canned HTTP/1.1 response over a tokio-rustls
// session with the fixture certificate; the client side is the
// capability engine itself.
pub(crate) const TLS_CA_PEM: &str = include_str!("../fixtures/tls/ca.pem");
pub(crate) const TLS_SERVER_PEM: &str = include_str!("../fixtures/tls/server.pem");
pub(crate) const TLS_SERVER_KEY_PEM: &str = include_str!("../fixtures/tls/server-key.pem");

pub(crate) fn canned_tls_server(body: &'static str) -> (String, std::thread::JoinHandle<()>) {
    let listener = StdTcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    // from_std adopts the socket into the runtime: it must already be
    // nonblocking, or every test fails at registration.
    listener.set_nonblocking(true).expect("nonblocking");
    let handle = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tls fixture runtime");
        runtime.block_on(async move {
            use rustls::pki_types::pem::PemObject;
            let certs: Vec<rustls::pki_types::CertificateDer<'static>> =
                rustls::pki_types::CertificateDer::pem_slice_iter(
                    TLS_SERVER_PEM.as_bytes(),
                )
                .collect::<Result<_, _>>()
                .expect("fixture server cert");
            let key = rustls::pki_types::PrivateKeyDer::from_pem_slice(
                TLS_SERVER_KEY_PEM.as_bytes(),
            )
            .expect("fixture server key");
            let config = rustls::ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(certs, key)
                .expect("fixture server config");
            let acceptor = tokio_rustls::TlsAcceptor::from(std::sync::Arc::new(config));
            let listener = tokio::net::TcpListener::from_std(listener).expect("adopt");
            let (tcp, _) = listener.accept().await.expect("accept");
            let mut tls = acceptor.accept(tcp).await.expect("handshake");
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let mut seen = Vec::new();
            let mut buf = [0u8; 1024];
            loop {
                let n = tls.read(&mut buf).await.expect("request bytes");
                if n == 0 {
                    break;
                }
                seen.extend_from_slice(&buf[..n]);
                if seen.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            tls.write_all(response.as_bytes()).await.expect("response");
            tls.shutdown().await.expect("close");
        });
    });
    (format!("https://{addr}"), handle)
}

pub(crate) fn adhoc_tls_caps(sandbox: &Sandbox, base: &str) -> Capabilities {
    let host_port = base.trim_start_matches("https://127.0.0.1:");
    let mut caps = sandbox.caps(CapabilityGrants {
        adhoc_net: vec![
            lca_permissions::parse_net_pattern(&format!("127.0.0.1:{host_port}"))
                .expect("literal with port"),
        ],
        ..CapabilityGrants::default()
    });
    // The fixture CA is the only trust the offline server can offer:
    // webpki roots never sign loopback, so the engine learns this one
    // anchor for the test (gh #94's additive host hook).
    caps.add_trust_anchor(TLS_CA_PEM.as_bytes())
        .expect("fixture CA parses");
    caps
}

/// One loopback server that 401s, so a request has something to reach.
pub(crate) fn refusing_server() -> String {
    let listener = StdTcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            use std::io::{Read, Write};
            let mut stream = stream;
            let mut buffer = [0u8; 4096];
            let _ = stream.read(&mut buffer);
            let _ = stream.write_all(
                b"HTTP/1.1 401 Unauthorized\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}",
            );
        }
    });
    format!("http://{addr}/mcp")
}

/// Capabilities wired so a flow-scoped request to `host` is permitted.
pub(crate) fn flow_caps(sandbox: &Sandbox) -> Capabilities {
    let caps = sandbox.caps(CapabilityGrants {
        net_declared: true,
        oauth: Some(OAuthSettings {
            redirect_path: "/callback".to_string(),
            timeout_seconds: 30,
        }),
        ..CapabilityGrants::default()
    });
    // The flow-scoped host set: `remember_unauthorized` attaches these before
    // its request, exactly as `sign_in_flow` does.
    caps.oauth_flow_attach(["127.0.0.1".to_string()])
        .expect("attach the loopback host");
    caps
}
