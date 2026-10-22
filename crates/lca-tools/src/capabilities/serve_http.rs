//! The inbound HTTP listener (gh #363, ADR-0049): the host owns the socket,
//! routes by path, checks the token, then asks one extension's handler.
//!
//! Split from `capabilities/mod.rs` for the workspace file ceiling (gate 11),
//! and because this is the one part of the capability engine that accepts
//! input from outside the process. Three properties are enforced here rather
//! than anywhere else, and each has a reason worth stating:
//!
//! - **Loopback only.** The bind address is a constant. There is no
//!   parameter, because a capability that can be told to bind a routable
//!   address is a different capability with a different consent.
//! - **The token is mandatory and checked before routing.** A request
//!   without it never reaches a namespace, so an unauthenticated caller
//!   cannot even learn which extensions are mounted on the port.
//! - **The `Host` header must be the listener's own.** This is the
//!   DNS-rebinding case: an attacker-controlled name resolving to
//!   `127.0.0.1` passes a loopback-only bind and reaches this code with a
//!   foreign `Host`. Refused, and recorded *as a rebinding attempt* rather
//!   than an ordinary denial, because it means something different.
//!
//! The token is a secret. It is generated per process with the platform's
//! CSPRNG, held in this struct, compared in constant time, and never written
//! to a log, a session record, or `ext info`. `Debug` is implemented by hand
//! so an accidental `{:?}` cannot print it either.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use lca_protocol::CapabilityError;

use super::Capabilities;

/// The port the listener tries first (the issue's default).
pub const DEFAULT_PORT: u16 = 8080;

/// How many ports above [`DEFAULT_PORT`] are tried before the capability
/// reports unavailable. Nine attempts is enough for a handful of concurrent
/// sessions and small enough that a machine with a busy range is not held up.
pub const PORT_ATTEMPTS: u16 = 10;

/// A request body larger than this is refused with 413 rather than buffered.
/// A browser UI posts forms, not files; `net`'s outbound path has its own
/// limits, and an inbound body has to be bounded somewhere because the whole
/// thing is held in memory before the guest sees it.
pub const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;

/// The token length in bytes. 32 bytes of CSPRNG output rendered as hex is
/// 64 characters: short enough to paste, far past guessing.
const TOKEN_BYTES: usize = 32;

/// The contract types live in `lca-protocol` so the listener, the guest call,
/// and the dispatch trait share one definition (see that module's docs).
pub use lca_protocol::http::{HttpRequest as IncomingRequest, HttpResponse, ServeHttpNamespace};

/// What a parsed request asks for after routing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Routed {
    /// The session id segment.
    pub session: String,
    /// The extension name segment.
    pub extension: String,
    /// The remainder, always starting with `/`.
    pub rest: String,
}

/// Why a request was turned away before any extension saw it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// No token, or the wrong one.
    Unauthorized,
    /// A `Host` header naming something other than this listener: the
    /// DNS-rebinding shape, recorded distinctly (see the module docs).
    Rebinding(String),
    /// No session segment, or no extension segment.
    NoRoute,
    /// The path named a session or extension the listener does not serve.
    UnknownNamespace(String),
    /// The request body exceeded [`MAX_BODY_BYTES`].
    BodyTooLarge,
    /// The request line or headers were malformed.
    Malformed(String),
    /// The client asked to upgrade (WebSocket), which this phase refuses
    /// rather than bridging (ADR-0049's deferral, stated cleanly).
    UpgradeNotSupported,
}

impl Refusal {
    /// The status code the client sees.
    #[must_use]
    pub fn status(&self) -> u16 {
        match self {
            // 401 rather than 403: the caller may authenticate and retry, and
            // the token's absence is what is wrong. No `WWW-Authenticate`
            // challenge is sent, because there is no scheme to advertise -
            // the token is a shared secret, not a browser auth scheme.
            Refusal::Unauthorized => 401,
            Refusal::Rebinding(_) | Refusal::UnknownNamespace(_) => 404,
            Refusal::NoRoute => 404,
            Refusal::BodyTooLarge => 413,
            Refusal::Malformed(_) => 400,
            Refusal::UpgradeNotSupported => 501,
        }
    }

