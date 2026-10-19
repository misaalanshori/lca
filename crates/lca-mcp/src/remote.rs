//! Remote servers: the streamable-HTTP transport over `net` plus
//! the OAuth loop (dynamic registration, token store, step-up
//! scopes). Native only: the sandboxed guest stays stdio until the
//! phase-3 management owns server URLs (a `net` import on the
//! `tool-catalog` world is a WIT change, deferred under the freeze).
//!
//! Every function takes the engine behind `Arc`: exchanges run on
//! timeout threads, which need `'static`.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use crate::{
    HttpServerConfig, StoredToken, is_transient_status, merge_scopes, outcome_from_result,
    parse_www_authenticate, tools_from_list,
};

/// The needs-sign-in marker: every sign-in report contains it, and
/// the managed connect matches on it to sort a 401 into state
/// instead of failure (one builder, one matcher, one test pinning
/// the words).
pub(crate) const NEEDS_AUTH_MARKER: &str = "requires sign-in";

/// One PKCE pair (RFC 7636): a random verifier and its S256
/// challenge. Local (not the subscription kit's): the bridge must
/// not drag the provider-login kit into the binary for two pure
/// functions (NFR-1's budget).
pub fn pkce_pair() -> Result<(String, String), String> {
    let mut verifier_bytes = [0u8; 32];
    getrandom::fill(&mut verifier_bytes)
        .map_err(|err| format!("the platform CSPRNG failed: {err}"))?;
    let verifier = base64url(&verifier_bytes);
    let challenge = {
        use sha2::Digest as _;
        base64url(&sha2::Sha256::digest(verifier.as_bytes()))
    };
    Ok((verifier, challenge))
}

/// Unpadded base64url.
fn base64url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(ALPHABET[((n >> 18) & 63) as usize] as char);
        out.push(ALPHABET[((n >> 12) & 63) as usize] as char);
        if chunk.len() > 1 {
            out.push(ALPHABET[((n >> 6) & 63) as usize] as char);
        }
        if chunk.len() > 2 {
            out.push(ALPHABET[(n & 63) as usize] as char);
        }
    }
    out
}

/// Percent-encode one URL or form field (uppercase hex).
pub fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// pi's connect delays, reused between request attempts.
const RETRY_DELAYS: [Duration; 2] = [Duration::from_millis(250), Duration::from_millis(1000)];
/// Attempts for one idempotent read (the first try plus pi's two retries).
const READ_ATTEMPTS: usize = 3;
/// An access token this close to expiry refreshes before it is sent
/// (pi's refresh skew).
const REFRESH_SKEW_MS: u64 = 30_000;
/// Fallback lifetime when the server sends no `expires_in`.
pub(crate) const DEFAULT_TOKEN_LIFETIME_SECS: u64 = 3600;

/// Run `work` on a thread and wait at most `timeout`: the `net`
/// engine bounds body reads but not the head wait, so the bridge
/// bounds the whole exchange itself. An expiry abandons the exchange
/// (the engine owns the in-flight handle); the error names the wait.
pub(crate) fn with_timeout<T: Send + 'static>(
    timeout: Duration,
    what: &str,
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = done_tx.send(work());
    });
    match done_rx.recv_timeout(timeout) {
        Ok(outcome) => outcome,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            Err(format!("{what} timed out after {}s", timeout.as_secs()))
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            Err(format!("{what} went away before answering"))
        }
    }
}

/// The token store: one credentials key per server (pi's
/// `mcp-auth.json` shape, keyed by name and URL behind the host's
/// secret storage instead of a plaintext file).
#[derive(Clone)]
pub struct TokenStore {
    caps: Arc<lca_tools::Capabilities>,
    key: String,
}

impl TokenStore {
    /// Open the store for one server.
    pub fn new(caps: Arc<lca_tools::Capabilities>, server: &HttpServerConfig) -> TokenStore {
        TokenStore {
            caps,
            key: crate::token_key(&server.name, &server.url),
        }
    }

    /// The stored tokens, if the server signed in before.
    pub fn load(&self) -> Result<Option<StoredToken>, String> {
        let raw = self
            .caps
            .credentials_get(&self.key)
            .map_err(|err| format!("cannot read the token store: {err}"))?;
        raw.map(|json| {
            serde_json::from_str(&json).map_err(|err| format!("the stored token is corrupt: {err}"))
        })
        .transpose()
    }

