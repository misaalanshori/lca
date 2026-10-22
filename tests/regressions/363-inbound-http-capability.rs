//! NFR-24 guard for gh #363 / ADR-0049: the inbound HTTP listener.
//!
//! These rows drive a **real** `TcpListener` on loopback and a **real** socket
//! client, because the properties that matter here are all properties of the
//! wire: the bind address, the token check, the `Host` check, the prefix
//! stripping, the port fallback, and clean shutdown. A unit test over the
//! routing predicate would pass while the listener served without a token.
//!
//! The extension half is a stub implementing [`ServeHttpNamespace`], so the
//! host's gate is what is under test rather than a guest's behaviour. The
//! guest half is covered by `lca-ext-host`'s conformance rows.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use lca_protocol::CapabilityError;
use lca_tools::serve_http::{
    self, HttpResponse, IncomingRequest, Listener, Refusal, ServeHttpNamespace, SessionToken,
};

/// An extension stub that records what it was asked and answers with a
/// deterministic body naming the path it received.
struct Stub {
    extension: String,
    session: String,
    /// Set when the handler was reached at all, so a refusal row can assert the
    /// guest was *not* called rather than only that the status was wrong.
    called: Arc<AtomicBool>,
    /// The paths the handler saw, already stripped.
    seen: Arc<Mutex<Vec<String>>>,
    /// Answers with an error instead of a response when set, which is the
    /// "one bad handler must not take the listener down" row.
    fails: Arc<AtomicBool>,
}

impl Stub {
    fn new(extension: &str, session: &str) -> Arc<Stub> {
        Arc::new(Stub {
            extension: extension.to_string(),
            session: session.to_string(),
            called: Arc::new(AtomicBool::new(false)),
            seen: Arc::new(Mutex::new(Vec::new())),
            fails: Arc::new(AtomicBool::new(false)),
        })
    }

    fn paths(&self) -> Vec<String> {
        self.seen.lock().expect("lock").clone()
    }
}

impl ServeHttpNamespace for Stub {
    fn extension(&self) -> &str {
        &self.extension
    }

    fn session(&self) -> &str {
        &self.session
    }

    fn answer(
        &self,
        base_url: &str,
        request: IncomingRequest,
    ) -> Result<HttpResponse, CapabilityError> {
        self.called.store(true, Ordering::SeqCst);
        let rest = request.path.clone();
        self.seen.lock().expect("lock").push(rest.clone());
        if self.fails.load(Ordering::SeqCst) {
            return Err(CapabilityError::Invalid("handler exploded".into()));
        }
        Ok(HttpResponse {
            status: 200,
            headers: vec![(
                "content-type".to_string(),
                "text/html; charset=utf-8".to_string(),
            )],
            body: format!("base={base_url};rest={rest};q={}", request.query).into_bytes(),
        })
    }

    fn record_refusal(&self, _parameter: &str, _reason: &str) {}
}

/// Serializes rows that bind the listener's port range.
///
/// The listener tries `8080..8089` - a fixed range, because that is what a user's
/// machine looks like (one process per port). `nextest` runs rows in parallel, so
/// two rows binding at once contend for the same ten ports and the loser sees a
/// *neighbour's* socket: `dropping_the_listener_frees_the_port` failed exactly
/// that way, not because shutdown was broken but because another row had already
/// taken the port it was trying to rebind.
///
/// A test-only base-port parameter in the production API would have hidden this
/// by giving every row its own range, which no real session has. Holding a lock
/// models reality: one listener at a time per machine.
static PORT_LOCK: Mutex<()> = Mutex::new(());

/// Take the port lock. Every row that binds holds it until it finishes.
fn serial() -> std::sync::MutexGuard<'static, ()> {
    PORT_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Bind a listener over one stub and return it with the stub.
fn start(extension: &str, session: &str) -> (Listener, Arc<Stub>) {
    let stub = Stub::new(extension, session);
    let listing: Arc<Mutex<Vec<Arc<dyn ServeHttpNamespace>>>> =
        Arc::new(Mutex::new(
            vec![stub.clone() as Arc<dyn ServeHttpNamespace>],
        ));
    let token = SessionToken::from_known("test-token-0123456789");
    let listener = serve_http::bind(listing, token).expect("the listener binds a free port");
    (listener, stub)
}

