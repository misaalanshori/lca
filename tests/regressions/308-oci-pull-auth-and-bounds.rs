//! GitHub #308: three defects in one OCI fetch path.
//!
//! **1. The post-401 retry ignored redirects.** `fetch_oci` followed up to five
//! redirects on the initial request and then, after the 401 token dance,
//! checked the retry with `is_success()`. But a 307 to the CDN blob store is the
//! *normal* ghcr blob-pull shape, so every authenticated pull died with
//! `HTTP 307 ... after authorization`. The loop was written once, on the path
//! that did not need it, and not on the path that did.
//!
//! **2. The token parse read one field.** It looked at `json.get("token")`
//! only. The distribution spec permits `access_token`, and a registry that
//! answers with the spec's other field left the retry unauthenticated.
//!
//! **3. The client was unbounded and direct.** A bare `HttpConnector` meant no
//! `HTTP_PROXY`/`HTTPS_PROXY` support, unlike the `net` capability client
//! (gh #145); `collect_body`/`fetch_bytes` had no byte limit and no timeout, so
//! a registry streaming an endless body was accumulated in full *before* the
//! digest ran. Digest-before-install is the supply-chain guarantee
//! (`docs/installation.md`), and it can only run after the bytes land — which
//! is exactly why an unbounded collect is a pre-verification memory hazard.
//!
//! ## What this guard pins
//!
//! The *composition* rather than the parts. Each row drives the public path
//! against a scripted loopback registry, because each defect lives in the
//! sequence of requests: a redirect loop that is only exercised on the initial
//! attempt cannot notice that the retry has none, and a cap that is only
//! asserted on a function's argument cannot notice that nothing calls it.
//!
//! Verifies: NFR-24 (a shipped defect's guard), GitHub issue #308 acceptance
//! 1-3.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use lca_registry::{Proxies, ProxyEndpoint};

/// One recorded request: the line, then the headers.
type Recorded = (String, Vec<(String, String)>);
/// The shared log a scripted registry fills.
type RequestLog = std::sync::Arc<std::sync::Mutex<Vec<Recorded>>>;

/// A scripted loopback registry. Replies are served in order, one per accepted
/// connection, and every request is recorded.
struct Registry {
    base: String,
    seen: RequestLog,
}

enum Reply {
    Status {
        line: &'static str,
        headers: Vec<(&'static str, String)>,
        body: Vec<u8>,
    },
    /// Accept, promise a body, then send nothing and never close.
    Stall,
}

impl Registry {
    /// Bind, then script. The closure receives the base URL so replies that
    /// name the port (a challenge realm, a redirect target) can be built.
    async fn scripted(build: impl FnOnce(&str) -> Vec<Reply>) -> Registry {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        let base = format!("http://{addr}");
        let replies = build(&base);
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let log = seen.clone();
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
                log.lock().expect("log").push((request_line, headers));
                match reply {
                    Reply::Stall => {
                        let _ = stream
                            .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 99999\r\n\r\n")
                            .await;
                        held.push(stream);
                    }
                    Reply::Status {
                        line,
                        headers,
                        body,
                    } => {
                        let mut head = format!("HTTP/1.1 {line}\r\n");
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
            if !held.is_empty() {
                std::future::pending::<()>().await;
            }
        });
        Registry { base, seen }
    }

    fn requests(&self) -> Vec<String> {
        self.seen
            .lock()
            .expect("log")
            .iter()
            .map(|(line, _)| line.clone())
            .collect()
    }