    /// Persist freshly issued tokens.
    pub fn save(&self, token: &StoredToken) -> Result<(), String> {
        let json = serde_json::to_string(token)
            .map_err(|err| format!("cannot encode the token: {err}"))?;
        self.caps
            .credentials_set(&self.key, &json)
            .map_err(|err| format!("cannot store the token: {err}"))
    }

    /// Forget the server (a rejected refresh or an explicit sign-out):
    /// the next call reports that sign-in is needed.
    pub fn clear(&self) -> Result<(), String> {
        self.caps
            .credentials_delete(&self.key)
            .map_err(|err| format!("cannot clear the token store: {err}"))?;
        // Pending scopes die with the grant; a best-effort delete
        // (absence is the common case).
        let _ = self.caps.credentials_delete(&self.pending_key());
        Ok(())
    }

    /// Forget the remembered challenge (a best-effort delete; a finished
    /// sign-in's step-up scopes are now in the grant itself).
    pub fn clear_pending(&self) -> Result<(), String> {
        self.caps
            .credentials_delete(&self.pending_key())
            .map_err(|err| format!("cannot clear the pending challenge: {err}"))
    }

    fn pending_key(&self) -> String {
        // Rebuilt from the token key so the two can never drift apart.
        self.key.replacen("mcp-oauth:", "mcp-oauth-pending:", 1)
    }

    /// The `WWW-Authenticate` challenge a 401 advertised, waiting for the
    /// next sign-in.
    ///
    /// gh #418 Phase D: the whole challenge is kept, not just `scope`. The
    /// `resource_metadata` member is the RFC 9728 pointer to the server's
    /// protected-resource metadata - the only way to find an authorization
    /// server that lives on a different host. Discarding it made that
    /// discovery branch dead, which is half of gh #290.
    pub fn pending_challenge(&self) -> Result<Option<BTreeMap<String, String>>, String> {
        let Some(stored) = self
            .caps
            .credentials_get(&self.pending_key())
            .map_err(|err| format!("cannot read the pending challenge: {err}"))?
        else {
            return Ok(None);
        };
        // Stored as JSON; a value from an older build was a bare scope list,
        // so a parse failure is reported as its scope rather than dropped.
        match serde_json::from_str::<serde_json::Value>(&stored) {
            Ok(serde_json::Value::Object(object)) => Ok(Some(
                object
                    .into_iter()
                    .filter_map(|(key, value)| value.as_str().map(|text| (key, text.to_string())))
                    .collect(),
            )),
            _ => Ok(merge_scopes(&[Some(stored.as_str())]).map(|scope| {
                let mut challenge = BTreeMap::new();
                challenge.insert("scope".to_string(), scope);
                challenge
            })),
        }
    }

    /// Remember where the sign-in's authorization server lives, for the
    /// sign-in screen's copy (gh #418 Phase D's consent property: a
    /// discovered host is named before the credential goes there). Empty
    /// clears it.
    pub fn set_discovery_note(&self, note: &str) -> Result<(), String> {
        if note.is_empty() {
            let _ = self.caps.credentials_delete(&self.note_key());
            return Ok(());
        }
        self.caps
            .credentials_set(&self.note_key(), note)
            .map_err(|err| format!("cannot store the discovery note: {err}"))
    }

    /// The stored discovery note, when a sign-in discovered an outside host.
    pub fn discovery_note(&self) -> Result<Option<String>, String> {
        self.caps
            .credentials_get(&self.note_key())
            .map_err(|err| format!("cannot read the discovery note: {err}"))
    }

    fn note_key(&self) -> String {
        self.key.replacen("mcp-oauth:", "mcp-oauth-note:", 1)
    }