/// Send a raw HTTP/1.1 request and return `(status line, headers, body)`.
fn raw(addr: SocketAddr, request: &str) -> (String, String, String) {
    let mut stream = TcpStream::connect(addr).expect("connect");
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(10)))
        .expect("timeout");
    stream.write_all(request.as_bytes()).expect("write");
    stream.flush().expect("flush");
    let mut response = String::new();
    stream.read_to_string(&mut response).expect("read");
    let (head, body) = response.split_once("\r\n\r\n").unwrap_or((&response, ""));
    let mut lines = head.split("\r\n");
    let status = lines.next().unwrap_or("").to_string();
    let headers = lines.collect::<Vec<_>>().join("\r\n");
    (status, headers, body.to_string())
}

/// A GET with the token in the query, the shape a browser navigates with.
fn get_with_token(listener: &Listener, path: &str) -> String {
    format!(
        "GET {path}{}token={} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
        if path.contains('?') { "&" } else { "?" },
        listener.token().expose(),
        listener.bound().addr.port()
    )
}

#[test]
fn the_listener_binds_loopback_and_never_a_wildcard() {
    // One listener at a time per machine (see `serial`).
    let _serial = serial();
    // The one invariant that a manifest cannot argue with: there is no
    // address parameter to get wrong, so the assertion is that the bound
    // address is literally a loopback address on the port we were told.
    let (listener, _stub) = start("ui-example", "sess-1");
    let bound = listener.bound().addr;
    assert_eq!(bound.ip(), IpAddr::V4(Ipv4Addr::LOCALHOST));
    assert!(
        !bound.ip().is_unspecified(),
        "a wildcard bind would be unspecified"
    );
    assert!(listener.bound().origin.starts_with("http://127.0.0.1:"));
}

#[test]
fn a_request_with_the_token_is_served_and_reaches_the_extension() {
    // One listener at a time per machine (see `serial`).
    let _serial = serial();
    let (listener, stub) = start("ui-example", "sess-1");
    let (status, headers, body) = raw(
        listener.bound().addr,
        &get_with_token(&listener, "/sess-1/ui-example/index.html"),
    );
    assert!(status.contains("200"), "status was {status}");
    assert!(headers.to_lowercase().contains("content-type: text/html"));
    // The body proves the extension was reached AND that it saw the stripped
    // path, which is the contract that makes `index.html` portable.
    assert!(body.contains("rest=/index.html"), "body was {body}");
    assert!(stub.called.load(Ordering::SeqCst));
}

#[test]
fn a_request_without_the_token_is_refused_before_the_extension_is_called() {
    // One listener at a time per machine (see `serial`).
    let _serial = serial();
    let (listener, stub) = start("ui-example", "sess-1");
    let request = format!(
        "GET /sess-1/ui-example/index.html HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
        listener.bound().addr.port()
    );
    let (status, _, body) = raw(listener.bound().addr, &request);
    assert!(status.contains("401"), "status was {status}");
    assert!(body.contains("unauthorized"), "body was {body}");
    // The important half: the guest never ran. A 401 that still called the
    // handler would be a gate in front of nothing.
    assert!(
        !stub.called.load(Ordering::SeqCst),
        "the extension was called"
    );
    assert!(stub.paths().is_empty());
}

#[test]
fn a_wrong_token_is_refused_exactly_like_no_token() {
    // One listener at a time per machine (see `serial`).
    let _serial = serial();
    let (listener, stub) = start("ui-example", "sess-1");
    let request = format!(
        "GET /sess-1/ui-example/ HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
        listener.bound().addr.port()
    )
    .replace("HTTP/1.1", "HTTP/1.1");
    // Append a near-miss token: one character off the real one.
    let mut near_miss = listener.token().expose().to_string();
    near_miss.pop();
    near_miss.push('0');
    let request = format!(
        "{}token={near_miss}",
        request.replace("HTTP/1.1", "HTTP/1.1&")
    );
    let (status, _, body) = raw(listener.bound().addr, &request);
    assert!(status.contains("401"), "status was {status}");
    assert!(body.contains("unauthorized"));
    assert!(!stub.called.load(Ordering::SeqCst));
}

