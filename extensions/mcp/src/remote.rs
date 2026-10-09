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

/// pi's connect delays, reused between request attempts.
const RETRY_DELAYS: [Duration; 2] = [Duration::from_millis(250), Duration::from_millis(1000)];
/// Attempts for one idempotent read (the first try plus pi's two retries).
const READ_ATTEMPTS: usize = 3;
/// An access token this close to expiry refreshes before it is sent
/// (pi's refresh skew).
const REFRESH_SKEW_MS: u64 = 30_000;
/// Fallback lifetime when the server sends no `expires_in`.
const DEFAULT_TOKEN_LIFETIME_SECS: u64 = 3600;

/// Run `work` on a thread and wait at most `timeout`: the `net`
/// engine bounds body reads but not the head wait, so the bridge
/// bounds the whole exchange itself. An expiry abandons the exchange
/// (the engine owns the in-flight handle); the error names the wait.
fn with_timeout<T: Send + 'static>(
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

    fn pending_key(&self) -> String {
        // Rebuilt from the token key so the two can never drift apart.
        self.key.replacen("mcp-oauth:", "mcp-oauth-pending:", 1)
    }

    /// The scopes a 401 advertised, waiting for the next sign-in.
    pub fn pending_scope(&self) -> Result<Option<String>, String> {
        self.caps
            .credentials_get(&self.pending_key())
            .map_err(|err| format!("cannot read the pending scopes: {err}"))
    }

    /// Remember advertised scopes for the next sign-in (the union
    /// with what is already pending).
    pub fn set_pending(&self, scope: &str) -> Result<(), String> {
        let merged =
            merge_scopes(&[self.pending_scope()?.as_deref(), Some(scope)]).unwrap_or_default();
        self.caps
            .credentials_set(&self.pending_key(), &merged)
            .map_err(|err| format!("cannot store the pending scopes: {err}"))
    }
}

/// Milliseconds since the Unix epoch.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_millis() as u64)
        .unwrap_or(0)
}