    /// Remember an advertised challenge for the next sign-in: the new values
    /// win, `scope` merges (step-up adds scopes, never drops them).
    pub fn set_pending_challenge(
        &self,
        challenge: &BTreeMap<String, String>,
    ) -> Result<(), String> {
        let mut remembered = self.pending_challenge()?.unwrap_or_default();
        for (key, value) in challenge {
            if key == "scope" {
                if let Some(merged) = merge_scopes(&[
                    remembered.get(key).map(String::as_str),
                    Some(value.as_str()),
                ]) {
                    remembered.insert(key.clone(), merged);
                }
            } else {
                remembered.insert(key.clone(), value.clone());
            }
        }
        let encoded = serde_json::to_string(&remembered)
            .map_err(|err| format!("cannot encode the pending challenge: {err}"))?;
        self.caps
            .credentials_set(&self.pending_key(), &encoded)
            .map_err(|err| format!("cannot store the pending challenge: {err}"))
    }
}

/// Milliseconds since the Unix epoch.
pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_millis() as u64)
        .unwrap_or(0)
}

/// One buffered HTTP exchange.
pub(crate) struct Attempt {
    pub(crate) status: u16,
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) body: Vec<u8>,
}

/// What one attempt can report, split so denials never retry.
pub(crate) enum AttemptOutcome {
    Exchanged(Attempt),
    /// A transport failure worth another attempt.
    Retryable(String),
    /// A permission refusal: surface at once.
    Denied(String),
}

/// POST one envelope and buffer the answer.
pub(crate) fn post_attempt(
    caps: &lca_tools::Capabilities,
    url: &str,
    headers: &[(String, String)],
    body: &[u8],
) -> AttemptOutcome {
    let refs: Vec<(&str, &str)> = headers
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    let handle = match caps.oauth_flow_net_request("POST", url, &refs, Some(body)) {
        Ok(handle) => handle,
        Err(err) => {
            let detail = err.to_string();
            return match err {
                lca_protocol::CapabilityError::Permission(_)
                | lca_protocol::CapabilityError::NotGranted(_) => AttemptOutcome::Denied(detail),
                _ => AttemptOutcome::Retryable(format!("request failed: {detail}")),
            };
        }
    };
    let status = match caps.net_response_status(handle) {
        Ok(status) => status,
        Err(err) => {
            let _ = caps.net_close_response(handle);
            return AttemptOutcome::Retryable(format!("no status: {err}"));
        }
    };
    let headers = caps.net_response_headers(handle).unwrap_or_default();
    let mut body = Vec::new();
    loop {
        match caps.net_read_body(handle, 64 * 1024) {
            Ok(Some(chunk)) => body.extend_from_slice(&chunk),
            Ok(None) => break,
            Err(err) => {
                let _ = caps.net_close_response(handle);
                return AttemptOutcome::Retryable(format!("body read failed: {err}"));
            }
        }
    }
    let _ = caps.net_close_response(handle);
    AttemptOutcome::Exchanged(Attempt {
        status,
        headers,
        body,
    })
}

/// GET one document (discovery and metadata live here).
pub(crate) fn get_document(
    caps: Arc<lca_tools::Capabilities>,
    timeout: Duration,
    url: &str,
) -> Result<serde_json::Value, String> {
    // Validated before any request: a metadata URL on a public `http://` host
    // would otherwise leak the request (and, on the token side, the grant).
    crate::oauth::checked_endpoint(url, "a metadata URL")?;
    let url = url.to_string();
    with_timeout(timeout, "the metadata fetch", move || {
        let refs: Vec<(&str, &str)> = Vec::new();
        let handle = caps
            .oauth_flow_net_request("GET", &url, &refs, None)
            .map_err(|err| format!("fetch failed: {err}"))?;
        let status = caps.net_response_status(handle).unwrap_or(0);
        let mut body = Vec::new();
        loop {
            match caps.net_read_body(handle, 64 * 1024) {
                Ok(Some(chunk)) => body.extend_from_slice(&chunk),
                Ok(None) => break,
                Err(err) => {
                    let _ = caps.net_close_response(handle);
                    return Err(format!("fetch body failed: {err}"));
                }
            }
        }
        let _ = caps.net_close_response(handle);
        if status != 200 {
            return Err(format!("metadata fetch answered {status}"));
        }
        serde_json::from_slice(&body).map_err(|err| format!("bad metadata JSON: {err}"))
    })
}

/// Find one header regardless of case.
fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