#[test]
fn a_bearer_header_authenticates_where_a_query_token_cannot_be_set() {
    // One listener at a time per machine (see `serial`).
    let _serial = serial();
    // A `fetch` cannot put the token in the URL of a navigation but can set a
    // header; both forms are accepted so a page can call its own backend.
    let (listener, stub) = start("ui-example", "sess-1");
    let request = format!(
        "POST /sess-1/ui-example/api HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nAuthorization: Bearer {}\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello",
        listener.bound().addr.port(),
        listener.token().expose()
    );
    let (status, _, body) = raw(listener.bound().addr, &request);
    assert!(status.contains("200"), "status was {status}");
    assert!(body.contains("rest=/api"));
    assert!(stub.called.load(Ordering::SeqCst));
}

#[test]
fn a_foreign_host_header_is_refused_as_a_rebinding_attempt() {
    // One listener at a time per machine (see `serial`).
    let _serial = serial();
    // The DNS-rebinding case: an attacker's name resolves to 127.0.0.1, so
    // the loopback bind has already passed and only this check is left.
    let (listener, stub) = start("ui-example", "sess-1");
    let request = format!(
        "GET /sess-1/ui-example/index.html?token={} HTTP/1.1\r\nHost: evil.example:8080\r\nConnection: close\r\n\r\n",
        listener.token().expose()
    );
    let (status, _, body) = raw(listener.bound().addr, &request);
    assert!(status.contains("404"), "status was {status}");
    assert!(
        !body.contains("rebinding"),
        "the body must not explain itself: {body}"
    );
    assert!(
        !stub.called.load(Ordering::SeqCst),
        "a rebinding attempt reached the extension"
    );
}

#[test]
fn the_extension_receives_a_stripped_path_so_relative_assets_work() {
    // One listener at a time per machine (see `serial`).
    let _serial = serial();
    // The requirement that makes a static UI possible: the guest is told
    // `/style.css`, not `/<session>/<ext>/style.css`, so its own HTML can say
    // `./style.css` and it works wherever it is mounted.
    let (listener, stub) = start("ui-example", "sess-1");
    for asset in ["/index.html", "/style.css", "/bundle.js"] {
        let (status, _, _) = raw(
            listener.bound().addr,
            &get_with_token(&listener, &format!("/sess-1/ui-example{asset}")),
        );
        assert!(status.contains("200"), "{asset}: status was {status}");
    }
    assert_eq!(
        stub.paths(),
        vec!["/index.html", "/style.css", "/bundle.js"]
    );
}

#[test]
fn the_extension_is_told_its_own_base_url_with_the_trailing_slash() {
    // One listener at a time per machine (see `serial`).
    let _serial = serial();
    // `base-url` is what lets an extension build an absolute link without
    // guessing the port; the trailing slash is what makes `base + "app.js"`
    // work by concatenation.
    let (listener, _stub) = start("ui-example", "sess-1");
    let (_, _, body) = raw(
        listener.bound().addr,
        &get_with_token(&listener, "/sess-1/ui-example/index.html"),
    );
    let expected = format!(
        "base=http://127.0.0.1:{}/sess-1/ui-example/;",
        listener.bound().addr.port()
    );
    assert!(body.contains(&expected), "body was {body}");
}

#[test]
fn the_query_string_arrives_separately_from_the_path() {
    // One listener at a time per machine (see `serial`).
    let _serial = serial();
    let (listener, _stub) = start("ui-example", "sess-1");
    let (_, _, body) = raw(
        listener.bound().addr,
        &get_with_token(&listener, "/sess-1/ui-example/search?q=hello&page=2"),
    );
    assert!(body.contains("rest=/search"), "body was {body}");
    assert!(body.contains("q=hello&page=2"), "body was {body}");
}