/// One buffered HTTP exchange.
struct Attempt {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

/// What one attempt can report, split so denials never retry.
enum AttemptOutcome {
    Exchanged(Attempt),
    /// A transport failure worth another attempt.
    Retryable(String),
    /// A permission refusal: surface at once.
    Denied(String),
}

/// POST one envelope and buffer the answer.
fn post_attempt(
    caps: &lca_tools::Capabilities,
    url: &str,
    headers: &[(String, String)],
    body: &[u8],
) -> AttemptOutcome {
    let refs: Vec<(&str, &str)> = headers
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    let handle = match caps.net_request("POST", url, &refs, Some(body)) {
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
fn get_document(
    caps: Arc<lca_tools::Capabilities>,
    timeout: Duration,
    url: &str,
) -> Result<serde_json::Value, String> {
    let url = url.to_string();
    with_timeout(timeout, "the metadata fetch", move || {
        let refs: Vec<(&str, &str)> = Vec::new();
        let handle = caps
            .net_request("GET", &url, &refs, None)
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

/// The authorization server behind one MCP server (RFC 8414's
/// document, however it was found).
struct IdpMeta {
    authorization_endpoint: String,
    token_endpoint: String,
    registration_endpoint: Option<String>,
}

impl IdpMeta {
    fn from_document(document: &serde_json::Value) -> Result<IdpMeta, String> {
        let string = |key: &str| {
            document
                .get(key)
                .and_then(|value| value.as_str())
                .map(str::to_string)
        };
        Ok(IdpMeta {
            authorization_endpoint: string("authorization_endpoint")
                .ok_or_else(|| "the metadata names no authorization endpoint".to_string())?,
            token_endpoint: string("token_endpoint")
                .ok_or_else(|| "the metadata names no token endpoint".to_string())?,
            registration_endpoint: string("registration_endpoint"),
        })
    }
}

/// The origin (`scheme://host[:port]`) of a URL.
fn origin_of(url: &str) -> Result<String, String> {
    let mut parts = url.splitn(4, '/');
    let scheme = parts.next().unwrap_or("");
    let empty = parts.next().unwrap_or("");
    let host = parts.next().unwrap_or("");
    if scheme.is_empty() || empty.is_empty() || host.is_empty() {
        return Err(format!("cannot take the origin of {url:?}"));
    }
    Ok(format!("{scheme}//{host}"))
}

/// Find the authorization server: the configured metadata document
/// wins (pi's `authServerMetadataUrl`), else the challenge's
/// `resource_metadata` (RFC 9728), else the MCP origin as its own
/// issuer.
fn discover(
    caps: Arc<lca_tools::Capabilities>,
    server: &HttpServerConfig,
    challenge: &BTreeMap<String, String>,
) -> Result<IdpMeta, String> {
    if let Some(url) = server
        .oauth
        .as_ref()
        .and_then(|oauth| oauth.auth_server_metadata_url.clone())
    {
        return IdpMeta::from_document(&get_document(caps, server.timeout(), &url)?);
    }
    if let Some(metadata_url) = challenge.get("resource_metadata") {
        let document = get_document(caps.clone(), server.timeout(), metadata_url)?;
        let issuer = document
            .get("authorization_servers")
            .and_then(|servers| servers.as_array())
            .and_then(|servers| servers.first())
            .and_then(|first| first.as_str())
            .ok_or_else(|| "the resource metadata names no authorization server".to_string())?;
        let url = format!(
            "{}/.well-known/oauth-authorization-server",
            issuer.trim_end_matches('/')
        );
        return IdpMeta::from_document(&get_document(caps, server.timeout(), &url)?);
    }
    let url = format!(
        "{}/.well-known/oauth-authorization-server",
        origin_of(&server.url)?
    );
    IdpMeta::from_document(&get_document(caps, server.timeout(), &url)?)
}

/// Register this client (RFC 7591): the name the server sees, the
/// loopback redirect, and the requested scopes.
fn register_client(
    caps: Arc<lca_tools::Capabilities>,
    server: &HttpServerConfig,
    meta: &IdpMeta,
    redirect_uri: &str,
    scope: Option<&str>,
) -> Result<(String, Option<String>), String> {
    let endpoint = meta.registration_endpoint.clone().ok_or_else(|| {
        "the server supports no dynamic registration; configure a client id (phase 3)".to_string()
    })?;
    let client_name = server
        .oauth
        .as_ref()
        .and_then(|oauth| oauth.client_name.clone())
        .unwrap_or_else(|| "lca".to_string());
    let mut registration = serde_json::json!({
        "client_name": client_name,
        "redirect_uris": [redirect_uri],
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
    });
    if let Some(scope) = scope {
        registration["scope"] = scope.into();
    }
    let body = serde_json::to_vec(&registration)
        .map_err(|err| format!("cannot encode the registration: {err}"))?;
    let caps_moved = caps.clone();
    let endpoint_moved = endpoint.clone();
    let attempt = with_timeout(server.timeout(), "the client registration", move || {
        let headers = vec![("Content-Type".to_string(), "application/json".to_string())];
        match post_attempt(&caps_moved, &endpoint_moved, &headers, &body) {
            AttemptOutcome::Exchanged(attempt) => Ok(attempt),
            AttemptOutcome::Retryable(detail) | AttemptOutcome::Denied(detail) => Err(detail),
        }
    })?;
    if attempt.status != 200 && attempt.status != 201 {
        return Err(format!(
            "registration answered {}: {}",
            attempt.status,
            String::from_utf8_lossy(&attempt.body)
        ));
    }
    let answer: serde_json::Value = serde_json::from_slice(&attempt.body)
        .map_err(|err| format!("bad registration JSON: {err}"))?;
    let client_id = answer
        .get("client_id")
        .and_then(|id| id.as_str())
        .ok_or_else(|| "registration answered no client id".to_string())?
        .to_string();
    let client_secret = answer
        .get("client_secret")
        .and_then(|secret| secret.as_str())
        .map(str::to_string);
    Ok((client_id, client_secret))
}

/// The interactive sign-in (the anthropic browser flow's shape):
/// register (or reuse the configured client), open the authorize URL,
/// wait for the pasted or looped-back code, exchange, store. The
/// requested scopes are the configured ones plus any stored grant
/// (step-up merges, never replaces).
pub fn sign_in(
    caps: Arc<lca_tools::Capabilities>,
    server: &HttpServerConfig,
) -> Result<(), String> {
    let store = TokenStore::new(caps.clone(), server);
    let stored = store.load()?;
    let pending = store.pending_scope()?;
    let scope = merge_scopes(&[
        server
            .oauth
            .as_ref()
            .and_then(|oauth| oauth.scope.as_deref()),
        stored.as_ref().and_then(|token| token.scope.as_deref()),
        pending.as_deref(),
    ]);
    // A server that answered 401 names what it needs; without a
    // challenge the configured (plus stored) scopes are the request.
    let (redirect_uri, flow) = caps
        .oauth_begin("/callback")
        .map_err(|err| format!("cannot start the loopback flow: {err}"))?;
    let meta = discover(caps.clone(), server, &BTreeMap::new())?;
    let (verifier, challenge) =
        lca_subscription::pkce_pair().map_err(|err| format!("PKCE failed: {}", err.0))?;
    // pi sends the verifier as the state; the equality check then
    // covers both values at once.
    let state = verifier.clone();
    let (client_id, client_secret) = match server
        .oauth
        .as_ref()
        .and_then(|oauth| oauth.client_id.clone())
    {
        Some(configured) => (
            configured,
            server
                .oauth
                .as_ref()
                .and_then(|oauth| oauth.client_secret.clone()),
        ),
        None => register_client(caps.clone(), server, &meta, &redirect_uri, scope.as_deref())?,
    };
    let encode = lca_subscription::percent_encode;
    let mut url = format!(
        "{}?response_type=code&client_id={}&redirect_uri={}&code_challenge={}&code_challenge_method=S256&state={}",
        meta.authorization_endpoint,
        encode(&client_id),
        encode(&redirect_uri),
        encode(&challenge),
        encode(&state),
    );
    if let Some(scope) = &scope {
        url.push_str(&format!("&scope={}", encode(scope)));
    }
    let _ = caps.oauth_open(&url);
    let callback = caps.oauth_await(flow);
    let _ = caps.oauth_end(flow);
    let callback = callback.map_err(|err| err.to_string())?;
    let get = |key: &str| {
        callback
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.clone())
            .unwrap_or_default()
    };
    if get("state") != state {
        return Err("OAuth state mismatch; the callback did not come from this flow".to_string());
    }
    let code = get("code");
    if code.is_empty() {
        return Err("the authorization server returned no code".to_string());
    }
    exchange(
        caps.clone(),
        server,
        &meta,
        &store,
        &client_id,
        client_secret.as_deref(),
        &code,
        &redirect_uri,
        &verifier,
        scope.as_deref(),
    )
}

/// Exchange a code (or a refresh token) at the token endpoint; the
/// `grant` closure shapes the form body per grant type.
fn token_post(
    caps: Arc<lca_tools::Capabilities>,
    server: &HttpServerConfig,
    token_url: &str,
    pairs: &[(&str, &str)],
) -> Result<serde_json::Value, String> {
    let encoded = pairs
        .iter()
        .map(|(key, value)| {
            format!(
                "{}={}",
                lca_subscription::percent_encode(key),
                lca_subscription::percent_encode(value)
            )
        })
        .collect::<Vec<_>>()
        .join("&");
    let token_url = token_url.to_string();
    let attempt = with_timeout(server.timeout(), "the token exchange", move || {
        let headers = vec![(
            "Content-Type".to_string(),
            "application/x-www-form-urlencoded".to_string(),
        )];
        match post_attempt(&caps, &token_url, &headers, encoded.as_bytes()) {
            AttemptOutcome::Exchanged(attempt) => Ok(attempt),
            AttemptOutcome::Retryable(detail) | AttemptOutcome::Denied(detail) => Err(detail),
        }
    })?;
    if attempt.status != 200 {
        return Err(format!(
            "token endpoint answered {}: {}",
            attempt.status,
            String::from_utf8_lossy(&attempt.body)
        ));
    }
    serde_json::from_slice(&attempt.body).map_err(|err| format!("bad token JSON: {err}"))
}

/// Store an issuance (code or refresh grant) against its client.
fn store_issuance(
    store: &TokenStore,
    answer: &serde_json::Value,
    client_id: &str,
    client_secret: Option<&str>,
    token_url: &str,
    fallback_scope: Option<&str>,
) -> Result<(), String> {
    let access_token = answer
        .get("access_token")
        .and_then(|token| token.as_str())
        .ok_or_else(|| "the token endpoint issued no access token".to_string())?
        .to_string();
    let lifetime = answer
        .get("expires_in")
        .and_then(|secs| secs.as_u64())
        .unwrap_or(DEFAULT_TOKEN_LIFETIME_SECS);
    store.save(&StoredToken {
        access_token,
        refresh_token: answer
            .get("refresh_token")
            .and_then(|token| token.as_str())
            .map(str::to_string),
        expires_at_ms: Some(now_ms() + lifetime * 1000),
        client_id: client_id.to_string(),
        client_secret: client_secret.map(str::to_string),
        scope: answer
            .get("scope")
            .and_then(|scope| scope.as_str())
            .map(str::to_string)
            .or_else(|| fallback_scope.map(str::to_string)),
        token_url: token_url.to_string(),
    })
}

/// The code exchange at the end of [`sign_in`].
#[allow(clippy::too_many_arguments)]
fn exchange(
    caps: Arc<lca_tools::Capabilities>,
    server: &HttpServerConfig,
    meta: &IdpMeta,
    store: &TokenStore,
    client_id: &str,
    client_secret: Option<&str>,
    code: &str,
    redirect_uri: &str,
    verifier: &str,
    scope: Option<&str>,
) -> Result<(), String> {
    let mut pairs = vec![
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", redirect_uri),
        ("client_id", client_id),
        ("code_verifier", verifier),
    ];
    let secret;
    if let Some(secret_value) = client_secret {
        secret = secret_value.to_string();
        pairs.push(("client_secret", secret.as_str()));
    }
    let answer = token_post(caps, server, &meta.token_endpoint, &pairs)?;
    store_issuance(
        store,
        &answer,
        client_id,
        client_secret,
        &meta.token_endpoint,
        scope,
    )?;
    // The grant now covers the stepped-up scopes: forget the pending
    // memory (a best-effort delete; absence is the common case).
    let _ = store.caps.credentials_delete(&store.pending_key());
    Ok(())
}

/// Use the refresh token to mint a new access token. `Ok(true)` means
/// the store holds fresh tokens; `Ok(false)` means there is nothing
/// to refresh with. An `invalid_grant` purges the dead tokens (the
/// grant died server-side; keeping it would loop).
pub fn refresh(
    caps: Arc<lca_tools::Capabilities>,
    server: &HttpServerConfig,
) -> Result<bool, String> {
    let store = TokenStore::new(caps.clone(), server);
    let Some(stored) = store.load()? else {
        return Ok(false);
    };
    let Some(refresh_token) = stored.refresh_token.clone() else {
        return Ok(false);
    };
    let mut pairs = vec![
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token.as_str()),
        ("client_id", stored.client_id.as_str()),
    ];
    let secret;
    if let Some(secret_value) = stored.client_secret.clone() {
        secret = secret_value;
        pairs.push(("client_secret", secret.as_str()));
    }
    match token_post(caps, server, &stored.token_url, &pairs) {
        Ok(answer) => {
            store_issuance(
                &store,
                &answer,
                &stored.client_id,
                stored.client_secret.as_deref(),
                &stored.token_url,
                stored.scope.as_deref(),
            )?;
            Ok(true)
        }
        Err(detail) if detail.contains("invalid_grant") => {
            let _ = store.clear();
            Ok(false)
        }
        Err(detail) => Err(detail),
    }
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
            let _ = refresh(self.caps.clone(), &self.server)?;
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
        let scope = headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("www-authenticate"))
            .map(|(_, value)| parse_www_authenticate(value))
            .and_then(|challenge| challenge.get("scope").cloned());
        if let Some(scope) = &scope {
            let store = TokenStore::new(self.caps.clone(), &self.server);
            // A best-effort memory: the report must survive even when
            // the store write cannot.
            let _ = store.set_pending(scope);
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
                        if refresh(self.caps.clone(), &self.server)? {
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
            return lca_wire_mcp::select_response(&lca_wire_mcp::parse_sse_messages(text), id);
        }
        let message: serde_json::Value = serde_json::from_str(text)
            .map_err(|err| format!("MCP server answered bad JSON: {err}"))?;
        lca_wire_mcp::select_response(std::slice::from_ref(&message), id)
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