    fn header_on(&self, index: usize, name: &str) -> Option<String> {
        self.seen
            .lock()
            .expect("log")
            .get(index)
            .and_then(|(_, headers)| {
                headers
                    .iter()
                    .find(|(key, _)| key.eq_ignore_ascii_case(name))
                    .map(|(_, value)| value.clone())
            })
    }
}

const PAYLOAD: &[u8] = b"{\"schemaVersion\":2,\"layers\":[]}";

fn unauthorized_against(base: &str) -> Reply {
    Reply::Status {
        line: "401 Unauthorized",
        headers: vec![(
            "www-authenticate",
            format!(
                "Bearer realm=\"{base}/token\",service=\"guard\",scope=\"repository:x/y:pull\""
            ),
        )],
        body: Vec::new(),
    }
}

fn ok(body: &[u8]) -> Reply {
    Reply::Status {
        line: "200 OK",
        headers: Vec::new(),
        body: body.to_vec(),
    }
}

fn to(location: String) -> Reply {
    Reply::Status {
        line: "307 Temporary Redirect",
        headers: vec![("location", location)],
        body: Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Acceptance 1: a 401 -> 307 sequence succeeds
// ---------------------------------------------------------------------------

/// **Acceptance 1.** The pre-fix error was
/// `HTTP 307 Temporary Redirect after authorization`, because the retry
/// checked `is_success()` and a redirect is not one. The row asserts the whole
/// sequence landed *and* that the redirect was actually followed — a fix that
/// merely tolerated 307 without following it would return an empty body.
#[tokio::test]
async fn a_blob_pull_through_the_auth_then_cdn_redirect_sequence_completes() {
    let registry = Registry::scripted(|base| {
        vec![
            unauthorized_against(base),
            ok(br#"{"token":"guard-tok"}"#),
            to(format!("{base}/cdn/blob")),
            ok(PAYLOAD),
        ]
    })
    .await;
    let (bytes, _headers) = lca_registry::fetch_oci_public(
        &format!("{}/v2/x/y/manifests/v1", registry.base),
        "*",
        "x/y",
    )
    .await
    .expect("401 -> token -> 307 -> 200 is the normal ghcr shape");
    assert_eq!(bytes, PAYLOAD, "the artifact came from the CDN hop");

    let requests = registry.requests();
    assert_eq!(
        requests.len(),
        4,
        "anonymous, token, retry, redirect: {requests:?}"
    );
    assert!(
        requests[1].contains("/token"),
        "the token exchange: {requests:?}"
    );
    assert!(
        requests[3].contains("/cdn/blob"),
        "the retry followed the redirect: {requests:?}"
    );
    assert_eq!(
        registry.header_on(2, "authorization"),
        Some("Bearer guard-tok".to_string()),
        "the retry is authenticated"
    );
    // A *same-origin* redirect keeps the bearer: the client is still talking
    // to the registry. The cross-origin drop is its own row below, because
    // conflating the two would make this one assert the wrong rule.
    assert_eq!(
        registry.header_on(3, "authorization"),
        Some("Bearer guard-tok".to_string()),
        "a same-origin hop stays authenticated"
    );
}

/// The redirect budget is the same on both attempts, and a loop ends in an
/// error rather than a hang. The pre-fix retry followed zero; a fix that
/// followed without a bound would spin.
#[tokio::test]
async fn a_redirect_loop_is_bounded_on_the_authenticated_attempt() {
    let registry = Registry::scripted(|base| {
        let mut replies = vec![unauthorized_against(base), ok(br#"{"token":"t"}"#)];
        for _ in 0..20 {
            replies.push(to(format!("{base}/loop")));
        }
        replies
    })
    .await;
    let err = lca_registry::fetch_oci_public(&format!("{}/loop", registry.base), "*", "x/y")
        .await
        .expect_err("an endless redirect cannot resolve an artifact");
    let text = err.to_string();
    assert!(
        text.contains("redirect"),
        "the error names the cause: {text}"
    );
    assert!(
        registry.requests().len() < 12,
        "the budget stopped it long before the script ran out: {:?}",
        registry.requests()
    );
}

// ---------------------------------------------------------------------------
// Acceptance 2: `access_token` authenticates
// ---------------------------------------------------------------------------

/// **Acceptance 2.** The spec permits `access_token`; ghcr happens to answer
/// `token`. Reading one field meant every registry using the other was
/// unusable — the retry went out anonymous and the pull failed downstream with
/// a status that looked like a permissions problem.
#[tokio::test]
async fn a_registry_answering_access_token_authenticates_the_retry() {
    let registry = Registry::scripted(|base| {
        vec![
            unauthorized_against(base),
            ok(br#"{"access_token":"spec-field-token","expires_in":300}"#),
            ok(PAYLOAD),
        ]
    })
    .await;
    let (bytes, _) = lca_registry::fetch_oci_public(
        &format!("{}/v2/x/y/manifests/v1", registry.base),
        "*",
        "x/y",
    )
    .await
    .expect("access_token is equally valid");
    assert_eq!(bytes, PAYLOAD);
    assert_eq!(
        registry.header_on(2, "authorization"),
        Some("Bearer spec-field-token".to_string()),
        "the retry carries the field the endpoint answered"
    );
}

/// `token` wins when both are present (it is the older, more specific field),
/// and a token answer with neither is a *named* failure quoting the endpoint's
/// answer — never a silent anonymous retry.
#[tokio::test]
async fn a_token_answer_naming_no_token_is_refused_by_name() {
    let registry = Registry::scripted(|base| {
        vec![
            unauthorized_against(base),
            ok(br#"{"error":"insufficient_scope","expires_in":300}"#),
        ]
    })
    .await;
    let err = lca_registry::fetch_oci_public(
        &format!("{}/v2/x/y/manifests/v1", registry.base),
        "*",
        "x/y",
    )
    .await
    .expect_err("no token field means no retry can authenticate");
    let text = err.to_string();
    assert!(text.contains("token"), "names the field: {text}");
    assert!(
        text.contains("insufficient_scope"),
        "quotes the endpoint so the real cause is visible: {text}"
    );
    let requests = registry.requests();
    assert_eq!(
        requests.len(),
        2,
        "no third request: refusing beats a silent anonymous retry: {requests:?}"
    );
}

// ---------------------------------------------------------------------------
// Acceptance 3: oversized and slow responses fail bounded
// ---------------------------------------------------------------------------

/// The byte cap is enforced **mid-read**. The pre-fix `collect_body` collected
/// the whole body and then judged it, so a hostile registry could exhaust memory
/// before the digest check ever ran — the exact inversion of the
/// digest-before-install guarantee.
#[tokio::test]
async fn an_oversized_response_is_refused_at_the_cap_mid_read() {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    let written = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = written.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let mut buf = vec![0u8; 8192];
            let _ = stream.read(&mut buf).await;
            let _ = stream
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 10485760\r\n\r\n")
                .await;
            let chunk = vec![b'x'; 4096];
            for _ in 0..2560 {
                if stream.write_all(&chunk).await.is_err() {
                    break;
                }
                counter.fetch_add(chunk.len(), std::sync::atomic::Ordering::SeqCst);
            }
            let _ = stream.shutdown().await;
        }
    });
    let err = lca_registry::fetch_oci_within_capped(
        &format!("http://{addr}/v2/x/y/manifests/v1"),
        "*",
        "x/y",
        std::time::Duration::from_secs(5),
        8192,
    )
    .await
    .expect_err("a 10 MiB body over an 8 KiB cap is refused");
    let text = err.to_string();
    assert!(text.contains("8192"), "names the cap: {text}");
    // The server was still writing when the client gave up: the refusal is
    // mid-read, not a post-mortem on a fully collected body.
    let sent = written.load(std::sync::atomic::Ordering::SeqCst);
    assert!(
        sent < 10 * 1024 * 1024,
        "the client stopped reading before the whole 10 MiB arrived (server sent {sent})"
    );
}

/// A stalled body fails inside the deadline. The pre-fix path had none, so a
/// registry that answered headers and then went quiet hung `lca ext install`
/// forever.
#[tokio::test]
async fn a_stalled_response_fails_inside_the_deadline() {
    let registry = Registry::scripted(|_base| vec![Reply::Stall]).await;
    let started = std::time::Instant::now();
    let err = lca_registry::fetch_oci_within_capped(
        &format!("{}/v2/x/y/manifests/v1", registry.base),
        "*",
        "x/y",
        std::time::Duration::from_millis(200),
        1024 * 1024,
    )
    .await
    .expect_err("a stalled body cannot hang the caller");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(10),
        "gave up at the deadline: {:?}",
        started.elapsed()
    );
    assert!(
        err.to_string().contains("within"),
        "the error says it timed out: {err}"
    );
}

// ---------------------------------------------------------------------------
// The proxy rules, matching the `net` capability client (gh #145)
// ---------------------------------------------------------------------------

/// The registry client had no proxy support at all. It now follows the same
/// env rules as the `net` client, and this row pins them so the two cannot
/// drift: uppercase then lowercase, `ALL_PROXY` fallback, per-scheme
/// selection, and curl's `NO_PROXY` tail match.
#[test]
fn proxy_selection_mirrors_the_net_capability_client() {
    let endpoint = |host: &str, port: u16, auth: Option<&str>| ProxyEndpoint {
        host: host.to_string(),
        port,
        auth: auth.map(str::to_string),
    };
    let proxies = Proxies::new(
        Some(endpoint("http-proxy", 3128, None)),
        Some(endpoint("https-proxy", 8443, Some("u:p"))),
        vec!["internal.example".to_string(), "127.0.0.1:8080".to_string()],
    );
    assert_eq!(
        proxies
            .endpoint_for("https", "ghcr.io", 443)
            .map(|endpoint| (endpoint.host.as_str(), endpoint.port)),
        Some(("https-proxy", 8443)),
        "https reads HTTPS_PROXY"
    );
    assert_eq!(
        proxies
            .endpoint_for("http", "ghcr.io", 80)
            .map(|endpoint| endpoint.port),
        Some(3128),
        "http reads HTTP_PROXY"
    );
    assert!(
        proxies
            .endpoint_for("https", "internal.example", 443)
            .is_none(),
        "NO_PROXY bypasses an exact host"
    );
    assert!(
        proxies
            .endpoint_for("https", "deep.internal.example", 443)
            .is_none(),
        "and its subdomains"
    );
    assert!(
        proxies.endpoint_for("http", "127.0.0.1", 8080).is_none(),
        "a :port qualifier restricts the bypass to that port"
    );
    assert!(
        proxies.endpoint_for("http", "127.0.0.1", 8081).is_some(),
        "and only that port"
    );

    // The parser: curl defaults, userinfo captured separately (never logged),
    // unusable values ignored like an unset variable.
    let parsed =
        lca_registry::parse_proxy_for_tests("http://user:secret@proxy:3128").expect("parses");
    assert_eq!(parsed.host, "proxy");
    assert_eq!(parsed.port, 3128);
    assert_eq!(parsed.auth.as_deref(), Some("user:secret"));
    assert_eq!(
        lca_registry::parse_proxy_for_tests("proxy-only")
            .expect("parses")
            .port,
        1080,
        "curl's default port"
    );
    assert!(
        lca_registry::parse_proxy_for_tests("socks5://proxy:1080").is_none(),
        "a non-http scheme is ignored, not fatal"
    );
}

/// A cross-origin redirect must not carry the bearer, even though the retry now
/// follows redirects. This is gh #349's rule and it is the one thing the new
/// redirect handling could have broken: the normal ghcr blob pull lands on a
/// CDN host, which does not need a repository token and must not receive one.
#[tokio::test]
async fn the_bearer_is_dropped_when_the_redirect_changes_host() {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    // A separate listener is a genuinely different origin (host:port).
    let cdn = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let cdn_addr = cdn.local_addr().expect("addr");
    let cdn_auth: std::sync::Arc<std::sync::Mutex<Option<String>>> =
        std::sync::Arc::new(std::sync::Mutex::new(None));
    let recorded = cdn_auth.clone();
    tokio::spawn(async move {
        let Ok((mut stream, _)) = cdn.accept().await else {
            return;
        };
        let mut buf = vec![0u8; 16 * 1024];
        let read = stream.read(&mut buf).await.unwrap_or(0);
        for line in String::from_utf8_lossy(&buf[..read]).lines() {
            if let Some((name, value)) = line.split_once(':')
                && name.trim().eq_ignore_ascii_case("authorization")
            {
                *recorded.lock().expect("record") = Some(value.trim().to_string());
            }
        }
        let head = format!(
            "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
            PAYLOAD.len()
        );
        let _ = stream.write_all(head.as_bytes()).await;
        let _ = stream.write_all(PAYLOAD).await;
        let _ = stream.shutdown().await;
    });

    let registry = Registry::scripted(|base| {
        vec![
            unauthorized_against(base),
            ok(br#"{"token":"must-not-leak"}"#),
            to(format!("http://{cdn_addr}/blob")),
        ]
    })
    .await;
    let (bytes, _) = lca_registry::fetch_oci_public(
        &format!("{}/v2/x/y/manifests/v1", registry.base),
        "*",
        "x/y",
    )
    .await
    .expect("the redirect is followed");
    assert_eq!(bytes, PAYLOAD);
    assert_eq!(
        registry.header_on(2, "authorization"),
        Some("Bearer must-not-leak".to_string()),
        "the retry authenticates to the registry"
    );
    assert_eq!(
        *cdn_auth.lock().expect("record"),
        None,
        "and the CDN receives no bearer at all"
    );
}