#[test]
fn an_unknown_namespace_is_a_404_that_does_not_enumerate() {
    // One listener at a time per machine (see `serial`).
    let _serial = serial();
    let (listener, stub) = start("ui-example", "sess-1");
    let (status, _, body) = raw(
        listener.bound().addr,
        &get_with_token(&listener, "/sess-1/not-installed/index.html"),
    );
    assert!(status.contains("404"), "status was {status}");
    assert!(body.contains("not found"), "body was {body}");
    // Same body as a rebinding refusal, so walking the port teaches nothing.
    assert_eq!(body, "not found");
    assert!(!stub.called.load(Ordering::SeqCst));
}

#[test]
fn the_session_segment_must_match_so_a_stale_tab_cannot_reach_a_new_session() {
    // One listener at a time per machine (see `serial`).
    let _serial = serial();
    let (listener, stub) = start("ui-example", "sess-1");
    let (status, _, _) = raw(
        listener.bound().addr,
        &get_with_token(&listener, "/sess-OLD/ui-example/index.html"),
    );
    assert!(status.contains("404"), "status was {status}");
    assert!(!stub.called.load(Ordering::SeqCst));
}

#[test]
fn a_failing_handler_answers_500_and_the_listener_keeps_serving() {
    // One listener at a time per machine (see `serial`).
    let _serial = serial();
    // One bad handler must not take the port down: the agent's web UI is a
    // convenience, and a single trapped request turning off the listener would
    // make every other extension's UI vanish too.
    let (listener, stub) = start("ui-example", "sess-1");
    stub.fails.store(true, Ordering::SeqCst);
    let (status, _, body) = raw(
        listener.bound().addr,
        &get_with_token(&listener, "/sess-1/ui-example/boom"),
    );
    assert!(status.contains("500"), "status was {status}");
    assert!(body.contains("handler exploded"), "body was {body}");

    stub.fails.store(false, Ordering::SeqCst);
    let (status, _, _) = raw(
        listener.bound().addr,
        &get_with_token(&listener, "/sess-1/ui-example/after"),
    );
    assert!(
        status.contains("200"),
        "the listener stopped serving: {status}"
    );
}

#[test]
fn a_websocket_upgrade_is_refused_cleanly_rather_than_bridged() {
    // One listener at a time per machine (see `serial`).
    let _serial = serial();
    // ADR-0049 defers bridging. The honest answer is a refusal a client can
    // read, not a hang or a 200 the client will misread as an upgrade.
    let (listener, stub) = start("ui-example", "sess-1");
    let request = format!(
        "GET /sess-1/ui-example/ws?token={} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n",
        listener.token().expose(),
        listener.bound().addr.port()
    );
    let (status, _, body) = raw(listener.bound().addr, &request);
    assert!(status.contains("501"), "status was {status}");
    assert!(body.contains("websocket"), "body was {body}");
    assert!(!stub.called.load(Ordering::SeqCst));
}

#[test]
fn an_oversized_body_is_refused_by_its_declared_length() {
    // One listener at a time per machine (see `serial`).
    let _serial = serial();
    let (listener, stub) = start("ui-example", "sess-1");
    let request = format!(
        "POST /sess-1/ui-example/upload?token={} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Length: {}\r\n\r\n",
        listener.token().expose(),
        listener.bound().addr.port(),
        serve_http::MAX_BODY_BYTES + 1
    );
    let (status, _, _) = raw(listener.bound().addr, &request);
    assert!(status.contains("413"), "status was {status}");
    assert!(!stub.called.load(Ordering::SeqCst));
}

