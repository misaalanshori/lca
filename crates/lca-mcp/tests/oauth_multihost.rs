//! gh #418 Phase D (gh #290): OAuth discovery across two hosts.
//!
//! The defect had three parts, and each has rows here:
//!
//! 1. `sign_in` passed an empty challenge map to `discover`, so the RFC 9728
//!    `resource_metadata` branch was dead during sign-in — the server had just
//!    told us where its authorization server lives and we threw it away.
//! 2. Grant assembly granted only the MCP server's own host, so a metadata
//!    fetch or token exchange on a *different* host was denied by the
//!    capability engine before any prompt.
//! 3. Nothing validated that a discovered endpoint was `https://` (or loopback
//!    `http://`), so config or metadata could send a bearer token to a plain
//!    `http://` host anywhere.
//!
//! **Two hosts here, not two ports**: a `NetPattern` names a host, not a
//! `host:port` pair, so `127.0.0.1:18980` and `127.0.0.1:18981` would both be
//! covered by one `127.0.0.1` grant and the defect could not be expressed.
//! Both hosts are loopback, so plain `http://` is legal for both (RFC 8252).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

use lca_mcp::{OAuthConfig, TokenStore};
use lca_permissions::{Decision, GrantStore, PermissionPrompt, ProposalDiff, ScopeRoots};

struct Allow;

impl PermissionPrompt for Allow {
    fn ask(&mut self, _action: &lca_permissions::Action) -> Decision {
        Decision::Always
    }

    fn review_proposals(&mut self, _diff: &ProposalDiff) -> bool {
        false
    }
}

/// A recorded request: method, path, raw headers, body.
type Recorded = (String, String, String, String);

/// The second loopback host to use for the IdP.
///
/// Two *hosts* are required (a `NetPattern` names a host, not a host:port
/// pair), but which second host is bindable is platform-dependent: Linux
/// accepts the whole 127.0.0.0/8, while macOS accepts `127.0.0.1` and
/// `localhost` only (`EADDRNOTAVAIL` for `127.0.0.2`, which is how CI run
/// 39342838847 found this). `localhost` and `127.0.0.1` are distinct host
/// strings that both resolve to loopback, so `http://` is legal for both and
/// the capability engine treats them as separate grants.
fn second_host() -> &'static str {
    for candidate in ["127.0.0.2", "localhost"] {
        if let Ok(listener) = std::net::TcpListener::bind((candidate, 0)) {
            drop(listener);
            return candidate;
        }
    }
    "localhost"
}

/// One raw-TCP host. Raw TCP rather than a real HTTP server because that is
/// the shape `tests/common.rs` established and the engine speaks plain
/// HTTP/1.1 over loopback, so nothing is simulated at the layer under test.
struct Host {
    /// `http://<ip>:<port>`
    base: String,
    requests: Arc<Mutex<Vec<Recorded>>>,
}

impl Host {
    /// The paths this host was asked for, in order.
    fn paths(&self) -> Vec<String> {
        self.requests
            .lock()
            .expect("requests")
            .iter()
            .map(|(_, path, _, _)| path.clone())
            .collect()
    }
}