/// Probe one MCP endpoint and remember the challenge a 401 carries.
///
/// The capture half of gh #418 Phase D, split out so a caller (the manager's
/// sign-in entry point, or a test) can do exactly what a failed call does:
/// one unauthenticated request, and on a 401 the `WWW-Authenticate` members
/// are stored for the next sign-in. A 200 means nothing to capture.
pub fn remember_unauthorized(
    caps: &Arc<lca_tools::Capabilities>,
    server: &HttpServerConfig,
    url: &str,
) -> Result<Option<BTreeMap<String, String>>, String> {
    let refs: Vec<(&str, &str)> = Vec::new();
    let handle = caps
        .oauth_flow_net_request("POST", url, &refs, Some(b"{}"))
        .map_err(|err| format!("the probe request failed: {err}"))?;
    let status = caps
        .net_response_status(handle)
        .map_err(|err| format!("no status: {err}"))?;
    let headers = caps
        .net_response_headers(handle)
        .map_err(|err| format!("cannot read the response headers: {err}"))?;
    let _ = caps.net_close_response(handle);
    if status != 401 {
        return Ok(None);
    }
    let challenge = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("www-authenticate"))
        .map(|(_, value)| parse_www_authenticate(value))
        .unwrap_or_default();
    if challenge.is_empty() {
        return Ok(None);
    }
    let store = TokenStore::new(caps.clone(), server);
    store.set_pending_challenge(&challenge)?;
    Ok(Some(challenge))
}

/// One live remote server behind the bridge.
pub struct HttpSession {
    caps: Arc<lca_tools::Capabilities>,
    server: HttpServerConfig,
    session_id: Option<String>,
    next_id: u64,
}

impl HttpSession {
    /// Wrap one configured server (no I/O yet; [`HttpSession::initialize`]
    /// handshakes).
    pub fn new(caps: Arc<lca_tools::Capabilities>, server: HttpServerConfig) -> HttpSession {
        HttpSession {
            caps,
            server,
            session_id: None,
            next_id: 1,
        }
    }

    /// The bearer token to send, refreshing a near-expiry grant first.
    /// `None` means anonymous (or static headers, which skip OAuth
    /// entirely, exactly like pi).
    fn token(&mut self) -> Result<Option<String>, String> {
        if self.server.has_static_auth() {
            return Ok(None);
        }
        let store = TokenStore::new(self.caps.clone(), &self.server);
        let Some(stored) = store.load()? else {
            return Ok(None);
        };
        let stale = stored
            .expires_at_ms
            .is_none_or(|at| at.saturating_sub(now_ms()) < REFRESH_SKEW_MS);
        if stale && stored.refresh_token.is_some() {
            // A dead grant purges inside `refresh`; anything else
            // keeps the stale token for one best-effort attempt.
            let _ = crate::oauth::refresh(self.caps.clone(), &self.server)?;
            return Ok(store.load()?.map(|fresh| fresh.access_token));
        }
        Ok(Some(stored.access_token))
    }