    /// The one-line body. Deliberately terse and identical for every
    /// unauthorised case, so the response cannot be used to probe which
    /// namespaces exist.
    #[must_use]
    pub fn body(&self) -> &'static str {
        match self {
            Refusal::Unauthorized => "unauthorized: this listener needs its session token",
            Refusal::Rebinding(_) => "not found",
            Refusal::NoRoute => "not found: expected /<session-id>/<extension-name>/",
            Refusal::UnknownNamespace(_) => "not found",
            Refusal::BodyTooLarge => "request body too large",
            Refusal::Malformed(_) => "bad request",
            Refusal::UpgradeNotSupported => "websocket upgrades are not bridged yet",
        }
    }

    /// What to record when this refusal happens. `None` means nothing
    /// worth recording: a request with no token and no route is a scanner,
    /// and journaling every one of those would fill the denial log with
    /// noise a user cannot act on.
    #[must_use]
    pub fn record(&self) -> Option<(&'static str, String)> {
        match self {
            Refusal::Unauthorized => None,
            Refusal::Rebinding(host) => Some(("serve-http", format!("host: {host}"))),
            Refusal::NoRoute | Refusal::UnknownNamespace(_) => None,
            Refusal::BodyTooLarge => Some(("serve-http", "body".to_string())),
            Refusal::Malformed(reason) => Some(("serve-http", format!("request: {reason}"))),
            Refusal::UpgradeNotSupported => Some(("serve-http", "upgrade: websocket".to_string())),
        }
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::Unauthorized => write!(f, "no valid token"),
            Refusal::Rebinding(host) => write!(f, "rebinding attempt via Host: {host}"),
            Refusal::NoRoute => write!(f, "path names no session and extension"),
            Refusal::UnknownNamespace(what) => write!(f, "unknown namespace {what}"),
            Refusal::BodyTooLarge => write!(f, "request body over {MAX_BODY_BYTES} bytes"),
            Refusal::Malformed(reason) => write!(f, "malformed request: {reason}"),
            Refusal::UpgradeNotSupported => write!(f, "websocket upgrade refused"),
        }
    }
}

/// The per-process auth token.
///
/// Equality is constant-time: `==` on a `String` short-circuits at the first
/// differing byte, which leaks the length of the matching prefix to anyone
/// who can time the responses. That is a real attack against a local port
/// (the attacker controls both ends and can average), and it costs one
/// fold to close.
#[derive(Clone)]
pub struct SessionToken(String);

impl SessionToken {
    /// Mint a fresh token from the platform CSPRNG.
    ///
    /// A random source that fails is a startup-fatal condition for this
    /// capability: a predictable token is worse than no listener, so the
    /// caller degrades the capability to unavailable rather than falling
    /// back to a counter or a timestamp.
    pub fn generate() -> Result<SessionToken, CapabilityError> {
        let mut bytes = [0u8; TOKEN_BYTES];
        getrandom::fill(&mut bytes).map_err(|err| {
            CapabilityError::Io(format!("no random source for the session token: {err}"))
        })?;
        let mut hex = String::with_capacity(TOKEN_BYTES * 2);
        for byte in bytes {
            hex.push_str(&format!("{byte:02x}"));
        }
        Ok(SessionToken(hex))
    }

    /// Build a token from a known string (tests only: a real one is minted by
    /// [`SessionToken::generate`] from the CSPRNG).
    #[must_use]
    pub fn from_known(value: impl Into<String>) -> SessionToken {
        SessionToken(value.into())
    }

    /// The token's text, for the one place it is displayed (the session
    /// notice). Never log the result of this.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// Constant-time comparison.
    #[must_use]
    pub fn matches(&self, candidate: &str) -> bool {
        let a = self.0.as_bytes();
        let b = candidate.as_bytes();
        // The length is not secret (it is a fixed 64 hex characters for a
        // generated token), but folding over the longer slice keeps the
        // loop count independent of where the first difference is.
        let mut diff = (a.len() ^ b.len()) as u8;
        let len = a.len().max(b.len());
        for index in 0..len {
            let left = a.get(index).copied().unwrap_or(0);
            let right = b.get(index).copied().unwrap_or(0);
            diff |= left ^ right;
        }
        diff == 0
    }
}

/// Hand-written so a stray `{:?}` cannot print the token.
impl std::fmt::Debug for SessionToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SessionToken(<redacted>)")
    }
}

/// Where the listener ended up, and how to reach it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bound {
    /// The address actually bound (always loopback).
    pub addr: SocketAddr,
    /// `http://127.0.0.1:<port>` — the origin, without a trailing slash.
    pub origin: String,
}