/// Serve one loopback host. `answer` maps (method, path) to (status, body,
/// extra headers); `base` is the host's own `http://ip:port`, so metadata can
/// name itself rather than inventing a third host.
fn host(
    ip: &str,
    answer: impl Fn(&str, &str, &str) -> (u16, String, Vec<(String, String)>) + Send + Sync + 'static,
) -> Host {
    let listener = std::net::TcpListener::bind((ip, 0)).expect("bind");
    let addr = listener.local_addr().expect("addr");
    // The requested host NAME, not the resolved address: a grant is keyed by
    // the host string a URL carries, and on macOS `localhost` resolves to
    // `::1` — building the base from `addr` would collapse the IdP onto the
    // MCP server's host and make every cross-host row in this file vacuous.
    let base = format!("http://{ip}:{}", addr.port());
    let base_for_answers = base.clone();
    let requests: Arc<Mutex<Vec<Recorded>>> = Arc::new(Mutex::new(Vec::new()));
    let recorded = requests.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut stream = stream;
            let mut buffer = vec![0u8; 64 * 1024];
            let read = match stream.read(&mut buffer) {
                Ok(read) if read > 0 => read,
                _ => continue,
            };
            let text = String::from_utf8_lossy(&buffer[..read]).into_owned();
            let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
            let mut lines = head.lines();
            let request_line = lines.next().unwrap_or("");
            let mut parts = request_line.split_whitespace();
            let method = parts.next().unwrap_or("").to_string();
            let path = parts.next().unwrap_or("").to_string();
            let headers = lines.map(str::to_string).collect::<Vec<_>>().join("\n");
            recorded.lock().expect("record").push((
                method,
                path.clone(),
                headers.clone(),
                body.to_string(),
            ));
            let (status, payload, extra) = answer(&path, &headers, &base_for_answers);
            let reason = match status {
                200 => "OK",
                401 => "Unauthorized",
                404 => "Not Found",
                _ => "Error",
            };
            let mut response = format!(
                "HTTP/1.1 {status} {reason}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n",
                payload.len()
            );
            for (name, value) in extra {
                response.push_str(&format!("{name}: {value}\r\n"));
            }
            response.push_str("\r\n");
            response.push_str(&payload);
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        }
    });
    Host { base, requests }
}

/// A pattern rendered for an assertion message.
fn describe(pattern: &lca_permissions::NetPattern) -> String {
    // A host probe recovers the text without widening the public API for a
    // test. Both the MCP host and the dynamically chosen IdP host are probed,
    // so an assertion can tell them apart on any platform.
    for candidate in ["127.0.0.1", second_host(), "localhost"] {
        if pattern.matches_host(candidate) {
            return candidate.to_string();
        }
    }
    "other".to_string()
}

/// A capability engine shaped the way the manager builds one: `process`,
/// workspace `fs`, `oauth`, `credentials`, and exactly the *static* hosts the
/// config names — which is the over-narrow part of the defect.
fn engine(name: &str, net_hosts: &[&str]) -> (Arc<lca_tools::Capabilities>, std::path::PathBuf) {
    let root = lca_testkit::scratch_path(&format!("lca-mcp-d-{name}"));
    for part in ["workspace", "private", "config", "data", "tmp"] {
        std::fs::create_dir_all(root.join(part)).expect("mkdir");
    }
    let mut grants = lca_mcp::manifest_grants();
    grants.oauth = Some(lca_permissions::OAuthSettings {
        redirect_path: "/callback".to_string(),
        timeout_seconds: 30,
    });
    grants.credentials = true;
    for host in net_hosts {
        grants
            .adhoc_net
            .push(lca_permissions::parse_net_pattern(host).expect("pattern"));
    }
    let caps = Arc::new(lca_tools::Capabilities::new(
        "mcp",
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
            GrantStore::open(&root.join("grants.json")).expect("grants"),
        )),
        root.join("workspace"),
        None,
    ));
    // The browser launcher is a recorder: a headless test opens nothing, and
    // `oauth_open` reports `no web browser found` otherwise.
    caps.set_browser_opener(Some(Arc::new(|_url: &str| Ok(()))));
    (caps, root)
}

fn server_config(url: &str, oauth: Option<OAuthConfig>) -> lca_mcp::HttpServerConfig {
    lca_mcp::HttpServerConfig {
        name: "remote".to_string(),
        url: url.to_string(),
        headers: Default::default(),
        timeout_secs: 5,
        oauth,
    }
}