    /// The sign-in report: what to do (plus the missing scopes, when
    /// the server named them). A named scope is remembered in the
    /// store, so the next sign-in asks for the union (step-up). The
    /// stored access token is already gone by now (purged on the
    /// failed refresh, or never stored).
    fn remember_step_up(&self, headers: &[(String, String)]) -> String {
        // gh #418 Phase D: keep the *whole* challenge. `resource_metadata` is
        // the RFC 9728 pointer to the protected-resource metadata, and it is
        // the only way to find an authorization server on another host; the
        // old code kept `scope` alone and threw that away.
        let challenge = headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("www-authenticate"))
            .map(|(_, value)| parse_www_authenticate(value))
            .unwrap_or_default();
        let scope = challenge.get("scope").cloned();
        if !challenge.is_empty() {
            let store = TokenStore::new(self.caps.clone(), &self.server);
            // A best-effort memory: the report must survive even when
            // the store write cannot.
            let _ = store.set_pending_challenge(&challenge);
        }
        match scope {
            Some(scope) => format!(
                "MCP server {:?} requires sign-in (missing scopes: {scope})",
                self.server.name
            ),
            None => format!(
                "MCP server {:?} requires sign-in; sign in and retry the call",
                self.server.name
            ),
        }
    }

    /// POST one envelope: auth, the 401 refresh-and-retry, transient
    /// retries for idempotent reads, the per-request timeout, and the
    /// JSON-or-SSE decode.
    fn post(
        &mut self,
        method: &str,
        params: serde_json::Value,
        idempotent: bool,
    ) -> Result<serde_json::Value, String> {
        let attempts = if idempotent { READ_ATTEMPTS } else { 1 };
        let mut refreshed = false;
        let mut attempt = 0;
        loop {
            let id = self.next_id;
            self.next_id += 1;
            let body = serde_json::to_vec(&serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": method,
                "params": params,
            }))
            .map_err(|err| format!("cannot encode the MCP request: {err}"))?;
            // The headers read the store fresh every attempt, so a
            // refreshed grant rides the retry at once.
            let headers = self.headers()?;
            let url = self.server.url.clone();
            let timeout = self.server.timeout();
            let caps = self.caps.clone();
            let outcome = with_timeout(timeout, &format!("MCP {method}"), move || {
                Ok(post_attempt(&caps, &url, &headers, &body))
            });
            match outcome {
                Ok(AttemptOutcome::Denied(detail)) => return Err(detail),
                Err(_) if idempotent && attempt + 1 < attempts => {
                    std::thread::sleep(RETRY_DELAYS[attempt]);
                    attempt += 1;
                    continue;
                }
                Err(transport) => return Err(transport),
                Ok(AttemptOutcome::Retryable(_)) if idempotent && attempt + 1 < attempts => {
                    std::thread::sleep(RETRY_DELAYS[attempt]);
                    attempt += 1;
                    continue;
                }
                Ok(AttemptOutcome::Retryable(detail)) => return Err(detail),
                Ok(AttemptOutcome::Exchanged(exchange)) => {
                    if exchange.status == 401 && !refreshed {
                        if crate::oauth::refresh(self.caps.clone(), &self.server)? {
                            refreshed = true;
                            continue;
                        }
                        return Err(self.remember_step_up(&exchange.headers));
                    }
                    if exchange.status == 401 {
                        return Err(self.remember_step_up(&exchange.headers));
                    }
                    if is_transient_status(exchange.status) {
                        if idempotent && attempt + 1 < attempts {
                            std::thread::sleep(RETRY_DELAYS[attempt]);
                            attempt += 1;
                            continue;
                        }
                        return Err(format!(
                            "MCP server answered {}: {}",
                            exchange.status,
                            String::from_utf8_lossy(&exchange.body)
                        ));
                    }
                    if exchange.status == 202 {
                        return Err("MCP server accepted without answering".to_string());
                    }
                    if exchange.status != 200 {
                        return Err(format!(
                            "MCP server answered {}: {}",
                            exchange.status,
                            String::from_utf8_lossy(&exchange.body)
                        ));
                    }
                    if let Some(session) = header(&exchange.headers, "mcp-session-id") {
                        self.session_id = Some(session.to_string());
                    }
                    return self.decode(&exchange, id);
                }
            }
        }
    }

    /// Decode one answered envelope by its content type.
    fn decode(&self, exchange: &Attempt, id: u64) -> Result<serde_json::Value, String> {
        let text = std::str::from_utf8(&exchange.body)
            .map_err(|_| "MCP server answered non-UTF-8".to_string())?;
        let streamed = exchange
            .headers
            .iter()
            .filter(|(key, _)| key.eq_ignore_ascii_case("content-type"))
            .any(|(_, value)| value.contains("text/event-stream"));
        if streamed {
            return crate::wire::select_response(&crate::wire::parse_sse_messages(text), id);
        }
        let message: serde_json::Value = serde_json::from_str(text)
            .map_err(|err| format!("MCP server answered bad JSON: {err}"))?;
        crate::wire::select_response(std::slice::from_ref(&message), id)
    }

    /// The request headers: declared headers, the bearer token when
    /// one is in play (notifications authenticate exactly like
    /// requests), and the session id once minted.
    fn headers(&mut self) -> Result<Vec<(String, String)>, String> {
        let mut headers = vec![
            ("Content-Type".to_string(), "application/json".to_string()),
            (
                "Accept".to_string(),
                "application/json, text/event-stream".to_string(),
            ),
        ];
        headers.extend(self.server.headers.clone());
        if let Some(token) = self.token()?
            && !self.server.has_static_auth()
        {
            headers.push(("Authorization".to_string(), format!("Bearer {token}")));
        }
        if let Some(session) = &self.session_id {
            headers.push(("Mcp-Session-Id".to_string(), session.clone()));
        }
        Ok(headers)
    }

    /// POST a notification (no id): 202 is the answer.
    fn notify(&mut self, method: &str) -> Result<(), String> {
        let body = serde_json::to_vec(&serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
        }))
        .map_err(|err| format!("cannot encode the MCP request: {err}"))?;
        let headers = self.headers()?;
        let url = self.server.url.clone();
        let timeout = self.server.timeout();
        let caps = self.caps.clone();
        let outcome = with_timeout(timeout, &format!("MCP {method}"), move || {
            Ok(match post_attempt(&caps, &url, &headers, &body) {
                AttemptOutcome::Exchanged(exchange) => exchange,
                AttemptOutcome::Retryable(detail) | AttemptOutcome::Denied(detail) => {
                    return Err(detail);
                }
            })
        });
        match outcome {
            Ok(exchange) if exchange.status == 202 || exchange.status == 200 => Ok(()),
            Ok(exchange) => Err(format!(
                "MCP server answered the notification with {}",
                exchange.status
            )),
            Err(transport) => Err(transport),
        }
    }

    /// The MCP opening handshake over POST. Returns whether the
    /// server offers resources.
    pub fn initialize(&mut self) -> Result<bool, String> {
        let result = self.post(
            "initialize",
            serde_json::json!({
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": {"name": "lca", "version": "0.6.0"},
            }),
            true,
        )?;
        if result.get("protocolVersion").is_none() {
            return Err("MCP server omitted its protocol version".to_string());
        }
        let offers = result
            .get("capabilities")
            .and_then(|capabilities| capabilities.get("resources"))
            .is_some();
        self.notify("notifications/initialized")?;
        Ok(offers)
    }

    /// List one page of resources plus the next cursor.
    pub fn resource_list(
        &mut self,
        cursor: Option<&str>,
    ) -> Result<(Vec<crate::McpResource>, Option<String>), String> {
        let mut params = serde_json::json!({});
        if let Some(cursor) = cursor {
            params["cursor"] = cursor.into();
        }
        let result = self.post("resources/list", params, true)?;
        Ok(crate::resources_from_list(&result))
    }

    /// List the URI templates (method-not-found reads as no
    /// templates, pi's `withoutTemplates`; anything else errors).
    pub fn resource_templates(&mut self) -> Result<Vec<crate::McpResourceTemplate>, String> {
        match self.post("resources/templates/list", serde_json::json!({}), true) {
            Ok(result) => Ok(crate::templates_from_list(&result)),
            Err(err) if err.contains("-32601") => Ok(Vec::new()),
            Err(err) => Err(err),
        }
    }

    /// Read one resource by URI.
    pub fn resource_read(&mut self, uri: &str) -> Result<Vec<crate::ResourceContent>, String> {
        let result = self.post("resources/read", serde_json::json!({"uri": uri}), true)?;
        crate::contents_from_read(&result)
    }

    /// List the server's tools, qualified for `server`.
    pub fn tools(&mut self, server: &str) -> Result<Vec<crate::ServerTool>, String> {
        let result = self.post("tools/list", serde_json::json!({}), true)?;
        tools_from_list(server, &result)
    }

    /// Call one server tool by its server-side name (never retried).
    pub fn call(
        &mut self,
        name: &str,
        arguments: &serde_json::Value,
    ) -> Result<crate::ToolOutcome, String> {
        let result = self.post(
            "tools/call",
            serde_json::json!({"name": name, "arguments": arguments}),
            false,
        )?;
        Ok(outcome_from_result(&result))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Verifies: gh #53 - PKCE matches the RFC 7636 appendix vector
    // (the bridge carries its own pair, not the subscription kit's,
    // for NFR-1's budget).
    #[test]
    fn pkce_matches_the_rfc_vector() {
        use sha2::Digest as _;
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let challenge = base64url(&sha2::Sha256::digest(verifier.as_bytes()));
        assert_eq!(challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
        let (verifier, challenge) = pkce_pair().expect("random pair");
        assert_eq!(verifier.len(), 43);
        assert_eq!(challenge.len(), 43);
        assert_eq!(percent_encode("a b/c?"), "a%20b%2Fc%3F");
    }
}