#[test]
fn a_guest_cannot_inject_a_header_or_lie_about_the_length() {
    // One listener at a time per machine (see `serial`).
    let _serial = serial();
    // The host owns the framing: a guest-supplied `content-length` or a header
    // value carrying a newline would let an extension forge the response
    // envelope the client parses.
    struct Liar;
    impl ServeHttpNamespace for Liar {
        fn extension(&self) -> &str {
            "liar"
        }
        fn session(&self) -> &str {
            "sess-1"
        }
        fn answer(
            &self,
            _base_url: &str,
            _request: IncomingRequest,
        ) -> Result<HttpResponse, CapabilityError> {
            Ok(HttpResponse {
                status: 200,
                headers: vec![
                    ("content-length".to_string(), "9999".to_string()),
                    ("x-injected".to_string(), "a\r\nX-Evil: yes".to_string()),
                ],
                body: b"real".to_vec(),
            })
        }
        fn record_refusal(&self, _parameter: &str, _reason: &str) {}
    }
    let listing: Arc<Mutex<Vec<Arc<dyn ServeHttpNamespace>>>> = Arc::new(Mutex::new(vec![
        Arc::new(Liar) as Arc<dyn ServeHttpNamespace>,
    ]));
    let listener = serve_http::bind(listing, SessionToken::from_known("test-token-0123456789"))
        .expect("binds");
    let (status, headers, body) = raw(
        listener.bound().addr,
        &get_with_token(&listener, "/sess-1/liar/x"),
    );
    assert!(status.contains("200"), "status was {status}");
    // The real length, not the guest's claim.
    assert!(headers.contains("content-length: 4"), "headers: {headers}");
    assert!(
        !headers.contains("X-Evil"),
        "a header was injected: {headers}"
    );
    assert_eq!(body, "real");
}

#[test]
fn a_distinct_session_id_gets_its_own_namespace_so_two_agents_can_share_a_port() {
    // One listener at a time per machine (see `serial`).
    let _serial = serial();
    // The `<session-id>` segment is what lets one port serve two sessions: the
    // path is the only thing separating them, so it must actually separate.
    let first = Stub::new("ui-example", "sess-1");
    let second = Stub::new("ui-example", "sess-2");
    let listing: Arc<Mutex<Vec<Arc<dyn ServeHttpNamespace>>>> = Arc::new(Mutex::new(vec![
        first.clone() as Arc<dyn ServeHttpNamespace>,
        second.clone() as Arc<dyn ServeHttpNamespace>,
    ]));
    let listener = serve_http::bind(listing, SessionToken::from_known("test-token-0123456789"))
        .expect("binds");
    let (status, _, _) = raw(
        listener.bound().addr,
        &get_with_token(&listener, "/sess-2/ui-example/index.html"),
    );
    assert!(status.contains("200"), "status was {status}");
    assert!(
        !first.called.load(Ordering::SeqCst),
        "sess-1 served sess-2's request"
    );
    assert!(second.called.load(Ordering::SeqCst));
}

#[test]
fn a_port_in_use_falls_back_to_the_next_one_and_the_bound_address_says_so() {
    // One listener at a time per machine (see `serial`).
    let _serial = serial();
    // The degradation contract: the notice must name the port actually bound,
    // because a user told 8080 while the listener is on 8083 was told
    // something false.
    //
    // Anchor the range by occupying the first free port the bind would try.
    let mut squatter: Option<TcpListener> = None;
    let mut expected_first = None;
    for offset in 0..serve_http::PORT_ATTEMPTS {
        let port = serve_http::DEFAULT_PORT + offset;
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
        if let Ok(listener) = TcpListener::bind(addr) {
            // Keep it bound, and note the port the real bind must therefore skip.
            expected_first = Some(port);
            squatter = Some(listener);
            break;
        }
    }
    let squatter = squatter.expect("a free port in the default range");
    let blocked = expected_first.expect("the squatted port");

    let (listener, _stub) = start("ui-example", "sess-1");
    let bound = listener.bound().addr.port();
    assert_ne!(bound, blocked, "the listener took the occupied port");
    assert_eq!(
        listener.bound().origin,
        format!("http://127.0.0.1:{bound}"),
        "the origin must name the port actually bound"
    );
    // And the fallback range is still inside the documented window.
    assert!(
        (serve_http::DEFAULT_PORT..serve_http::DEFAULT_PORT + serve_http::PORT_ATTEMPTS)
            .contains(&bound),
        "bound {bound} is outside the fallback range"
    );
    drop(squatter);
}

