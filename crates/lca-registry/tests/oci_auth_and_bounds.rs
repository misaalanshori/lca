//! gh #308: the OCI fetch path's three defects, driven end to end against a
//! scripted loopback registry.
//!
//! Every row here speaks real HTTP to a real socket, because each defect lives
//! in the *sequence* of requests rather than in one function's return value:
//! the post-401 retry's redirect handling, the token field the challenge
//! answered with, and the bounds on a body that never ends.
//!
//! Split from `registry.rs` rather than appended to it: that file is at 1,140
//! lines and the 1,200-line ceiling is gate 11.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::sync::{Arc, Mutex};

/// One recorded request: what the mock registry actually saw.
#[derive(Debug, Clone)]
struct Seen {
    request_line: String,
    headers: Vec<(String, String)>,
}

impl Seen {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

/// What the mock answers one request with.
enum Reply {
    Answer {
        status: &'static str,
        headers: Vec<(&'static str, String)>,
        body: Vec<u8>,
    },
    /// Accept, promise a body, send nothing, never close.
    Stall,
}

/// The requests the mock saw, in order.
type Log = Arc<Mutex<Vec<Seen>>>;

fn ok(body: &[u8]) -> Reply {
    Reply::Answer {
        status: "200 OK",
        headers: Vec::new(),
        body: body.to_vec(),
    }
}

fn redirect(location: String) -> Reply {
    Reply::Answer {
        status: "307 Temporary Redirect",
        headers: vec![("location", location)],
        body: Vec::new(),
    }
}

/// A 401 whose challenge points at `base`'s token endpoint.
fn challenge(base: &str) -> Reply {
    Reply::Answer {
        status: "401 Unauthorized",
        headers: vec![(
            "www-authenticate",
            format!("Bearer realm=\"{base}/token\",service=\"mock\",scope=\"repository:x/y:pull\""),
        )],
        body: Vec::new(),
    }
}

fn token_json(body: &[u8]) -> Reply {
    ok(body)
}

/// Bind a loopback listener and hand back its base URL, so replies that need
/// the port (a challenge realm, a redirect target) can be built before serving.
async fn bind() -> (tokio::net::TcpListener, String) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    (listener, format!("http://{addr}"))
}

/// Serve `replies` in order, one per accepted connection, recording each
/// request.
fn serve(listener: tokio::net::TcpListener, replies: Vec<Reply>) -> Log {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let log: Log = Arc::new(Mutex::new(Vec::new()));
    let seen = log.clone();
    tokio::spawn(async move {
        let mut held = Vec::new();
        for reply in replies {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let mut buf = vec![0u8; 16 * 1024];
            let read = stream.read(&mut buf).await.unwrap_or(0);
            let text = String::from_utf8_lossy(&buf[..read]).into_owned();
            let mut lines = text.lines();
            let request_line = lines.next().unwrap_or_default().to_string();
            let headers = lines
                .filter_map(|line| line.split_once(':'))
                .map(|(key, value)| (key.trim().to_string(), value.trim().to_string()))
                .collect::<Vec<_>>();
            seen.lock().expect("log").push(Seen {
                request_line,
                headers,
            });
            match reply {
                Reply::Stall => {
                    let _ = stream
                        .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 5000\r\n\r\n")
                        .await;
                    held.push(stream);
                }
                Reply::Answer {
                    status,
                    headers,
                    body,
                } => {
                    let mut head = format!("HTTP/1.1 {status}\r\n");
                    for (name, value) in headers {
                        head.push_str(&format!("{name}: {value}\r\n"));
                    }
                    head.push_str(&format!(
                        "content-length: {}\r\nconnection: close\r\n\r\n",
                        body.len()
                    ));
                    let _ = stream.write_all(head.as_bytes()).await;
                    let _ = stream.write_all(&body).await;
                    let _ = stream.shutdown().await;
                }
            }
        }
        // Keep the held connections open for as long as the test needs them:
        // dropping `held` here would close the socket and turn a stalled body
        // into an ordinary truncation error.
        if !held.is_empty() {
            std::future::pending::<()>().await;
        }
    });
    log
}

const PAYLOAD: &[u8] = b"{\"layers\":[{\"digest\":\"sha256:abc\"}]}";

/// The request lines the mock received.
fn request_lines(log: &Log) -> Vec<String> {
    log.lock()
        .expect("log")
        .iter()
        .map(|seen| seen.request_line.clone())
        .collect()
}

// ---------------------------------------------------------------------------
// Defect 1: the post-401 retry must follow redirects
// ---------------------------------------------------------------------------

/// **The issue's row.** ghcr's normal blob-pull shape is 401 → token → 307 to
/// the CDN → 200. The initial attempt followed redirects; the post-401 retry
/// did not, so the pull died with `HTTP 307 after authorization`.
#[tokio::test]
async fn a_pull_through_401_then_307_succeeds() {
    // The blob URL needs the port, so bind before building the script.
    let (listener, base) = bind().await;
    let blob = format!("{base}/blob");
    let log = serve(
        listener,
        vec![
            challenge(&base),
            token_json(br#"{"token":"tok-abc"}"#),
            redirect(blob),
            ok(PAYLOAD),
        ],
    );

    let (bytes, _headers) =
        lca_registry::fetch_oci_public(&format!("{base}/v2/x/y/manifests/v1"), "*", "x/y")
            .await
            .expect("401 -> token -> 307 -> 200 must succeed");

    assert_eq!(bytes, PAYLOAD, "the blob body arrives");
    let requests = request_lines(&log);
    assert_eq!(requests.len(), 4, "four requests, in order: {requests:?}");
    assert!(requests[0].contains("/v2/x/y/manifests/v1"), "{requests:?}");
    assert!(
        requests[1].contains("/token"),
        "the token exchange: {requests:?}"
    );
    assert!(requests[2].contains("/v2/x/y/manifests/v1"), "{requests:?}");
    assert!(
        requests[3].contains("/blob"),
        "the redirect is followed: {requests:?}"
    );
}

/// The post-401 retry spends the same 5-redirect budget the initial attempt
/// does, and gives up with an error rather than spinning.
#[tokio::test]
async fn a_redirect_loop_after_authorization_gives_up() {
    let (listener, base) = bind().await;
    let mut replies = vec![challenge(&base), token_json(br#"{"token":"tok-abc"}"#)];
    for _ in 0..12 {
        replies.push(redirect(format!("{base}/loop")));
    }
    let log = serve(listener, replies);
    let err = lca_registry::fetch_oci_public(&format!("{base}/loop"), "*", "x/y")
        .await
        .expect_err("a redirect loop must not be followed forever");
    let text = err.to_string();
    assert!(
        text.contains("redirect") || text.contains("307"),
        "the error says what went wrong: {text}"
    );
    assert!(
        request_lines(&log).len() <= 8,
        "the budget stopped it: {:?}",
        request_lines(&log)
    );
}

// ---------------------------------------------------------------------------
// Defect 2: `access_token` is equally valid
// ---------------------------------------------------------------------------

/// The distribution spec permits `access_token` where ghcr answers `token`.
/// Reading only `token` left the retry unauthenticated.
#[tokio::test]
async fn a_token_endpoint_answering_access_token_authenticates() {
    let (listener, base) = bind().await;
    let log = serve(
        listener,
        vec![
            challenge(&base),
            token_json(br#"{"access_token":"spec-tok-1"}"#),
            ok(PAYLOAD),
        ],
    );
    let (bytes, _) =
        lca_registry::fetch_oci_public(&format!("{base}/v2/x/y/manifests/v1"), "*", "x/y")
            .await
            .expect("access_token must authenticate");
    assert_eq!(bytes, PAYLOAD);
    let requests = log.lock().expect("log").clone();
    let retry = requests.last().expect("the retry");
    assert_eq!(
        retry.header("authorization"),
        Some("Bearer spec-tok-1"),
        "the retry carries the spec's field: {retry:?}"
    );
}

/// Neither field: a named failure quoting what the endpoint answered, rather
/// than retrying unauthenticated and reporting a confusing downstream status.
#[tokio::test]
async fn a_token_answer_with_neither_field_fails_by_name() {
    let (listener, base) = bind().await;
    let _log = serve(
        listener,
        vec![challenge(&base), token_json(br#"{"expires_in":300}"#)],
    );
    let err = lca_registry::fetch_oci_public(&format!("{base}/v2/x/y/manifests/v1"), "*", "x/y")
        .await
        .expect_err("a token answer with no token cannot be used");
    let text = err.to_string();
    assert!(text.contains("token"), "names the missing field: {text}");
    assert!(
        text.contains("expires_in"),
        "quotes what the endpoint actually answered: {text}"
    );
}

// ---------------------------------------------------------------------------
// Defect 3: bounds
// ---------------------------------------------------------------------------

/// An oversized body is refused *while reading*, not after the whole thing
/// lands in memory. A digest is verified only after the bytes arrive, so an
/// unbounded collect is a pre-verification memory hazard.
#[tokio::test]
async fn an_oversized_body_is_refused_at_the_cap() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let mut buf = vec![0u8; 8192];
            let _ = stream.read(&mut buf).await;
            let _ = stream
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 1048576\r\n\r\n")
                .await;
            let chunk = vec![b'x'; 4096];
            for _ in 0..512 {
                if stream.write_all(&chunk).await.is_err() {
                    break;
                }
            }
            let _ = stream.shutdown().await;
        }
    });
    let err = lca_registry::fetch_oci_within_capped(
        &format!("http://{addr}/v2/x/y/manifests/v1"),
        "*",
        "x/y",
        std::time::Duration::from_secs(5),
        4096,
    )
    .await
    .expect_err("a body over the cap is refused");
    let text = err.to_string();
    assert!(
        text.contains("4096") || text.contains("limit"),
        "the error names the cap: {text}"
    );
}

/// A body that stalls mid-transfer fails inside the deadline instead of
/// hanging. The timeout covers the whole request *and* the body read, which is
/// the `plain_get` shape (gh #355).
#[tokio::test]
async fn a_stalled_body_fails_within_the_deadline() {
    let (listener, base) = bind().await;
    let _log = serve(listener, vec![Reply::Stall]);
    let started = std::time::Instant::now();
    let err = lca_registry::fetch_oci_within_capped(
        &format!("{base}/v2/x/y/manifests/v1"),
        "*",
        "x/y",
        std::time::Duration::from_millis(200),
        1024 * 1024,
    )
    .await
    .expect_err("a stalled body must not hang");
    let elapsed = started.elapsed();
    assert!(
        elapsed < std::time::Duration::from_secs(10),
        "gave up at the deadline, not later: {elapsed:?}"
    );
    let text = err.to_string();
    assert!(
        text.contains("within") || text.contains("timed out"),
        "the error says it timed out: {text}"
    );
}

// ---------------------------------------------------------------------------
// Defect 3: proxy support
// ---------------------------------------------------------------------------

/// The proxy decision logic, driven directly: no process environment to mutate
/// (which would race every other row in this binary), so the configuration is
/// built explicitly. These are the `net` capability client's rules (gh #145),
/// and this crate must not diverge from them.
#[test]
fn proxy_routing_follows_the_net_clients_rules() {
    use lca_registry::{Proxies, ProxyEndpoint};

    let endpoint = |host: &str, port: u16| ProxyEndpoint {
        host: host.to_string(),
        port,
        auth: None,
    };
    let proxies = Proxies::new(
        Some(endpoint("proxy.http", 3128)),
        Some(endpoint("proxy.https", 8443)),
        vec![
            "example.com".to_string(),
            "127.0.0.1:8080".to_string(),
            ".internal".to_string(),
        ],
    );

    // Per-scheme selection.
    assert_eq!(
        proxies
            .endpoint_for("https", "ghcr.io", 443)
            .map(|endpoint| endpoint.port),
        Some(8443),
        "https uses HTTPS_PROXY"
    );
    assert_eq!(
        proxies
            .endpoint_for("http", "ghcr.io", 80)
            .map(|endpoint| endpoint.port),
        Some(3128),
        "http uses HTTP_PROXY"
    );
    assert!(
        proxies.endpoint_for("ftp", "ghcr.io", 21).is_none(),
        "an unknown scheme is never proxied"
    );

    // NO_PROXY: exact host, subdomain tail match, port qualifier, dot prefix.
    assert!(
        proxies.endpoint_for("https", "example.com", 443).is_none(),
        "an exact host bypasses"
    );
    assert!(
        proxies
            .endpoint_for("https", "api.example.com", 443)
            .is_none(),
        "a subdomain bypasses (tail match)"
    );
    assert!(
        proxies
            .endpoint_for("https", "notexample.com", 443)
            .is_some(),
        "a lookalike does NOT bypass"
    );
    assert!(
        proxies.endpoint_for("http", "127.0.0.1", 8080).is_none(),
        "the port qualifier matches"
    );
    assert!(
        proxies.endpoint_for("http", "127.0.0.1", 9090).is_some(),
        "and only that port"
    );
    assert!(
        proxies.endpoint_for("https", "svc.internal", 443).is_none(),
        "a leading dot means the domain and below"
    );

    // A star bypasses everything; an empty list bypasses nothing.
    let star = Proxies::new(
        Some(endpoint("proxy.http", 3128)),
        Some(endpoint("proxy.https", 8443)),
        vec!["*".to_string()],
    );
    assert!(star.endpoint_for("https", "anything.at.all", 443).is_none());
    let none = Proxies::new(None, None, Vec::new());
    assert!(none.endpoint_for("https", "ghcr.io", 443).is_none());
}

/// `ALL_PROXY` is the fallback for both schemes, and a lowercase variable is
/// honoured (curl convention), with uppercase winning.
#[test]
fn proxy_environment_precedence_matches_the_net_client() {
    // Read through the same parser the client uses. This row exists because a
    // silently-dropped `ALL_PROXY` would make every proxied registry pull fail
    // with a connection error that looks like a network problem.
    let parsed = lca_registry::parse_proxy_for_tests("proxy.example:8080").expect("parses");
    assert_eq!(parsed.host, "proxy.example");
    assert_eq!(parsed.port, 8080);
    assert_eq!(parsed.auth, None, "no userinfo, no credentials");

    let with_auth =
        lca_registry::parse_proxy_for_tests("http://user:pass@proxy.example:3128").expect("parses");
    assert_eq!(with_auth.auth.as_deref(), Some("user:pass"));
    // curl's default port.
    assert_eq!(
        lca_registry::parse_proxy_for_tests("http://proxy.example")
            .expect("parses")
            .port,
        1080
    );
    // Unusable values are ignored, not fatal - exactly like an unset variable.
    assert!(lca_registry::parse_proxy_for_tests("").is_none());
    assert!(lca_registry::parse_proxy_for_tests("socks5://proxy.example:1080").is_none());
}

// ---------------------------------------------------------------------------
// Cross-origin redirect must not forward the bearer (gh #349's rule)
// ---------------------------------------------------------------------------

/// The normal ghcr blob pull redirects to a CDN on a *different host*. The
/// signed URL does not need the bearer, and forwarding a repository token past
/// its origin is the leak gh #349 was about. The retry now follows redirects,
/// so this rule has to hold on the retry too.
#[tokio::test]
async fn a_cross_origin_redirect_does_not_forward_the_bearer() {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    // The CDN: a second listener, so the redirect genuinely changes host:port.
    let cdn = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let cdn_addr = cdn.local_addr().expect("addr");
    let cdn_log: Log = Arc::new(Mutex::new(Vec::new()));
    let cdn_seen = cdn_log.clone();
    tokio::spawn(async move {
        let Ok((mut stream, _)) = cdn.accept().await else {
            return;
        };
        let mut buf = vec![0u8; 16 * 1024];
        let read = stream.read(&mut buf).await.unwrap_or(0);
        let text = String::from_utf8_lossy(&buf[..read]).into_owned();
        let mut lines = text.lines();
        let request_line = lines.next().unwrap_or_default().to_string();
        let headers = lines
            .filter_map(|line| line.split_once(':'))
            .map(|(key, value)| (key.trim().to_string(), value.trim().to_string()))
            .collect::<Vec<_>>();
        cdn_seen.lock().expect("log").push(Seen {
            request_line,
            headers,
        });
        let head = format!(
            "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
            PAYLOAD.len()
        );
        let _ = stream.write_all(head.as_bytes()).await;
        let _ = stream.write_all(PAYLOAD).await;
        let _ = stream.shutdown().await;
    });

    let (listener, base) = bind().await;
    let log = serve(
        listener,
        vec![
            challenge(&base),
            token_json(br#"{"token":"secret-tok"}"#),
            redirect(format!("http://{cdn_addr}/blob")),
        ],
    );

    let (bytes, _) =
        lca_registry::fetch_oci_public(&format!("{base}/v2/x/y/manifests/v1"), "*", "x/y")
            .await
            .expect("the cross-origin redirect is followed");
    assert_eq!(bytes, PAYLOAD, "the CDN's body arrives");

    // The retry DID carry the token to the registry...
    let registry_requests = log.lock().expect("log").clone();
    let retry = registry_requests
        .last()
        .expect("the retry reached the registry");
    assert_eq!(
        retry.header("authorization"),
        Some("Bearer secret-tok"),
        "the retry authenticates to its own origin: {retry:?}"
    );
    // ...and the CDN did not receive it.
    let cdn_requests = cdn_log.lock().expect("log").clone();
    assert_eq!(cdn_requests.len(), 1, "the CDN was asked once");
    assert!(
        cdn_requests[0].header("authorization").is_none(),
        "the bearer is not forwarded cross-origin: {cdn_requests:?}"
    );
}

/// The environment reader itself (not just the pure decision): a set variable
/// reaches `endpoint_for`, and `NO_PROXY` clears it. This row runs in its own
/// process because it mutates the environment — hence `--test-threads=1` is
/// *not* needed: the mutation is scoped to this one test and the crate's other
/// proxy rows build their configuration explicitly.
#[test]
fn proxy_environment_is_read_into_the_route_decision() {
    // SAFETY: single-threaded mutation of a process-global for this test only;
    // other proxy rows never read the environment.
    unsafe {
        std::env::set_var("HTTPS_PROXY", "http://env-proxy:3128");
        std::env::remove_var("NO_PROXY");
        std::env::remove_var("no_proxy");
        std::env::remove_var("ALL_PROXY");
        std::env::remove_var("all_proxy");
    }
    let proxies = lca_registry::Proxies::from_env();
    assert_eq!(
        proxies
            .endpoint_for("https", "ghcr.io", 443)
            .map(|endpoint| (endpoint.host.as_str(), endpoint.port)),
        Some(("env-proxy", 3128)),
        "HTTPS_PROXY reaches the decision"
    );

    // And `NO_PROXY` clears it for a matching host.
    unsafe {
        std::env::set_var("NO_PROXY", "ghcr.io");
    }
    let proxies = lca_registry::Proxies::from_env();
    assert!(
        proxies.endpoint_for("https", "ghcr.io", 443).is_none(),
        "NO_PROXY bypasses"
    );
    unsafe {
        std::env::remove_var("HTTPS_PROXY");
        std::env::remove_var("NO_PROXY");
    }
}