/// The MCP server: always 401, challenging with a metadata URL on `idp_base`.
fn mcp_host(idp_base: &str) -> Host {
    let metadata_url = format!("{idp_base}/.well-known/oauth-protected-resource");
    host("127.0.0.1", move |_path, _headers, _base| {
        (
            401,
            r#"{"jsonrpc":"2.0","id":0,"error":{"code":-32001,"message":"unauthorized"}}"#
                .to_string(),
            vec![(
                "WWW-Authenticate".to_string(),
                format!(r#"Bearer resource_metadata="{metadata_url}", scope="resources:read""#),
            )],
        )
    })
}

/// The identity provider: protected-resource metadata, authorization server
/// metadata, dynamic registration, and the token endpoint, all on its own
/// host. `metadata_base` lets a row point discovery somewhere else (the
/// scheme-validation rows do).
fn idp_host(metadata_base: Option<&'static str>) -> Host {
    host(second_host(), move |path, _headers, base| {
        let named = metadata_base.unwrap_or(base);
        if path.contains("oauth-protected-resource") {
            return (
                200,
                format!(r#"{{"resource":"{named}/mcp","authorization_servers":["{named}"]}}"#),
                Vec::new(),
            );
        }
        if path.contains("oauth-authorization-server") {
            return (
                200,
                format!(
                    r#"{{"issuer":"{named}","authorization_endpoint":"{named}/authorize","token_endpoint":"{named}/token","registration_endpoint":"{named}/register"}}"#
                ),
                Vec::new(),
            );
        }
        if path.contains("/register") {
            return (
                200,
                r#"{"client_id":"d-client","client_secret":"d-secret"}"#.to_string(),
                Vec::new(),
            );
        }
        if path.contains("/token") {
            return (
                    200,
                    r#"{"access_token":"d-access","refresh_token":"d-refresh","expires_in":1800,"scope":"resources:read"}"#
                        .to_string(),
                    Vec::new(),
                );
        }
        (404, "{}".to_string(), Vec::new())
    })
}

/// Answer the flow with a pasted callback, the way a browser that cannot
/// reach the loopback would: the manual-delivery seam the existing sign-in
/// rows use (`oauth_deliver_manual`), which wakes `oauth_await` directly.
fn answer_callback(caps: Arc<lca_tools::Capabilities>) {
    std::thread::spawn(move || {
        for _ in 0..600 {
            let opened = caps.oauth_opened();
            if let Some(url) = opened.last() {
                let field = |name: &str| {
                    url.split(&format!("{name}="))
                        .nth(1)
                        .and_then(|rest| rest.split('&').next())
                        .unwrap_or("")
                };
                let state = field("state").to_string();
                if !state.is_empty() {
                    let _ = caps.oauth_deliver_manual(vec![
                        ("code".to_string(), "d-code".to_string()),
                        ("state".to_string(), state),
                    ]);
                    return;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    });
}

// Verifies: gh #418 Phase D (gh #290) - the round-trip the issue names. A 401
// carrying `resource_metadata` and `scope` must remember *both*; before the
// fix only `scope` was persisted and the RFC 9728 branch was dead.
#[test]
fn the_401_challenge_survives_to_discovery() {
    let idp = idp_host(None);
    let mcp = mcp_host(&idp.base);
    let (caps, _root) = engine("challenge", &["127.0.0.1"]);
    let server = server_config(&format!("{}/mcp", mcp.base), Some(OAuthConfig::default()));
    let store = TokenStore::new(caps.clone(), &server);
    assert!(
        store.pending_challenge().expect("read").is_none(),
        "nothing pending before a 401"
    );

    lca_mcp::remote::remember_unauthorized(&caps, &server, &format!("{}/mcp", mcp.base))
        .expect("the 401 was captured");

    let pending = store
        .pending_challenge()
        .expect("read")
        .expect("the challenge is remembered");
    assert_eq!(
        pending.get("resource_metadata").map(String::as_str),
        Some(format!("{}/.well-known/oauth-protected-resource", idp.base).as_str()),
        "the header's resource_metadata survives to the next sign-in: {pending:?}"
    );
    assert_eq!(
        pending.get("scope").map(String::as_str),
        Some("resources:read"),
        "the header's scope survives: {pending:?}"
    );
}

// Verifies: gh #418 Phase D (gh #290) - the full journey across two hosts. The
// MCP server's 401 names metadata on host B; discovery follows it there; the
// authorization server, registration, and token exchange are all host B; the
// sign-in completes and stores a token. Before the fix the first metadata
// fetch was denied by the one-host grant.
#[test]
fn a_sign_in_reaches_an_authorization_server_on_another_host() {
    // The premise of every row in this file: two *distinct* host strings, both
    // loopback. A platform where they collapsed would make the cross-host
    // rows prove nothing, so it is asserted rather than assumed.
    assert_ne!(
        second_host(),
        "127.0.0.1",
        "the IdP must be a different host string than the MCP server"
    );
    let idp = idp_host(None);
    let mcp = mcp_host(&idp.base);
    let (caps, _root) = engine("cross-host", &["127.0.0.1"]);
    let server = server_config(&format!("{}/mcp", mcp.base), Some(OAuthConfig::default()));

    lca_mcp::remote::remember_unauthorized(&caps, &server, &format!("{}/mcp", mcp.base))
        .expect("the 401 was captured");
    answer_callback(caps.clone());

    let outcome = lca_mcp::sign_in(caps.clone(), &server);
    assert!(
        outcome.is_ok(),
        "the cross-host sign-in completes: {outcome:?}"
    );

    let store = TokenStore::new(caps.clone(), &server);
    let token = store
        .load()
        .expect("read token")
        .expect("a token is stored");
    assert_eq!(token.access_token, "d-access");

    let seen = idp.paths();
    assert!(
        !seen.is_empty(),
        "the IdP host was actually reached (base {} vs MCP {})",
        idp.base,
        mcp.base
    );
    for (needle, what) in [
        ("oauth-protected-resource", "the challenge's metadata fetch"),
        ("oauth-authorization-server", "RFC 8414 discovery"),
        ("/register", "dynamic registration"),
        ("/token", "the token exchange"),
    ] {
        assert!(
            seen.iter().any(|path| path.contains(needle)),
            "{what} ran on host B: {seen:?}"
        );
    }
}

// Verifies: gh #418 Phase D (gh #290) - the same journey refuses when the
// discovered authorization server is plain `http://` on a non-loopback host,
// and the refusal names the URL and the rule. Loopback `http://` is legal
// (RFC 8252); anything else must be TLS.
#[test]
fn a_discovered_endpoint_on_a_plain_http_host_is_refused() {
    // `.invalid` never resolves and cannot be reached: the refusal must come
    // from validation, not from a connection failure.
    let idp = idp_host(Some("http://idp.example.invalid"));
    let mcp = mcp_host(&idp.base);
    let (caps, _root) = engine("insecure", &["127.0.0.1"]);
    let server = server_config(&format!("{}/mcp", mcp.base), Some(OAuthConfig::default()));

    lca_mcp::remote::remember_unauthorized(&caps, &server, &format!("{}/mcp", mcp.base))
        .expect("the 401 was captured");
    let err = lca_mcp::sign_in(caps.clone(), &server).expect_err("plain http is refused");
    assert!(
        err.contains("idp.example.invalid") && err.contains("https"),
        "the refusal names the URL and the rule: {err}"
    );
    // Nothing was sent to the insecure host: the check is before the request.
    assert!(
        idp.paths().iter().all(|path| !path.contains("/token")),
        "no token exchange was attempted: {:?}",
        idp.paths()
    );
}

// Verifies: gh #418 Phase D (gh #290) - a host granted for the OAuth flow is
// **not** reachable for ordinary traffic, and the row checks both halves of
// that: the host set is genuinely non-empty while the flow runs (so the
// "afterwards" assertion is not vacuous), and empty once it ends.
#[test]
fn an_oauth_flow_host_does_not_survive_into_tool_traffic() {
    let idp = idp_host(None);
    let mcp = mcp_host(&idp.base);
    let (caps, root) = engine("flow-scope", &["127.0.0.1"]);
    let server = server_config(&format!("{}/mcp", mcp.base), Some(OAuthConfig::default()));

    lca_mcp::remote::remember_unauthorized(&caps, &server, &format!("{}/mcp", mcp.base))
        .expect("the 401 was captured");

    // Observe the host set from **inside the flow**, not by polling it from
    // outside (gh #447).
    //
    // The original row spawned a watcher that polled `oauth_flow_hosts()` every
    // 5 ms and returned at the first non-empty sample. On a loaded runner the
    // whole sign-in could finish before the watcher's first tick, leaving
    // `during` legitimately empty and failing a row whose product was correct:
    // the row was racing its own instrument, and a sample-based assertion cannot
    // pin "it was admitted during the flow" if the flow can be shorter than the
    // sampling period.
    //
    // The browser opener is the seam that fixes it in kind rather than by
    // widening a window: `oauth_open` invokes it *synchronously, inside*
    // `sign_in_flow`, after both `oauth_flow_attach` waves and before
    // `sign_in` clears the set. Reading the hosts here is therefore a fact
    // about the flow's own body - it cannot miss, and it cannot observe an
    // after-state. The opener also delivers the pasted callback, exactly as
    // `answer_callback` did, so the journey is unchanged.
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let opener_caps = caps.clone();
    let opener_seen = seen.clone();
    caps.set_browser_opener(Some(Arc::new(move |url: &str| {
        // Inside the flow: the authorisation URL is being opened, so whatever
        // hosts the flow admitted are admitted right now.
        *opener_seen.lock().expect("seen") = opener_caps
            .oauth_flow_hosts()
            .into_iter()
            .map(|pattern| describe(&pattern))
            .collect();
        // Deliver the callback the way a browser that cannot reach the loopback
        // would (the manual-delivery seam the other rows use).
        let field = |name: &str| {
            url.split(&format!("{name}="))
                .nth(1)
                .and_then(|rest| rest.split('&').next())
                .unwrap_or("")
        };
        let state = field("state").to_string();
        if !state.is_empty() {
            let _ = opener_caps.oauth_deliver_manual(vec![
                ("code".to_string(), "d-code".to_string()),
                ("state".to_string(), state),
            ]);
        }
        Ok(())
    })));
    lca_mcp::sign_in(caps.clone(), &server).expect("the sign-in completes");

    let during = seen.lock().expect("seen").clone();
    assert!(
        !during.is_empty(),
        "the flow admitted at least one discovered host while it ran \
         (observed from inside the flow, so this cannot be a missed sample)"
    );
    // `localhost` may surface as its resolved `[::1]` spelling: the flow's
    // grant is keyed by what the URL said, and a URL may name either. Accept
    // both forms of the same host.
    assert!(
        during
            .iter()
            .any(|host| host.contains(second_host()) || host.contains("::1")),
        "the authorization server's host was among them: {during:?}"
    );
    assert!(
        caps.oauth_flow_hosts().is_empty(),
        "the flow-scoped hosts are gone once it ends: {:?}",
        caps.oauth_flow_hosts()
    );

    // The flow ended: an ordinary request to the IdP host is denied now.
    let err = caps
        .net_request("GET", &format!("{}/anything", idp.base), &[], None)
        .expect_err("the flow-scoped host is gone");
    let text = err.to_string().to_lowercase();
    assert!(
        text.contains("denied") || text.contains("not granted") || text.contains("net"),
        "the denial names the net capability: {text}"
    );

    // And nothing about the IdP host reached the user's grant store.
    let grants_text = std::fs::read_to_string(root.join("grants.json")).unwrap_or_default();
    assert!(
        !grants_text.contains(second_host()),
        "the flow host was never persisted: {grants_text}"
    );
}

// ============================================================================
// gh #290 (finishing #418 Phase D's discovery work): the untrusted-metadata
// rules that were still missing, and the stale-challenge lifecycle.
//
// Every fixture in this file uses **loopback** hosts, which is exactly why
// these rows are the ones a green suite was missing: `http://127.0.0.2:PORT`
// is legal for a *local* IdP (RFC 8252, and how `discover`'s own origin
// fallback reaches one) while being an attack when a **remote** server's
// metadata names it. The distinction is "is the MCP server itself loopback",
// not "is the endpoint loopback".
// ============================================================================

// Verifies: gh #290 design point 6 - a **remote** server's metadata naming a
// loopback authorization server is refused. A loopback entry there is a request
// to reach the user's own machine, which `net-local` exists to gate; a sign-in
// must not grant it silently.
//
// The MCP server's own host is `127.0.0.1` here and the IdP is `127.0.0.2`:
// both loopback, so a rule phrased "refuse a loopback endpoint" would break
// every local-IdP flow. The rule is `is_loopback(mcp server url)`.
#[test]
fn a_remote_servers_metadata_may_not_name_a_loopback_authorization_server() {
    let idp = idp_host(None);
    // A remote-shaped MCP server: its URL says example.invalid (never
    // resolved - the refusal must come from validation, not a connection
    // failure), and the 401 it returns points at the loopback IdP.
    let remote = "https://mcp.example.invalid/mcp";
    let (caps, _root) = engine("loop-remote", &[]);
    let server = server_config(remote, Some(OAuthConfig::default()));
    // Seed the challenge directly: probing a non-resolving host would fail
    // before the 401, and the row is about discovery, not transport.
    TokenStore::new(caps.clone(), &server)
        .set_pending_challenge(&{
            let mut challenge = BTreeMap::new();
            challenge.insert(
                "resource_metadata".to_string(),
                format!("{}/.well-known/oauth-protected-resource", idp.base),
            );
            challenge
        })
        .expect("seed the challenge");
    let err = lca_mcp::sign_in(caps.clone(), &server)
        .expect_err("a loopback IdP named by a remote server is refused");
    assert!(
        err.contains("loopback") || err.contains("local"),
        "the refusal says what it refused: {err}"
    );
    assert!(
        idp.paths().is_empty(),
        "nothing was fetched from the loopback host: {:?}",
        idp.paths()
    );
}

// Verifies: gh #290 design point 6, the other side - a **loopback** MCP server
// may still name a loopback authorization server, because that is the local
// IdP case `discover`'s origin fallback serves and RFC 8252 blesses. This row
// exists so the refusal above cannot be "fixed" by banning loopback broadly.
#[test]
fn a_loopback_servers_metadata_may_name_a_loopback_authorization_server() {
    let idp = idp_host(None);
    let mcp = mcp_host(&idp.base);
    let (caps, _root) = engine("loop-local", &["127.0.0.1"]);
    let server = server_config(&format!("{}/mcp", mcp.base), Some(OAuthConfig::default()));
    lca_mcp::remote::remember_unauthorized(&caps, &server, &format!("{}/mcp", mcp.base))
        .expect("the 401 was captured");
    answer_callback(caps.clone());
    let outcome = lca_mcp::sign_in(caps.clone(), &server);
    assert!(
        outcome.is_ok(),
        "a local IdP is still reachable from a local server: {outcome:?}"
    );
}

// Verifies: gh #290 design point 5 - a metadata document naming a **wildcard**
// authorization server is refused. `parse_net_pattern` would happily build
// `*.example.com`, and a flow-scoped grant on it would let one sign-in reach
// every subdomain of an attacker's choosing.
#[test]
fn a_metadata_document_naming_a_wildcard_authorization_server_is_refused() {
    let idp = idp_host(Some("https://*.example.com"));
    let mcp = mcp_host(&idp.base);
    let (caps, _root) = engine("wild", &["127.0.0.1"]);
    let server = server_config(&format!("{}/mcp", mcp.base), Some(OAuthConfig::default()));
    lca_mcp::remote::remember_unauthorized(&caps, &server, &format!("{}/mcp", mcp.base))
        .expect("the 401 was captured");
    let err = lca_mcp::sign_in(caps.clone(), &server)
        .expect_err("a wildcard authorization server is refused");
    // The refusal must be the **validation**, not a later denial. Without this
    // distinction the row passed while its own guard was removed: a wildcard
    // matches no granted pattern either, so "the sign-in failed and the message
    // mentions the URL" is satisfied by the permission error the rule exists to
    // prevent. `wildcard` appears only in the validation message.
    assert!(
        err.contains("wildcard") && err.contains("*.example.com"),
        "the refusal is the wildcard rule, not a downstream denial: {err}"
    );
    assert!(
        !err.contains("permission") && !err.contains("granted pattern"),
        "no request was attempted against the wildcard: {err}"
    );
}

// Verifies: gh #290 design point 5 - the number of hosts one sign-in may admit
// is **bounded**. A metadata document is attacker-influenced input; without a
// bound it can name a thousand authorization servers and the flow would admit
// every one of them for the duration.
#[test]
fn one_sign_in_admits_at_most_the_declared_number_of_hosts() {
    // The bound is a fact of the implementation, stated here so a later change
    // has to change this number deliberately.
    assert_eq!(
        lca_mcp::oauth::MAX_FLOW_HOSTS,
        lca_tools::MAX_OAUTH_FLOW_HOSTS,
        "the documented bound and the enforcing one are one number"
    );
    assert_eq!(
        lca_mcp::oauth::MAX_FLOW_HOSTS,
        8,
        "the flow host bound is part of this contract"
    );
    let (caps, _root) = engine("bound", &[]);
    let many: Vec<String> = (0..50)
        .map(|index| format!("h{index}.example.com:443"))
        .collect();
    let err = caps
        .oauth_flow_attach(many)
        .expect_err("50 hosts exceed the bound");
    let text = err.to_string();
    assert!(text.contains("8"), "the refusal states the bound: {text}");
    assert!(
        caps.oauth_flow_hosts().is_empty(),
        "and nothing was admitted by the failed call: {:?}",
        caps.oauth_flow_hosts()
    );
}

// ============================================================================
// gh #290 - the persisted challenge needs a **lifecycle**.
//
// `remember_step_up` writes a challenge and `sign_in` reads it. The question the
// issue does not ask, and the one that turns this fix into a security finding if
// it is skipped, is: *when is it cleared?* A `resource_metadata` remembered for
// server A must never steer a sign-in for server B (a different scoped server
// sharing the process), and a challenge from an **earlier** attempt at the same
// server must not steer a later one - the server is telling you where to go
// *now*, and a stale pointer that still resolves could send the credential
// somewhere the server has since stopped naming.
// ============================================================================

/// A token-store key is `mcp-oauth:<name>:<url>`, so the pending challenge is
/// keyed by the same pair: two servers cannot share one, even in one process.
/// This row is the cheap half of the lifecycle - the store's own namespacing.
#[test]
fn two_servers_never_share_one_pending_challenge() {
    let (caps, _root) = engine("lifecycle-ns", &["127.0.0.1"]);
    let a = server_config("http://127.0.0.1:1111/mcp", Some(OAuthConfig::default()));
    let b = server_config("http://127.0.0.1:2222/mcp", Some(OAuthConfig::default()));
    let store_a = TokenStore::new(caps.clone(), &a);
    let store_b = TokenStore::new(caps.clone(), &b);
    let mut challenge = BTreeMap::new();
    challenge.insert(
        "resource_metadata".to_string(),
        "https://a.invalid/meta".to_string(),
    );
    store_a.set_pending_challenge(&challenge).expect("write");
    assert!(
        store_b.pending_challenge().expect("read").is_none(),
        "server B's store sees nothing of A's challenge"
    );
}

/// The lifecycle rule: a sign-in **consumes** the challenge it used, so the same
/// remembered pointer cannot steer a second attempt. Without this, a server that
/// stops advertising `resource_metadata` (or points it elsewhere) would keep
/// being discovered through the old pointer for as long as the store lives -
/// and the pointer is attacker-influenced, so "remember it forever" is a
/// standing instruction from whoever sent the first 401.
///
/// Consuming rather than clearing-on-success: a *failed* sign-in must not leave
/// the pointer behind either, because the next attempt would read it without the
/// server having said anything, which is precisely the "stale pointer steers a
/// later sign-in" failure.
#[test]
fn a_sign_in_consumes_the_challenge_it_used() {
    let idp = idp_host(None);
    let mcp = mcp_host(&idp.base);
    let (caps, _root) = engine("lifecycle-consume", &["127.0.0.1"]);
    let server = server_config(&format!("{}/mcp", mcp.base), Some(OAuthConfig::default()));
    let store = TokenStore::new(caps.clone(), &server);
    lca_mcp::remote::remember_unauthorized(&caps, &server, &format!("{}/mcp", mcp.base))
        .expect("the 401 was captured");
    assert!(
        store
            .pending_challenge()
            .expect("read")
            .is_some_and(|challenge| challenge.contains_key("resource_metadata")),
        "the challenge was captured"
    );

    answer_callback(caps.clone());
    lca_mcp::sign_in(caps.clone(), &server).expect("the sign-in completes");

    assert!(
        store.pending_challenge().expect("read").is_none(),
        "the challenge was consumed by the sign-in that used it"
    );
}

/// The residue rule, stated as the property that matters: after a sign-in, a
/// **second** `sign_in` for the same server must not follow the old
/// `resource_metadata`. It falls through to the configured (or origin) discovery
/// path instead - which for this fixture means the sign-in no longer reaches
/// host B at all, because nothing names it any more.
///
/// This is the assertion the consuming half exists for, and it is checked from
/// the server's side (which paths host B saw) rather than from the store's.
#[test]
fn a_previous_sign_ins_metadata_does_not_steer_the_next_one() {
    let idp = idp_host(None);
    let mcp = mcp_host(&idp.base);
    let (caps, _root) = engine("lifecycle-residue", &["127.0.0.1"]);
    let server = server_config(&format!("{}/mcp", mcp.base), Some(OAuthConfig::default()));
    lca_mcp::remote::remember_unauthorized(&caps, &server, &format!("{}/mcp", mcp.base))
        .expect("the 401 was captured");
    answer_callback(caps.clone());
    lca_mcp::sign_in(caps.clone(), &server).expect("the first sign-in completes");
    let after_first = idp.paths().len();
    assert!(after_first > 0, "host B was reached by the first sign-in");

    // A second sign-in with no fresh 401: the challenge is gone, so discovery
    // must not consult host B's protected-resource metadata again.
    answer_callback(caps.clone());
    let second = lca_mcp::sign_in(caps.clone(), &server);
    let paths = idp.paths();
    let protected: Vec<&String> = paths
        .iter()
        .filter(|path| path.contains("oauth-protected-resource"))
        .collect();
    assert_eq!(
        protected.len(),
        1,
        "the resource metadata was fetched exactly once, by the first sign-in \
         ({:?}); second outcome {second:?}",
        paths
    );
}

/// The half the success path does not cover: a **failed** sign-in must consume
/// the challenge too. `exchange` clears it after a successful token exchange, so
/// a completed sign-in is already clean; but a sign-in that fails *before* the
/// exchange (a refusal, a bad endpoint, an unreachable host) leaves the pointer
/// behind, and the next attempt reads it without the server having said
/// anything. That is the lifecycle hole the success-only clearing misses, and it
/// is the one that matters, because a refusal is exactly when a stale pointer
/// would be followed blindly.
#[test]
fn a_failed_sign_in_also_consumes_the_challenge() {
    // A loopback IdP named by a *remote* server: refused by the loopback rule,
    // so the sign-in fails before it can exchange anything.
    let idp = idp_host(None);
    let (caps, _root) = engine("lifecycle-failed", &[]);
    let server = server_config(
        "https://mcp.example.invalid/mcp",
        Some(OAuthConfig::default()),
    );
    let store = TokenStore::new(caps.clone(), &server);
    let mut challenge = BTreeMap::new();
    challenge.insert(
        "resource_metadata".to_string(),
        format!("{}/.well-known/oauth-protected-resource", idp.base),
    );
    store.set_pending_challenge(&challenge).expect("seed");
    lca_mcp::sign_in(caps.clone(), &server).expect_err("the loopback rule refuses");
    assert!(
        store.pending_challenge().expect("read").is_none(),
        "a refused sign-in leaves no pointer for the next attempt to follow"
    );
}