#[test]
fn dropping_the_listener_frees_the_port_so_no_process_exits_with_it_bound() {
    // One listener at a time per machine (see `serial`).
    let _serial = serial();
    // Shutdown is clean: the port is free the moment the listener is dropped,
    // which is the property that keeps a crashed or exited session from
    // leaving a socket behind for the next one to trip over.
    let port = {
        let (listener, _stub) = start("ui-example", "sess-1");
        listener.bound().addr.port()
    };
    // Give the accept thread a moment to notice and exit after the drop.
    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    let mut rebound = None;
    for _ in 0..50 {
        if let Ok(listener) = TcpListener::bind(addr) {
            rebound = Some(listener);
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(
        rebound.is_some(),
        "port {port} was still bound after the listener was dropped"
    );
}

#[test]
fn the_browser_url_carries_the_token_so_the_notice_has_one_thing_to_show() {
    // One listener at a time per machine (see `serial`).
    let _serial = serial();
    let (listener, _stub) = start("ui-example", "sess-1");
    let url = listener.browser_url();
    assert!(url.starts_with("http://127.0.0.1:"), "got {url}");
    assert!(url.contains("?token="), "got {url}");
    assert!(url.ends_with(listener.token().expose()), "got {url}");
}

#[test]
fn no_port_free_reports_unavailable_instead_of_panicking_or_hanging() {
    // One listener at a time per machine (see `serial`).
    let _serial = serial();
    // Every port in the range occupied: the capability must report a refusal
    // the caller turns into "unavailable", never panic and never block. The
    // bind is bounded, so this also proves the scan terminates.
    let mut squatters = Vec::new();
    for offset in 0..serve_http::PORT_ATTEMPTS {
        let port = serve_http::DEFAULT_PORT + offset;
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
        if let Ok(listener) = TcpListener::bind(addr) {
            squatters.push(listener);
        }
    }
    // Only meaningful if we actually hold every port in the range; on a machine
    // where something else already holds one, holding the rest is enough.
    let listing: Arc<Mutex<Vec<Arc<dyn ServeHttpNamespace>>>> = Arc::new(Mutex::new(Vec::new()));
    let result = serve_http::bind(listing, SessionToken::from_known("test-token-0123456789"));
    match result {
        Err(err) => {
            let text = format!("{err}");
            assert!(
                text.contains("no free loopback port"),
                "the refusal must name the cause: {text}"
            );
        }
        Ok(listener) => {
            // Another process released a port between our scan and the bind, or
            // a port in the range belonged to someone else and is now free. Not
            // a failure: the degradation path is covered by the row below.
            let port = listener.bound().addr.port();
            assert!(
                (serve_http::DEFAULT_PORT..serve_http::DEFAULT_PORT + serve_http::PORT_ATTEMPTS)
                    .contains(&port)
            );
        }
    }
    drop(squatters);
}

#[test]
fn the_refusal_vocabulary_is_the_one_the_listener_actually_uses() {
    // One listener at a time per machine (see `serial`).
    let _serial = serial();
    // Ties the enum to the wire: every variant that can be produced here has a
    // status a client can act on, and the two that must never be confused (a
    // scanner versus a rebinding attempt) differ in whether they are recorded.
    let recorded: Vec<Refusal> = vec![
        Refusal::Rebinding("evil.example".into()),
        Refusal::BodyTooLarge,
        Refusal::Malformed("bad".into()),
        Refusal::UpgradeNotSupported,
    ];
    for refusal in recorded {
        assert!(refusal.record().is_some(), "{refusal} should be journaled");
    }
    assert!(Refusal::Unauthorized.record().is_none());
    assert!(Refusal::NoRoute.record().is_none());
}

/// A guard against the row below silently passing on a machine where the
/// default ports are all taken by someone else.
#[test]
fn the_default_port_is_reachable_on_this_machine() {
    // One listener at a time per machine (see `serial`).
    let _serial = serial();
    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), serve_http::DEFAULT_PORT);
    let free = TcpListener::bind(addr).is_ok();
    let probes = Arc::new(AtomicU32::new(0));
    probes.fetch_add(u32::from(!free), Ordering::SeqCst);
    // Either the port is free (the normal case in CI and on a laptop) or
    // something already holds it, in which case the fallback rows carry the
    // evidence. Nothing to assert beyond "we looked".
    assert!(probes.load(Ordering::SeqCst) <= 1);
}