/// A running listener. Dropping it stops the accept loop and frees the port,
/// which is the shutdown contract (`serve-http` stops with the session).
pub struct Listener {
    bound: Bound,
    token: SessionToken,
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl std::fmt::Debug for Listener {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `bound` only: the token is redacted by its own impl, and the
        // handle is not interesting.
        f.debug_struct("Listener")
            .field("bound", &self.bound)
            .finish()
    }
}

impl Listener {
    /// The address actually bound. The notice renders from this, never from
    /// the requested port: a user told "8080" while the listener is on 8083
    /// has been told something false.
    #[must_use]
    pub fn bound(&self) -> &Bound {
        &self.bound
    }

    /// The token, for the notice.
    #[must_use]
    pub fn token(&self) -> &SessionToken {
        &self.token
    }

    /// The URL a user should open: the origin plus the token, in the shape a
    /// browser can navigate to.
    #[must_use]
    pub fn browser_url(&self) -> String {
        format!("{}/?token={}", self.bound.origin, self.token.expose())
    }

    /// Stop the accept loop and wait for the thread. Also runs on drop, so a
    /// caller that forgets still leaves no bound port.
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Unblock `accept`: it is blocking with no timeout, so one loopback
        // connection of our own is what wakes it. Best effort - if the
        // connect fails the loop notices the flag at its next iteration.
        let _ = TcpStream::connect_timeout(&self.bound.addr, std::time::Duration::from_millis(50));
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Bind the listener and start the accept loop.
///
/// Tries [`DEFAULT_PORT`], then [`PORT_ATTEMPTS`]-1 ports above it. Every
/// attempt binds `127.0.0.1` specifically (`Ipv4Addr::LOCALHOST`), never a
/// wildcard, so a failed attempt cannot accidentally expose anything.
///
/// Returns `Err` when no port in the range is free, which the caller turns
/// into "the capability reports unavailable and the session continues": a web
/// UI is a convenience, and the agent must not fail to start because of a port
/// collision (ADR-0049's decision 5).
pub fn bind(
    namespaces: Arc<Mutex<Vec<Arc<dyn ServeHttpNamespace>>>>,
    token: SessionToken,
) -> Result<Listener, CapabilityError> {
    let mut last_err = String::new();
    let mut chosen: Option<TcpListener> = None;
    for offset in 0..PORT_ATTEMPTS {
        let port = DEFAULT_PORT.saturating_add(offset);
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
        match TcpListener::bind(addr) {
            Ok(listener) => {
                chosen = Some(listener);
                break;
            }
            Err(err) => last_err = format!("{addr}: {err}"),
        }
    }
    let Some(listener) = chosen else {
        return Err(CapabilityError::Io(format!(
            "no free loopback port in {}..{} ({last_err})",
            DEFAULT_PORT,
            DEFAULT_PORT.saturating_add(PORT_ATTEMPTS - 1)
        )));
    };
    let addr = listener
        .local_addr()
        .map_err(|err| CapabilityError::Io(err.to_string()))?;
    let bound = Bound {
        addr,
        origin: format!("http://127.0.0.1:{}", addr.port()),
    };
    let stop = Arc::new(AtomicBool::new(false));
    let stop_for_thread = stop.clone();
    let token_for_thread = token.clone();
    let origin = bound.origin.clone();
    let handle = std::thread::Builder::new()
        .name("lca-serve-http".to_string())
        .spawn(move || {
            accept_loop(
                listener,
                stop_for_thread,
                token_for_thread,
                namespaces,
                origin,
            );
        })
        .map_err(|err| CapabilityError::Io(format!("cannot start the listener: {err}")))?;
    Ok(Listener {
        bound,
        token,
        stop,
        handle: Some(handle),
    })
}

fn accept_loop(
    listener: TcpListener,
    stop: Arc<AtomicBool>,
    token: SessionToken,
    namespaces: Arc<Mutex<Vec<Arc<dyn ServeHttpNamespace>>>>,
    origin: String,
) {
    for stream in listener.incoming() {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        let Ok(stream) = stream else { continue };
        let token = token.clone();
        let namespaces = namespaces.clone();
        let origin = origin.clone();
        // One thread per connection, unpooled: a browser opens a handful of
        // connections and each is short-lived. A connection that outlives its
        // read timeouts is closed by the handler, so this cannot grow without
        // bound in practice.
        // `shortcut: one thread per connection, revisit if a real consumer
        // needs concurrency past a browser's connection fan-out.`
        let _ = std::thread::Builder::new()
            .name("lca-serve-http-conn".to_string())
            .spawn(move || handle_connection(stream, &token, &namespaces, &origin));
    }
}

fn handle_connection(
    mut stream: TcpStream,
    token: &SessionToken,
    namespaces: &Arc<Mutex<Vec<Arc<dyn ServeHttpNamespace>>>>,
    origin: &str,
) {
    // A client that connects and then says nothing must not pin this thread
    // for ever. Both timeouts are small: a local browser is not slow.
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
    let _ = stream.set_write_timeout(Some(std::time::Duration::from_secs(5)));
    let response = match read_request(&mut stream, origin) {
        Ok(request) => {
            let authority = authority_of(origin);
            route_and_answer(request, token, namespaces, &authority)
        }
        Err(refusal) => refuse(refusal),
    };
    write_response(&mut stream, &response);
}

/// Split `http://127.0.0.1:8080` into `127.0.0.1:8080` for `Host` comparison.
fn authority_of(origin: &str) -> String {
    origin.strip_prefix("http://").unwrap_or(origin).to_string()
}

/// Read one request: request line, headers, then a `Content-Length`-bounded
/// body. Chunked bodies are refused (`411`-adjacent: this listener answers a
/// browser and a `curl`, both of which send a length).
fn read_request(stream: &mut TcpStream, origin: &str) -> Result<IncomingRequest, Refusal> {
    let mut reader = BufReader::new(
        stream
            .try_clone()
            .map_err(|err| Refusal::Malformed(format!("cannot read the connection: {err}")))?,
    );
    let mut line = String::new();
    if reader
        .read_line(&mut line)
        .map_err(|err| Refusal::Malformed(err.to_string()))?
        == 0
    {
        return Err(Refusal::Malformed("empty request".into()));
    }
    let mut parts = line.split_whitespace();
    let method = parts
        .next()
        .ok_or_else(|| Refusal::Malformed("no method".into()))?
        .to_ascii_uppercase();
    let target = parts
        .next()
        .ok_or_else(|| Refusal::Malformed("no request target".into()))?;
    // HTTP/1.1 requires a version token; accepting anything is harmless here
    // because the framing below is HTTP/1.0-compatible anyway.
    let (path, query) = match target.split_once('?') {
        Some((path, query)) => (path.to_string(), query.to_string()),
        None => (target.to_string(), String::new()),
    };

    let mut headers: Vec<(String, String)> = Vec::new();
    loop {
        let mut header = String::new();
        let read = reader
            .read_line(&mut header)
            .map_err(|err| Refusal::Malformed(err.to_string()))?;
        if read == 0 {
            break;
        }
        let trimmed = header.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some((name, value)) = trimmed.split_once(':') {
            headers.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
        }
        if headers.len() > 128 {
            return Err(Refusal::Malformed("too many headers".into()));
        }
    }

    // Refuse an upgrade before reading a body: a WebSocket handshake has none,
    // and `501` is the honest answer for a phase that does not bridge.
    if headers
        .iter()
        .any(|(name, value)| name == "upgrade" && value.eq_ignore_ascii_case("websocket"))
    {
        return Err(Refusal::UpgradeNotSupported);
    }
    if headers.iter().any(|(name, _)| name == "transfer-encoding") {
        return Err(Refusal::Malformed(
            "chunked request bodies are not accepted".into(),
        ));
    }

    let length = headers
        .iter()
        .find(|(name, _)| name == "content-length")
        .and_then(|(_, value)| value.parse::<usize>().ok())
        .unwrap_or(0);
    if length > MAX_BODY_BYTES {
        return Err(Refusal::BodyTooLarge);
    }
    let mut body = vec![0u8; length];
    if length > 0 {
        reader
            .read_exact(&mut body)
            .map_err(|err| Refusal::Malformed(format!("short body: {err}")))?;
    }

    // `origin` is passed for symmetry with the `Host` check below, which the
    // caller performs; keeping the parameter here documents that a request
    // cannot influence it.
    let _ = origin;

    Ok(IncomingRequest {
        method,
        path,
        query,
        headers,
        body,
    })
}

/// The auth and routing gate, then the extension's own answer.
fn route_and_answer(
    request: IncomingRequest,
    token: &SessionToken,
    namespaces: &Arc<Mutex<Vec<Arc<dyn ServeHttpNamespace>>>>,
    authority: &str,
) -> HttpResponse {
    // (1) `Host`. Checked first, before the token, because a rebinding attempt
    // is the case worth journaling even when it also lacks a token - and
    // refusing it as "rebinding" rather than "unauthorized" is what makes the
    // record meaningful.
    match request.header("host") {
        Some(host) if !host_matches(host, authority) => {
            let refusal = Refusal::Rebinding(host.to_string());
            if let Some((capability, parameter)) = refusal.record() {
                record_on_all(namespaces, capability, &parameter, &refusal.to_string());
            }
            return refuse(refusal);
        }
        // A request with no `Host` at all is HTTP/1.0-shaped. Allowed: it is
        // not a rebinding shape (a browser always sends one), and refusing it
        // would break a plain `curl --http1.0`.
        _ => {}
    }

    // (2) The token: query parameter or bearer header, checked before the path
    // is parsed for a namespace, so an unauthenticated caller learns nothing
    // about what is mounted.
    let authorized = request
        .query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .any(|(key, value)| key == "token" && token.matches(&percent_decode(value)))
        || request
            .header("authorization")
            .and_then(|value| value.strip_prefix("Bearer "))
            .is_some_and(|candidate| token.matches(candidate.trim()));
    if !authorized {
        return refuse(Refusal::Unauthorized);
    }

    // (3) Routing: `/<session>/<extension>/<rest>`.
    let routed = match route(&request.path) {
        Ok(routed) => routed,
        Err(refusal) => return refuse(refusal),
    };

    // (4) Resolve the namespace. An unknown one answers 404 with the same body
    // as "not found", so the port cannot be walked to enumerate extensions.
    let namespace = {
        let listing = lock(namespaces);
        listing
            .iter()
            .find(|candidate| {
                candidate.extension() == routed.extension && candidate.session() == routed.session
            })
            .cloned()
    };
    let Some(namespace) = namespace else {
        return refuse(Refusal::UnknownNamespace(format!(
            "{}/{}",
            routed.session, routed.extension
        )));
    };

    // The guest sees the stripped path, which is the contract that lets
    // `index.html` and `./style.css` work without knowing the mount point.
    let base_url = format!(
        "{}/{}/{}/",
        origin_origin(authority),
        routed.session,
        routed.extension
    );
    let mut request = request;
    request.path = routed.rest;
    match namespace.answer(&base_url, request) {
        Ok(response) => response,
        Err(err) => {
            // One bad handler must not take the listener down: answer 500,
            // record it, and keep accepting (FR-EXT-3/4).
            namespace.record_refusal("handler", &err.to_string());
            HttpResponse::text(500, format!("the extension failed: {err}"))
        }
    }
}

/// `authority` is `host:port`; the base URL needs the scheme back.
fn origin_origin(authority: &str) -> String {
    format!("http://{authority}")
}

fn record_on_all(
    namespaces: &Arc<Mutex<Vec<Arc<dyn ServeHttpNamespace>>>>,
    capability: &str,
    parameter: &str,
    reason: &str,
) {
    // A rebinding attempt arrived on the shared port, so no single extension
    // owns it. Journal it against every mounted extension: the user reading
    // `ext info` should see that something probed the listener, and the
    // alternative - a port-wide record with no home - would be invisible.
    let listing = lock(namespaces);
    for namespace in listing.iter() {
        namespace.record_refusal(&format!("{capability}/{parameter}"), reason);
    }
}

/// Whether a `Host` header names this listener.
///
/// Accepts `127.0.0.1:<port>`, `localhost:<port>`, and the same without a port
/// (a `curl --http1.0` may send a bare host). IPv6 loopback is accepted in
/// bracketed form for a caller that resolved `::1`.
fn host_matches(host: &str, authority: &str) -> bool {
    let host = host.trim().to_ascii_lowercase();
    if host == authority.to_ascii_lowercase() {
        return true;
    }
    let Some((_, port)) = authority.rsplit_once(':') else {
        return false;
    };
    let expected = [
        format!("localhost:{port}"),
        format!("[::1]:{port}"),
        "localhost".to_string(),
        "127.0.0.1".to_string(),
        "[::1]".to_string(),
    ];
    expected.contains(&host)
}

/// `/<session>/<extension>/<rest>` → its three parts, with `rest` starting `/`.
///
/// A path with no trailing content (`/<session>/<ext>`) routes with `rest` of
/// `/`, so the extension's index handler sees the same thing a browser
/// requesting `/` would.
pub fn route(path: &str) -> Result<Routed, Refusal> {
    let mut segments = path.splitn(4, '/');
    // A leading `/` yields an empty first segment.
    let _ = segments.next();
    let session = segments.next().unwrap_or("");
    let extension = segments.next().unwrap_or("");
    let rest = segments.next().unwrap_or("");
    if session.is_empty() || extension.is_empty() {
        return Err(Refusal::NoRoute);
    }
    // Segments are single components: a `.` or `..` here is a traversal
    // attempt against the mount, never a legitimate name (extension names
    // cannot contain them either - the manifest identifier rules are
    // `[a-z0-9-]`).
    if session == "." || session == ".." || extension == "." || extension == ".." {
        return Err(Refusal::UnknownNamespace(format!("{session}/{extension}")));
    }
    Ok(Routed {
        session: session.to_string(),
        extension: extension.to_string(),
        rest: format!("/{rest}"),
    })
}

/// Percent-decode one query value (the token arrives encoded if it ever holds
/// a character a browser escapes; a hex-only token never does, but a
/// hand-written `?token=` in a terminal may).
fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hi = (bytes[index + 1] as char).to_digit(16);
            let lo = (bytes[index + 2] as char).to_digit(16);
            if let (Some(hi), Some(lo)) = (hi, lo) {
                out.push((hi as u8) << 4 | lo as u8);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn refuse(refusal: Refusal) -> HttpResponse {
    HttpResponse::text(refusal.status(), refusal.body())
}

/// Write the response with the host's own framing.
///
/// `content-length` and `connection` are the host's, not the guest's: the host
/// owns the socket, so it owns the framing that tells the client how much to
/// read. A guest that set its own `content-length` would be able to hang a
/// browser by lying about it.
fn write_response(stream: &mut TcpStream, response: &HttpResponse) {
    let mut head = format!(
        "HTTP/1.1 {} {}\r\ncontent-length: {}\r\nconnection: close\r\n",
        response.status,
        status_text(response.status),
        response.body.len()
    );
    for (name, value) in &response.headers {
        let lowered = name.to_ascii_lowercase();
        if lowered == "content-length" || lowered == "connection" || lowered == "transfer-encoding"
        {
            continue;
        }
        // A header value with a newline would inject a header; refuse to write
        // it rather than trusting the guest (the response is host-framed, but
        // the values are not).
        if value.contains(['\r', '\n']) {
            continue;
        }
        head.push_str(&format!("{lowered}: {value}\r\n"));
    }
    head.push_str("\r\n");
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(&response.body);
    let _ = stream.flush();
}

/// The reason phrase for a status the host writes itself. A guest may return
/// any status; an unknown one gets an empty reason phrase, which HTTP allows.
fn status_text(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        301 => "Moved Permanently",
        302 => "Found",
        304 => "Not Modified",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Payload Too Large",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        503 => "Service Unavailable",
        _ => "",
    }
}

/// Expose the `Host` predicate to the unit rows without making it part of the
/// public surface (it is a policy detail, not an API).
#[doc(hidden)]
#[must_use]
pub fn host_matches_for_test(host: &str, authority: &str) -> bool {
    host_matches(host, authority)
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Capabilities {
    /// Record a `serve-http` refusal (gh #363). The listener owns the socket
    /// and the gate; this is how a refusal or a failed handler reaches the
    /// extension's own denial journal, which is what `ext info` counts.
    pub fn note_serve_http(&self, parameter: &str, reason: &str) {
        self.record("serve-http", parameter, reason);
    }

    /// Whether this extension declared `serve-http` at all (gh #363). The
    /// listener mounts a namespace only when this is true, so an ungranted
    /// extension has no route to be reached on rather than a route that
    /// refuses.
    pub fn serves_http(&self) -> bool {
        self.grants.serve_http.is_some()
    }

    /// [`Capabilities::refused`] for callers outside this module (the host's
    /// `serve-http` mount records through the engine, not around it).
    pub fn refused_for(
        &self,
        capability: &str,
        parameter: &str,
        err: CapabilityError,
    ) -> CapabilityError {
        self.refused(capability, parameter, err)
    }
}
