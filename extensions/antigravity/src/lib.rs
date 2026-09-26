//! The Antigravity provider: Google's subscription-backed models over
//! the loopback OAuth flow, the OAuth reference provider (ADR-0004,
//! ADR-0009, `docs/providers/antigravity.md`).
//!
//! Dual-mode like every extension under `extensions/`: both halves share
//! this file and run over the `net`, `oauth`, and `credentials`
//! capability surface ([`ProviderCap`] + [`OauthCap`]), so neither mode
//! binds a port, opens a socket, or reads a credential file on its own -
//! the host's loopback listener does the first (FR-PROV-3/4) and the
//! namespace store does the last (FR-PERM-6/7).
//!
//! Endpoint shapes follow `~/gits/pi-antigravity`: Google's public
//! desktop OAuth client, PKCE `S256`, `v1internal:loadCodeAssist` for
//! the project id, `v1internal:streamGenerateContent?alt=sse` for the
//! stream, `v1internal:fetchAvailableModels` for the catalog, and
//! `v1internal:retrieveUserQuotaSummary` for usage. Endpoint URLs may be
//! overridden through this extension's own credential namespace (an
//! API-gateway or mirror setup), which is why the tests can point every
//! call at a loopback mock; a non-`*.googleapis.com` override still
//! needs its ad hoc `net` grant, so the override grants no reach.
//!
//! # Unsafe-code exemption
//!
//! The `wasm32` half carries generated `wit-bindgen` export shims, the
//! only `unsafe` in this crate; the module holds the allowance.

#![deny(unsafe_code)]

use lca_protocol::{OauthCap, ProviderCap};

/// The manifest this form ships with (single source for
/// [`manifest_grants`]; a test keeps them in step).
pub const MANIFEST: &str = include_str!("../extension.toml");

/// The OAuth client pair is configuration, not source: pi-antigravity
/// embeds Google's desktop-client credentials for *its own* app, which
/// were never this project's to republish (and push protection agrees).
/// The names match pi's own override variables, so a user who already
/// set them - or who registered their own desktop client - logs in with
/// zero extra work: the native `login` reads the environment and stores
/// the pair in this extension's namespace, and both delivery modes read
/// the stored pair first.
fn client_pair(cap: &dyn ProviderCap) -> (String, String) {
    let stored_id = cap.credentials_get("client_id").unwrap_or_default();
    let stored_secret = cap.credentials_get("client_secret").unwrap_or_default();
    if !stored_id.is_empty() {
        return (stored_id, stored_secret);
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let id = std::env::var("ANTIGRAVITY_CLIENT_ID").unwrap_or_default();
        let secret = std::env::var("ANTIGRAVITY_CLIENT_SECRET").unwrap_or_default();
        if !id.is_empty() {
            return (id, secret);
        }
    }
    (String::new(), String::new())
}

const SCOPES: &[&str] = &[
    "https://www.googleapis.com/auth/aicode",
    "https://www.googleapis.com/auth/cloud-platform",
    "https://www.googleapis.com/auth/userinfo.email",
    "https://www.googleapis.com/auth/userinfo.profile",
    "https://www.googleapis.com/auth/cclog",
    "https://www.googleapis.com/auth/experimentsandconfigs",
];

const DEFAULT_AUTH_ENDPOINT: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const DEFAULT_TOKEN_ENDPOINT: &str = "https://oauth2.googleapis.com/token";
const DEFAULT_REVOKE_ENDPOINT: &str = "https://oauth2.googleapis.com/revoke";
const DEFAULT_API_BASE: &str = "https://daily-cloudcode-pa.googleapis.com";

/// The grants the manifest declares (net for Google's API surface, the
/// loopback flow, this provider's own credential namespace).
#[cfg(not(target_arch = "wasm32"))]
pub fn manifest_grants() -> lca_tools::CapabilityGrants {
    lca_tools::CapabilityGrants {
        net: vec![
            lca_permissions::parse_net_pattern("generativelanguage.googleapis.com")
                .expect("the manifest's own host pattern parses"),
            lca_permissions::parse_net_pattern("*.googleapis.com")
                .expect("the manifest's own wildcard pattern parses"),
        ],
        oauth: Some(lca_permissions::OAuthSettings {
            redirect_path: "/callback".to_string(),
            // The capability catalog's flow default.
            timeout_seconds: 300,
        }),
        credentials: true,
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------
// Endpoint resolution: manifest defaults, credential-namespace overrides
// ---------------------------------------------------------------------------

fn endpoint(cap: &dyn ProviderCap, key: &str, default: &str) -> String {
    cap.credentials_get(key)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| default.to_string())
}

// ---------------------------------------------------------------------------
// PKCE (RFC 7636) and state: real randomness from the platform CSPRNG
// ---------------------------------------------------------------------------

/// URL-safe base64 without padding (RFC 4648 §5).
fn base64url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity((bytes.len() * 4).div_ceil(3));
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        let chars = [
            ALPHABET[(n >> 18) as usize & 63],
            ALPHABET[(n >> 12) as usize & 63],
            ALPHABET[(n >> 6) as usize & 63],
            ALPHABET[n as usize & 63],
        ];
        let keep = match chunk.len() {
            1 => 2,
            2 => 3,
            _ => 4,
        };
        for byte in chars.iter().take(keep) {
            out.push(*byte as char);
        }
    }
    out
}

fn random_bytes(len: usize) -> Result<Vec<u8>, IdentityFailure> {
    let mut bytes = vec![0u8; len];
    getrandom::fill(&mut bytes)
        .map_err(|err| IdentityFailure(format!("the platform CSPRNG failed: {err}")))?;
    Ok(bytes)
}

/// A PKCE verifier/state pair: the verifier never leaves the extension
/// except through the token exchange, and the state (independent of the
/// verifier, so a leaked callback URL cannot disclose it) must come
/// back on the redirect.
struct Pkce {
    verifier: String,
    challenge: String,
    state: String,
}

fn pkce() -> Result<Pkce, IdentityFailure> {
    let verifier = base64url(&random_bytes(32)?);
    let digest = {
        use sha2::Digest;
        sha2::Sha256::digest(verifier.as_bytes())
    };
    Ok(Pkce {
        challenge: base64url(&digest),
        verifier,
        state: base64url(&random_bytes(32)?),
    })
}

/// An identity operation failed; the string is shown to the user
/// (ADR-0012's `failed` case).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityFailure(pub String);

impl From<StreamFailure> for IdentityFailure {
    fn from(err: StreamFailure) -> Self {
        IdentityFailure(err.message)
    }
}

// ---------------------------------------------------------------------------
// Shared HTTP over the capability surface
// ---------------------------------------------------------------------------

/// How a provider call failed, in the vocabulary the core's
/// `ProviderError` speaks (`docs/headless.md`'s classes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamFailure {
    /// Human-readable message.
    pub message: String,
    /// Class for the headless envelope.
    pub class: &'static str,
    /// Whether a retry could help (FR-CORE-6).
    pub retryable: bool,
}

impl From<lca_protocol::CapabilityError> for StreamFailure {
    fn from(err: lca_protocol::CapabilityError) -> Self {
        use lca_protocol::CapabilityError as E;
        let (class, retryable) = match &err {
            E::Permission(_) | E::NotGranted(_) | E::NotFound(_) | E::Invalid(_) => {
                ("invalid", false)
            }
            E::Io(_) | E::Timeout(_) => ("transport", true),
        };
        StreamFailure {
            message: err.to_string(),
            class,
            retryable,
        }
    }
}

impl From<IdentityFailure> for StreamFailure {
    fn from(err: IdentityFailure) -> Self {
        StreamFailure {
            message: err.0,
            class: "auth",
            retryable: false,
        }
    }
}

fn failure_for_status(status: u16, detail: &str) -> StreamFailure {
    let class = match status {
        401 | 403 => "auth",
        400..=499 => "invalid",
        _ => "transport",
    };
    StreamFailure {
        message: format!("provider returned HTTP {status}: {detail}"),
        class,
        retryable: status == 429 || status == 408 || (500..=599).contains(&status),
    }
}

/// POST once and read the whole response: the metadata calls are small
/// and request/response, unlike the stream.
fn post_json(
    cap: &dyn ProviderCap,
    url: &str,
    token: &str,
    body: &serde_json::Value,
) -> Result<(u16, String), StreamFailure> {
    let body_bytes = serde_json::to_vec(body).map_err(|err| StreamFailure {
        message: format!("cannot build request: {err}"),
        class: "invalid",
        retryable: false,
    })?;
    let bearer = format!("Bearer {token}");
    let headers = [
        ("content-type", "application/json"),
        ("authorization", bearer.as_str()),
        ("user-agent", "lca-antigravity/1.0"),
    ];
    let handle = cap.net_request("POST", url, &headers, Some(&body_bytes))?;
    let status = cap.net_response_status(handle)?;
    let mut collected = Vec::new();
    while let Some(chunk) = cap.net_read_body(handle, 64 * 1024)? {
        collected.extend_from_slice(&chunk);
        if collected.len() > 4 * 1024 * 1024 {
            break;
        }
    }
    let _ = cap.net_close_response(handle);
    Ok((status, String::from_utf8_lossy(&collected).into_owned()))
}

fn json_error_message(text: &str) -> String {
    let json: serde_json::Value = serde_json::from_str(text).unwrap_or_default();
    json.get("error")
        .and_then(|error| error.get("message"))
        .and_then(|message| message.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| {
            let text = text.trim();
            if text.is_empty() {
                "unknown error".to_string()
            } else {
                text.chars().take(200).collect()
            }
        })
}

// ---------------------------------------------------------------------------
// Credentials: tokens in this extension's own namespace
// ---------------------------------------------------------------------------

fn stored(cap: &dyn ProviderCap, key: &str) -> String {
    cap.credentials_get(key).unwrap_or_default()
}

fn now_epoch() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or(0)
}

/// The access token, refreshed first when it is within a minute of
/// expiry and a refresh token exists (FR-PROV-5).
fn access_token(cap: &dyn ProviderCap) -> Result<String, IdentityFailure> {
    let access = stored(cap, "access");
    if access.is_empty() {
        return Err(IdentityFailure(
            "no Antigravity login yet; run /login antigravity".to_string(),
        ));
    }
    let expires: u64 = stored(cap, "expires").parse().unwrap_or(0);
    if now_epoch() + 60 < expires {
        return Ok(access);
    }
    let refresh = stored(cap, "refresh");
    if refresh.is_empty() {
        return Err(IdentityFailure(
            "the stored token is expired and there is no refresh token; \
             run /login antigravity"
                .to_string(),
        ));
    }
    let url = endpoint(cap, "token_endpoint", DEFAULT_TOKEN_ENDPOINT);
    let (client_id, client_secret) = client_pair(cap);
    if client_id.is_empty() {
        return Err(IdentityFailure(
            "no OAuth client configured; set ANTIGRAVITY_CLIENT_ID and \
             ANTIGRAVITY_CLIENT_SECRET (the same names pi uses) and run \
             /login antigravity again"
                .to_string(),
        ));
    }
    let body = serde_json::json!({
        "client_id": client_id,
        "client_secret": client_secret,
        "refresh_token": refresh,
        "grant_type": "refresh_token",
    });
    let (status, text) = post_json(cap, &url, "", &body)?;
    if !(200..300).contains(&status) {
        return Err(IdentityFailure(format!(
            "token refresh failed: {}",
            json_error_message(&text)
        )));
    }
    let json: serde_json::Value =
        serde_json::from_str(&text).map_err(|err| IdentityFailure(format!("refresh: {err}")))?;
    let access = json
        .get("access_token")
        .and_then(|value| value.as_str())
        .ok_or_else(|| IdentityFailure("refresh returned no access token".to_string()))?
        .to_string();
    let expires_in = json
        .get("expires_in")
        .and_then(|v| v.as_u64())
        .unwrap_or(3600);
    cap.credentials_set("access", &access)
        .map_err(|err| IdentityFailure(format!("cannot store the token: {err}")))?;
    cap.credentials_set("expires", &(now_epoch() + expires_in).to_string())
        .map_err(|err| IdentityFailure(format!("cannot store the expiry: {err}")))?;
    if let Some(refresh) = json.get("refresh_token").and_then(|v| v.as_str()) {
        cap.credentials_set("refresh", refresh)
            .map_err(|err| IdentityFailure(format!("cannot store the refresh token: {err}")))?;
    }
    Ok(access)
}

// ---------------------------------------------------------------------------
// Identity (ADR-0012): the loopback login, a logout that revokes, usage
// ---------------------------------------------------------------------------

/// Extract the project id from a `loadCodeAssist` however deep the
/// vendor nests it: the field has moved between responses, and a login
/// that works but forgets the project fails every later call.
fn find_project_id(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::Object(map) => {
            for key in ["project", "projectId", "project_id"] {
                if let Some(found) = map.get(key).and_then(|v| v.as_str())
                    && !found.is_empty()
                {
                    return Some(found.to_string());
                }
            }
            for child in map.values() {
                if let Some(found) = find_project_id(child) {
                    return Some(found);
                }
            }
            None
        }
        serde_json::Value::Array(items) => items.iter().find_map(find_project_id),
        _ => None,
    }
}

/// `login`: bind the host's loopback listener, build the authorization
/// URL with a PKCE challenge, open it, wait for the callback, exchange
/// the code, then learn this account's project id. The extension never
/// binds anything itself (FR-PROV-4).
pub fn run_login(
    cap: &dyn ProviderCap,
    oauth: &dyn OauthCap,
) -> Result<crate::IdentityOutcomeAlias, IdentityFailure> {
    if !stored(cap, "access").is_empty() {
        return Ok(lca_protocol::IdentityOutcome::Ok);
    }
    let (client_id, client_secret) = client_pair(cap);
    if client_id.is_empty() {
        return Err(IdentityFailure(
            "no OAuth client configured; set ANTIGRAVITY_CLIENT_ID and \
             ANTIGRAVITY_CLIENT_SECRET (the same names pi uses) before \
             /login antigravity"
                .to_string(),
        ));
    }
    let pkce = pkce()?;
    let (redirect, flow) = oauth
        .oauth_begin("/callback")
        .map_err(|err| IdentityFailure(format!("cannot start the loopback flow: {err}")))?;
    let auth_url = {
        let mut url = format!(
            "{}?client_id={}&response_type=code&redirect_uri={}&code_challenge={}&\
             code_challenge_method=S256&state={}&access_type=offline&prompt=consent&scope={}",
            endpoint(cap, "auth_endpoint", DEFAULT_AUTH_ENDPOINT),
            percent(&client_id),
            percent(&redirect),
            pkce.challenge,
            pkce.state,
            percent(&SCOPES.join(" ")),
        );
        url.push_str(&format!("&nonce={}", base64url(&random_bytes(8)?)));
        url
    };
    // `oauth.open` is best effort (capability catalog): a machine with
    // no browser launcher still completes when the user reaches the
    // recorded URL another way - the host logged it in `oauth_opened` -
    // so a launch failure must not abort the flow.
    let _ = oauth.oauth_open(&auth_url);
    let callback = oauth.oauth_await(flow);
    let _ = oauth.oauth_end(flow);
    let callback = callback.map_err(|err| IdentityFailure(err.to_string()))?;
    let get = |key: &str| {
        callback
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.clone())
            .unwrap_or_default()
    };
    if get("state") != pkce.state {
        return Err(IdentityFailure(
            "OAuth state mismatch; the callback did not come from this flow".to_string(),
        ));
    }
    let code = get("code");
    if code.is_empty() {
        let problem = get("error_description");
        let problem = if problem.is_empty() {
            get("error")
        } else {
            problem
        };
        return Err(IdentityFailure(format!(
            "the authorization server returned no code: {problem}"
        )));
    }

    let token_url = endpoint(cap, "token_endpoint", DEFAULT_TOKEN_ENDPOINT);
    let body = serde_json::json!({
        "client_id": client_id,
        "client_secret": client_secret,
        "code": code,
        "grant_type": "authorization_code",
        "redirect_uri": redirect,
        "code_verifier": pkce.verifier,
    });
    let (status, text) = post_json(cap, &token_url, "", &body)?;
    if !(200..300).contains(&status) {
        return Err(IdentityFailure(format!(
            "token exchange failed: {}",
            json_error_message(&text)
        )));
    }
    let json: serde_json::Value = serde_json::from_str(&text)
        .map_err(|err| IdentityFailure(format!("token reply: {err}")))?;
    let access = json
        .get("access_token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| IdentityFailure("no access token in the reply".to_string()))?
        .to_string();
    let refresh = json
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            IdentityFailure(
                "no refresh token received; re-run /login and allow offline access".to_string(),
            )
        })?
        .to_string();
    let expires_in = json
        .get("expires_in")
        .and_then(|v| v.as_u64())
        .unwrap_or(3600);

    // The project id comes from the code-assist handshake pi performs
    // during login; without it the metadata calls have no project.
    let api_base = endpoint(cap, "api_base", DEFAULT_API_BASE);
    let (status, assist) = post_json(
        cap,
        &format!(
            "{}/v1internal:loadCodeAssist",
            api_base.trim_end_matches('/')
        ),
        &access,
        &serde_json::json!({ "metadata": { "ideType": "ANTIGRAVITY" } }),
    )?;
    let project = if (200..300).contains(&status) {
        find_project_id(&serde_json::from_str(&assist).unwrap_or_default()).unwrap_or_default()
    } else {
        // A handshake failure leaves no project; the catalog call
        // carries an empty one and reports the real error.
        String::new()
    };

    for (key, value) in [
        ("access", access.as_str()),
        ("refresh", refresh.as_str()),
        ("expires", (now_epoch() + expires_in).to_string().as_str()),
        ("project", project.as_str()),
        ("client_id", client_id.as_str()),
        ("client_secret", client_secret.as_str()),
    ] {
        cap.credentials_set(key, value)
            .map_err(|err| IdentityFailure(format!("cannot store {key}: {err}")))?;
    }
    Ok(lca_protocol::IdentityOutcome::Ok)
}

/// `logout`: revoke the access token server-side where the endpoint
/// supports it, then clear this namespace.
pub fn run_logout(cap: &dyn ProviderCap, oauth: &dyn OauthCap) -> lca_protocol::IdentityOutcome {
    let _ = oauth;
    let access = stored(cap, "access");
    if !access.is_empty() {
        let revoke = endpoint(cap, "revoke_endpoint", DEFAULT_REVOKE_ENDPOINT);
        let body = serde_json::json!({ "token": access });
        // Best effort: a revoke that fails still ends with a cleared
        // local namespace, which is what the user asked for.
        let _ = post_json(cap, &revoke, "", &body);
    }
    for key in [
        "access",
        "refresh",
        "expires",
        "project",
        "client_id",
        "client_secret",
    ] {
        if let Err(err) = cap.credentials_delete(key) {
            return lca_protocol::IdentityOutcome::Failed(format!("cannot clear {key}: {err}"));
        }
    }
    lca_protocol::IdentityOutcome::Ok
}

/// `usage`: the quota summary in the standard usage shape, with the raw
/// summary preserved in `extras` (a tier/quota picture does not decompose
/// into token counts, and inventing numbers would be worse than carrying
/// the real ones through).
pub fn run_usage(cap: &dyn ProviderCap) -> Result<lca_protocol::Usage, IdentityFailure> {
    let token = access_token(cap)?;
    let api_base = endpoint(cap, "api_base", DEFAULT_API_BASE);
    let (status, text) = post_json(
        cap,
        &format!(
            "{}/v1internal:retrieveUserQuotaSummary",
            api_base.trim_end_matches('/')
        ),
        &token,
        &serde_json::json!({}),
    )?;
    if !(200..300).contains(&status) {
        return Err(IdentityFailure(format!(
            "quota summary failed: {}",
            json_error_message(&text)
        )));
    }
    let mut extras = std::collections::BTreeMap::new();
    let compact: String = text
        .chars()
        .filter(|c| !c.is_whitespace())
        .take(2000)
        .collect();
    extras.insert("quota-summary".to_string(), compact);
    Ok(lca_protocol::Usage {
        input: 0,
        output: 0,
        cache_read: 0,
        cache_write: 0,
        cache_write_1h: 0,
        cost: 0.0,
        cost_input: 0.0,
        cost_cache_read: 0.0,
        cost_cache_write: 0.0,
        extras,
    })
}

// ---------------------------------------------------------------------------
// Model listing
// ---------------------------------------------------------------------------

/// Fallback when the catalog call fails: the ids pi's model table shows
/// as current. The picker falls back to the configured model anyway if
/// this is empty (FR-PROV-2).
const FALLBACK_MODELS: &[(&str, &str)] = &[
    ("gemini-2.5-pro", "Gemini 2.5 Pro"),
    ("gemini-2.5-flash", "Gemini 2.5 Flash"),
];

/// Models from `fetchAvailableModels`, falling back to the static pair.
pub fn list_models(cap: &dyn ProviderCap) -> Vec<lca_protocol::ModelInfo> {
    let Ok(token) = access_token(cap) else {
        return fallback_models();
    };
    let api_base = endpoint(cap, "api_base", DEFAULT_API_BASE);
    let project = stored(cap, "project");
    let Ok((status, text)) = post_json(
        cap,
        &format!(
            "{}/v1internal:fetchAvailableModels",
            api_base.trim_end_matches('/')
        ),
        &token,
        &serde_json::json!({ "project": project }),
    ) else {
        return fallback_models();
    };
    if !(200..300).contains(&status) {
        return fallback_models();
    }
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
        return fallback_models();
    };
    let Some(models) = json.get("models").and_then(|m| m.as_object()) else {
        return fallback_models();
    };
    let mut found: Vec<lca_protocol::ModelInfo> = models
        .iter()
        .map(|(id, entry)| {
            let name = entry
                .get("displayName")
                .and_then(|v| v.as_str())
                .unwrap_or(id.as_str())
                .to_string();
            lca_protocol::ModelInfo {
                id: id.clone(),
                name,
                context_window: entry
                    .get("inputTokenLimit")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0)
                    .min(u32::MAX as u64) as u32,
                max_tokens: entry
                    .get("outputTokenLimit")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0)
                    .min(u32::MAX as u64) as u32,
            }
        })
        .collect();
    if found.is_empty() {
        return fallback_models();
    }
    found.sort_by(|a, b| a.id.cmp(&b.id));
    found
}

fn fallback_models() -> Vec<lca_protocol::ModelInfo> {
    FALLBACK_MODELS
        .iter()
        .map(|(id, name)| lca_protocol::ModelInfo {
            id: id.to_string(),
            name: name.to_string(),
            context_window: 0,
            max_tokens: 0,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The completion stream
// ---------------------------------------------------------------------------

/// Minimal percent-encoding for a URL query value (redirect URLs and
/// scopes are the only things that pass through here).
fn percent(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' | b':' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// The request body: Gemini's shape (`contents` + `systemInstruction` +
/// `functionDeclarations`), following pi's `buildRequest`.
fn build_request(request: &lca_protocol::CompletionRequest, system: &str) -> serde_json::Value {
    let mut contents = Vec::new();
    // Tool results answer the call they belong to; remember names here
    // so the `functionResponse` part can address it.
    let call_names: std::collections::HashMap<&str, &str> = request
        .messages
        .iter()
        .flat_map(|message| message.tool_calls.iter())
        .map(|call| (call.call_id.as_str(), call.name.as_str()))
        .collect();
    for message in &request.messages {
        let role = match message.role {
            lca_protocol::MessageRole::Tool => "user",
            lca_protocol::MessageRole::Assistant => "model",
            _ => "user",
        };
        let mut parts = Vec::new();
        let text: String = message
            .content
            .iter()
            .filter_map(|block| match block {
                lca_protocol::ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        if message.role == lca_protocol::MessageRole::Tool {
            let name = message
                .tool_call_id
                .as_deref()
                .and_then(|id| call_names.get(id).copied())
                .unwrap_or("tool");
            parts.push(serde_json::json!({
                "functionResponse": {
                    "name": name,
                    "response": { "result": text },
                }
            }));
        } else {
            if !text.is_empty() {
                parts.push(serde_json::json!({ "text": text }));
            }
            for block in &message.content {
                if let lca_protocol::ContentBlock::Image { media_type, bytes } = block {
                    parts.push(serde_json::json!({
                        "inlineData": {
                            "mimeType": media_type,
                            "data": lca_protocol::base64_encode(bytes),
                        }
                    }));
                }
            }
            for call in &message.tool_calls {
                let args: serde_json::Value = serde_json::from_str(&call.arguments)
                    .unwrap_or_else(|_| serde_json::json!({ "arguments": call.arguments }));
                parts.push(serde_json::json!({
                    "functionCall": { "id": call.call_id, "name": call.name, "args": args }
                }));
            }
        }
        if parts.is_empty() {
            continue;
        }
        contents.push(serde_json::json!({ "role": role, "parts": parts }));
    }
    let tools = if request.tools.is_empty() {
        serde_json::Value::Null
    } else {
        let declarations: Vec<serde_json::Value> = request
            .tools
            .iter()
            .map(|tool| {
                serde_json::json!({
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.parameters,
                })
            })
            .collect();
        serde_json::json!([{ "functionDeclarations": declarations }])
    };
    let mut body = serde_json::json!({
        "contents": contents,
        "systemInstruction": {
            "role": "user",
            "parts": [{ "text": system }],
        },
    });
    if !tools.is_null() {
        body["tools"] = tools;
    }
    body
}

/// Decode one SSE frame of `streamGenerateContent` into typed events.
/// A `functionCall` part arrives whole, so it opens the call, carries
/// its arguments, and closes it in order (FR-PROV-7's shape).
fn handle_chunk(
    value: &serde_json::Value,
    open_calls: &mut Vec<String>,
    emit: &mut dyn FnMut(lca_protocol::StreamEvent) -> bool,
) -> bool {
    use lca_protocol::StreamEvent as E;
    if let Some(error) = value.get("error") {
        let message = error
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("vendor error")
            .to_string();
        return emit(E::Error {
            message,
            retryable: false,
        });
    }
    if let Some(candidate) = value
        .get("candidates")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("content"))
        .and_then(|c| c.get("parts"))
        .and_then(|p| p.as_array())
    {
        for part in candidate {
            if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                if part.get("thought").and_then(|t| t.as_bool()) == Some(true) {
                    if !emit(E::ReasoningDelta {
                        delta: text.to_string(),
                    }) {
                        return false;
                    }
                } else if !emit(E::TextDelta {
                    delta: text.to_string(),
                }) {
                    return false;
                }
            }
            if let Some(call) = part.get("functionCall") {
                let name = call
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("tool")
                    .to_string();
                let id = call
                    .get("id")
                    .and_then(|i| i.as_str())
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("ag-{}", open_calls.len()));
                if !emit(E::ToolCallStart {
                    call_id: id.clone(),
                    name,
                }) {
                    return false;
                }
                open_calls.push(id.clone());
                let args = call
                    .get("args")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({}));
                if !emit(E::ToolCallArgDelta {
                    call_id: id.clone(),
                    delta: args.to_string(),
                }) {
                    return false;
                }
                if !emit(E::ToolCallEnd { call_id: id }) {
                    return false;
                }
                open_calls.pop();
            }
        }
    }
    if let Some(usage) = value.get("usageMetadata") {
        let count = |key: &str| usage.get(key).and_then(|v| v.as_u64()).unwrap_or(0);
        let cache_read = count("cachedContentTokenCount");
        let prompt = count("promptTokenCount");
        let output = count("candidatesTokenCount") + count("thoughtsTokenCount");
        if prompt > 0 || output > 0 || cache_read > 0 {
            return emit(E::Usage {
                usage: lca_protocol::Usage {
                    input: prompt.saturating_sub(cache_read),
                    output,
                    cache_read,
                    cache_write: 0,
                    cache_write_1h: 0,
                    cost: 0.0,
                    cost_input: 0.0,
                    cost_cache_read: 0.0,
                    cost_cache_write: 0.0,
                    extras: Default::default(),
                },
            });
        }
    }
    true
}

/// A pull-based driver over one Antigravity streaming completion: the
/// sandboxed form drives it from the `completion-stream` resource's `next`,
/// so events leave as the host yields body chunks instead of after the whole
/// response (`docs/deferred_workplan.md` C1). The native form drains it in a
/// loop.
pub struct StreamDriver<'a> {
    cap: &'a dyn ProviderCap,
    handle: u32,
    buffer: String,
    open_calls: Vec<String>,
    pending: std::collections::VecDeque<lca_protocol::StreamEvent>,
    finished: bool,
}

impl<'a> StreamDriver<'a> {
    /// Authenticate, build the request, send it, and check the status.
    pub fn open(
        cap: &'a dyn ProviderCap,
        request: &lca_protocol::CompletionRequest,
    ) -> Result<StreamDriver<'a>, StreamFailure> {
        let token = access_token(cap).map_err(StreamFailure::from)?;
        let api_base = endpoint(cap, "api_base", DEFAULT_API_BASE);
        let system: String = request
            .messages
            .iter()
            .filter(|message| message.role == lca_protocol::MessageRole::System)
            .flat_map(|message| {
                message.content.iter().filter_map(|block| match block {
                    lca_protocol::ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
            })
            .collect::<Vec<_>>()
            .join("\n");
        let body = build_request(request, &system);
        let body_bytes = serde_json::to_vec(&body).map_err(|err| StreamFailure {
            message: format!("cannot build request: {err}"),
            class: "invalid",
            retryable: false,
        })?;
        let url = format!(
            "{}/v1internal:streamGenerateContent?alt=sse",
            api_base.trim_end_matches('/')
        );
        let bearer = format!("Bearer {token}");
        let headers = [
            ("content-type", "application/json"),
            ("authorization", bearer.as_str()),
            ("user-agent", "lca-antigravity/1.0"),
        ];
        let handle = cap.net_request("POST", &url, &headers, Some(&body_bytes))?;
        let status = cap.net_response_status(handle)?;
        if !(200..300).contains(&status) {
            let mut detail = Vec::new();
            while let Some(chunk) = cap.net_read_body(handle, 64 * 1024)? {
                detail.extend_from_slice(&chunk);
                if detail.len() > 1024 * 1024 {
                    break;
                }
            }
            let _ = cap.net_close_response(handle);
            let text = String::from_utf8_lossy(&detail);
            return Err(failure_for_status(status, &json_error_message(&text)));
        }
        Ok(StreamDriver {
            cap,
            handle,
            buffer: String::new(),
            open_calls: Vec::new(),
            pending: std::collections::VecDeque::new(),
            finished: false,
        })
    }

    /// The next typed event, reading more of the body when the frame buffer
    /// is empty; `None` at end of stream.
    pub fn next_event(&mut self) -> Option<Result<lca_protocol::StreamEvent, StreamFailure>> {
        loop {
            if let Some(event) = self.pending.pop_front() {
                return Some(Ok(event));
            }
            if self.finished {
                return None;
            }
            match self.cap.net_read_body(self.handle, 64 * 1024) {
                Ok(Some(chunk)) => {
                    self.buffer.push_str(&String::from_utf8_lossy(&chunk));
                    self.drain_frames();
                }
                Ok(None) => self.finished = true,
                Err(err) => {
                    self.finished = true;
                    return Some(Err(StreamFailure::from(err)));
                }
            }
        }
    }

    /// Decode every complete `\n\n`-terminated frame currently buffered.
    fn drain_frames(&mut self) {
        loop {
            let Some(end) = self.buffer.find("\n\n") else {
                return;
            };
            let frame: String = self.buffer.drain(..end + 2).collect();
            let mut events = Vec::new();
            for line in frame.lines() {
                let line = line.trim_end_matches('\r');
                let Some(payload) = line.strip_prefix("data:").map(str::trim) else {
                    continue;
                };
                if payload.is_empty() || payload == "[DONE]" {
                    continue;
                }
                let Ok(value) = serde_json::from_str::<serde_json::Value>(payload) else {
                    continue; // a malformed frame drops; the stream continues
                };
                handle_chunk(&value, &mut self.open_calls, &mut |event| {
                    events.push(event);
                    true
                });
            }
            self.pending.extend(events);
        }
    }
}

impl Drop for StreamDriver<'_> {
    fn drop(&mut self) {
        let _ = self.cap.net_close_response(self.handle);
    }
}

/// The whole completion call over capabilities: refresh if needed
/// (FR-PROV-5), then stream the SSE body chunk by chunk. `emit` returning
/// `false` stops the read (FR-CONC-3).
pub fn run_provider_stream(
    cap: &dyn ProviderCap,
    request: &lca_protocol::CompletionRequest,
    emit: &mut dyn FnMut(lca_protocol::StreamEvent) -> bool,
) -> Result<(), StreamFailure> {
    let mut driver = StreamDriver::open(cap, request)?;
    while let Some(event) = driver.next_event() {
        if !emit(event?) {
            break;
        }
    }
    Ok(())
}

/// The outcome alias identity returns on the success path.
pub type IdentityOutcomeAlias = lca_protocol::IdentityOutcome;

// ---------------------------------------------------------------------------
// Native delivery mode: the dispatch handle the registry holds
// ---------------------------------------------------------------------------

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use super::*;
    use std::sync::Arc;

    use lca_ext_abi::{DeliveryMode, DispatchFuture, ExtensionDispatch, World};
    use lca_protocol::{DispatchError, IdentityOutcome, ModelInfo};

    /// The native handle over the shared capability engine.
    pub struct Antigravity {
        cap: Arc<lca_tools::Capabilities>,
    }

    impl Antigravity {
        /// Build from the engine the manifest's grants live in.
        pub fn new(cap: Arc<lca_tools::Capabilities>) -> Antigravity {
            Antigravity { cap }
        }

        /// The engine, for tests that inspect recorded denials or the
        /// authorization URLs a login opened.
        pub fn capabilities(&self) -> &Arc<lca_tools::Capabilities> {
            &self.cap
        }
    }

    impl ExtensionDispatch for Antigravity {
        fn name(&self) -> &str {
            "antigravity"
        }

        fn delivery(&self) -> DeliveryMode {
            DeliveryMode::Native
        }

        fn worlds(&self) -> Vec<World> {
            vec![World::Provider]
        }

        fn interrupt(&self) {
            // A native call shares the caller's thread, so there is no epoch
            // to bump: flag the capability engine directly, and a blocked
            // `net` request polls its way out (FR-CONC-1, NFR-21).
            self.cap.cancel();
        }

        fn tool_specs(&self) -> Result<Vec<lca_protocol::ToolSpec>, DispatchError> {
            Err(DispatchError::MissingWorld {
                extension: "antigravity".to_string(),
                world: "tool",
            })
        }

        fn execute_tool<'a>(
            &'a self,
            _call: &'a lca_protocol::ToolCall,
        ) -> DispatchFuture<'a, Result<lca_protocol::ToolResult, DispatchError>> {
            Box::pin(std::future::ready(Err(DispatchError::MissingWorld {
                extension: "antigravity".to_string(),
                world: "tool",
            })))
        }

        fn command_specs(&self) -> Result<Vec<lca_protocol::CommandSpec>, DispatchError> {
            Ok(Vec::new())
        }

        fn invoke_command(
            &self,
            _name: &str,
            _argument: &str,
        ) -> Result<lca_protocol::CommandEffect, DispatchError> {
            Ok(lca_protocol::CommandEffect::None)
        }

        fn provider_models(&self) -> Result<Vec<ModelInfo>, DispatchError> {
            let cap = self.cap.clone();
            Ok(list_models(cap.as_ref()))
        }

        fn stream_completion<'a>(
            &'a self,
            request: lca_protocol::CompletionRequest,
            sink: &'a dyn lca_protocol::EventSink,
        ) -> DispatchFuture<'a, Result<(), DispatchError>> {
            let cap = self.cap.clone();
            Box::pin(async move {
                let result = lca_tools::bridge_stream(
                    move |bridge| {
                        run_provider_stream(cap.as_ref(), &request, &mut |event| bridge.push(event))
                    },
                    sink,
                )
                .await;
                match result {
                    Ok(()) => Ok(()),
                    Err(lca_tools::BridgeError::Work(failure)) => Err(DispatchError::Failed(
                        format!("antigravity: {}", failure.message),
                    )),
                    Err(lca_tools::BridgeError::Panicked) => Err(DispatchError::Failed(
                        "antigravity: the provider call panicked".to_string(),
                    )),
                }
            })
        }

        fn identity_login(
            &self,
        ) -> DispatchFuture<'static, Result<IdentityOutcome, DispatchError>> {
            let cap = self.cap.clone();
            Box::pin(async move {
                tokio::task::spawn_blocking({
                    let cap = cap.clone();
                    move || {
                        run_login(cap.as_ref(), cap.as_ref())
                            .map_err(|err| DispatchError::Failed(format!("antigravity: {}", err.0)))
                    }
                })
                .await
                .map_err(|_| DispatchError::Failed("antigravity: login panicked".into()))
                .and_then(std::convert::identity)
            })
        }

        fn identity_logout(
            &self,
        ) -> DispatchFuture<'static, Result<IdentityOutcome, DispatchError>> {
            let cap = self.cap.clone();
            Box::pin(async move {
                tokio::task::spawn_blocking(move || run_logout(cap.as_ref(), cap.as_ref()))
                    .await
                    .map_err(|_| DispatchError::Failed("antigravity: logout panicked".into()))
            })
        }

        fn identity_usage(
            &self,
        ) -> DispatchFuture<
            'static,
            Result<Result<lca_protocol::Usage, IdentityOutcome>, DispatchError>,
        > {
            let cap = self.cap.clone();
            Box::pin(async move {
                tokio::task::spawn_blocking(move || {
                    run_usage(cap.as_ref()).map_err(|err| IdentityOutcome::Failed(err.0))
                })
                .await
                .map_err(|_| DispatchError::Failed("antigravity: usage panicked".into()))
            })
        }

        fn on_pre_turn(&self) -> DispatchFuture<'static, Result<(), DispatchError>> {
            Box::pin(std::future::ready(Ok(())))
        }

        fn on_pre_tool_use<'a>(
            &'a self,
            _call: &'a lca_protocol::ToolCall,
        ) -> DispatchFuture<'a, Result<lca_protocol::HookAction, DispatchError>> {
            Box::pin(std::future::ready(Ok(lca_protocol::HookAction::Allow)))
        }

        fn on_post_tool_use<'a>(
            &'a self,
            _observation: &'a lca_protocol::PostToolObservation,
        ) -> DispatchFuture<'a, Result<(), DispatchError>> {
            Box::pin(std::future::ready(Ok(())))
        }

        fn on_post_turn_end<'a>(
            &'a self,
            _status: &'a str,
        ) -> DispatchFuture<'a, Result<(), DispatchError>> {
            Box::pin(std::future::ready(Ok(())))
        }

        fn on_attention_required<'a>(
            &'a self,
            _reason: &'a str,
        ) -> DispatchFuture<'a, Result<(), DispatchError>> {
            Box::pin(std::future::ready(Ok(())))
        }

        fn on_session_close(&self) -> DispatchFuture<'static, Result<(), DispatchError>> {
            Box::pin(std::future::ready(Ok(())))
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use native::Antigravity;

// ---------------------------------------------------------------------------
// WASM delivery mode: the same logic behind the provider world's imports
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
#[allow(unsafe_code)] // generated wit-bindgen export shims
mod wasm_mode {
    use super::*;
    use core::cell::RefCell;

    wit_bindgen::generate!({
        path: "../../wit",
        world: "provider",
        export_macro_name: "export_provider",
        with: {
            "lca:host/log@0.2.0": generate,
            "lca:host/net@0.2.0": generate,
            "lca:host/oauth@0.2.0": generate,
            "lca:host/credentials@0.2.0": generate,
        },
    });

    use exports::lca::ext::provider_completion::{
        CompletionStream, Guest as CompletionGuest, GuestCompletionStream, StreamEvent as WasmEvent,
    };
    use exports::lca::ext::provider_identity::{
        Guest as IdentityGuest, IdentityOutcome as WasmOutcome, TokenUsage,
    };
    use exports::lca::ext::provider_models::{Guest as ModelsGuest, ModelInfo as WasmModel};
    use lca::ext::types::{ExtraPair, Usage as WasmUsage};
    use lca::host::{credentials, net, oauth};

    use crate::{IdentityOutcomeAlias, run_login, run_logout, run_usage};

    fn map_net(err: net::Error) -> lca_protocol::CapabilityError {
        use lca_protocol::CapabilityError as E;
        match err {
            net::Error::Permission(d) => E::Permission(d),
            net::Error::NotGranted(d) => E::NotGranted(d),
            net::Error::Dns(d) => E::Io(d),
            net::Error::Tls(d) => E::Io(d),
            net::Error::Io(d) => E::Io(d),
            net::Error::Invalid(d) => E::Invalid(d),
        }
    }

    fn map_oauth(err: oauth::Error) -> lca_protocol::CapabilityError {
        use lca_protocol::CapabilityError as E;
        match err {
            oauth::Error::Permission(d) => E::Permission(d),
            oauth::Error::NotGranted(d) => E::NotGranted(d),
            oauth::Error::Timeout(d) => E::Timeout(d),
            oauth::Error::Io(d) => E::Io(d),
            oauth::Error::Invalid(d) => E::Invalid(d),
        }
    }

    /// The guest's capability view: host imports only.
    struct GuestCap;

    /// A `'static` view so the streaming driver can borrow the capability
    /// view for as long as its resource lives (a unit struct: free).
    static GUEST_CAP: GuestCap = GuestCap;

    impl ProviderCap for GuestCap {
        fn net_request(
            &self,
            method: &str,
            url: &str,
            headers: &[(&str, &str)],
            body: Option<&[u8]>,
        ) -> Result<u32, lca_protocol::CapabilityError> {
            net::request(
                method,
                url,
                &headers
                    .iter()
                    .map(|(key, value)| (key.to_string(), value.to_string()))
                    .collect::<Vec<_>>(),
                body,
            )
            .map_err(map_net)
        }

        fn net_response_status(&self, handle: u32) -> Result<u16, lca_protocol::CapabilityError> {
            net::response_status(handle).map_err(map_net)
        }

        fn net_read_body(
            &self,
            handle: u32,
            max: usize,
        ) -> Result<Option<Vec<u8>>, lca_protocol::CapabilityError> {
            net::read_body(handle, max as u64).map_err(map_net)
        }

        fn net_close_response(&self, handle: u32) -> Result<(), lca_protocol::CapabilityError> {
            net::close_response(handle).map_err(map_net)
        }

        fn credentials_get(&self, key: &str) -> Option<String> {
            credentials::get(key)
        }

        fn credentials_set(
            &self,
            key: &str,
            value: &str,
        ) -> Result<(), lca_protocol::CapabilityError> {
            credentials::set(key, value).map_err(|err| match err {
                credentials::Error::Permission(d) => lca_protocol::CapabilityError::Permission(d),
                credentials::Error::NotGranted(d) => lca_protocol::CapabilityError::NotGranted(d),
                credentials::Error::Io(d) => lca_protocol::CapabilityError::Io(d),
                credentials::Error::Invalid(d) => lca_protocol::CapabilityError::Invalid(d),
            })
        }

        fn credentials_delete(&self, key: &str) -> Result<(), lca_protocol::CapabilityError> {
            credentials::delete(key).map_err(|err| match err {
                credentials::Error::Permission(d) => lca_protocol::CapabilityError::Permission(d),
                credentials::Error::NotGranted(d) => lca_protocol::CapabilityError::NotGranted(d),
                credentials::Error::Io(d) => lca_protocol::CapabilityError::Io(d),
                credentials::Error::Invalid(d) => lca_protocol::CapabilityError::Invalid(d),
            })
        }
    }

    impl OauthCap for GuestCap {
        fn oauth_begin(
            &self,
            redirect_path: &str,
        ) -> Result<(String, u32), lca_protocol::CapabilityError> {
            oauth::begin(redirect_path).map_err(map_oauth)
        }

        fn oauth_open(&self, url: &str) -> Result<(), lca_protocol::CapabilityError> {
            oauth::open(url).map_err(map_oauth)
        }

        fn oauth_await(
            &self,
            handle: u32,
        ) -> Result<Vec<(String, String)>, lca_protocol::CapabilityError> {
            oauth::await_callback(handle).map_err(map_oauth)
        }

        fn oauth_end(&self, handle: u32) -> Result<(), lca_protocol::CapabilityError> {
            oauth::end_flow(handle).map_err(map_oauth)
        }
    }

    fn to_wit_usage(usage: &lca_protocol::Usage) -> WasmUsage {
        let mut extras: Vec<ExtraPair> = usage
            .extras
            .iter()
            .map(|(key, value)| ExtraPair {
                key: key.clone(),
                value: value.clone(),
            })
            .collect();
        for (key, value) in [
            ("cost_input", usage.cost_input),
            ("cost_cache_read", usage.cost_cache_read),
            ("cost_cache_write", usage.cost_cache_write),
        ] {
            if value != 0.0 {
                extras.push(ExtraPair {
                    key: key.to_string(),
                    value: value.to_string(),
                });
            }
        }
        WasmUsage {
            input: usage.input,
            output: usage.output,
            cache_read: usage.cache_read,
            cache_write: usage.cache_write,
            cache_write_hour: usage.cache_write_1h,
            cost: usage.cost,
            extras,
        }
    }

    fn to_wit_event(event: lca_protocol::StreamEvent) -> WasmEvent {
        use lca_protocol::StreamEvent as P;
        match event {
            P::TextDelta { delta } => WasmEvent::TextDelta(delta),
            P::ReasoningDelta { delta } => WasmEvent::ReasoningDelta(delta),
            P::ToolCallStart { call_id, name } => WasmEvent::ToolCallStart((call_id, name)),
            P::ToolCallArgDelta { call_id, delta } => WasmEvent::ToolCallArgDelta((call_id, delta)),
            P::ToolCallEnd { call_id } => WasmEvent::ToolCallEnd(call_id),
            P::Usage { usage } => WasmEvent::Usage(to_wit_usage(&usage)),
            P::Error { message, retryable } => WasmEvent::Error((message, retryable)),
            P::VendorEvent { kind, payload } => WasmEvent::VendorEvent((kind, payload.to_string())),
        }
    }

    fn to_wit_outcome(outcome: IdentityOutcomeAlias) -> WasmOutcome {
        match outcome {
            lca_protocol::IdentityOutcome::Ok => WasmOutcome::Ok,
            lca_protocol::IdentityOutcome::NotSupported => WasmOutcome::NotSupported,
            lca_protocol::IdentityOutcome::Failed(reason) => WasmOutcome::Failed(reason),
        }
    }

    pub struct AntigravityWasm;

    /// The `completion-stream` resource: a pull stream over the driver, so
    /// events leave as the host yields body chunks instead of after the whole
    /// response (C1).
    pub struct QueuedStream {
        driver: RefCell<Option<crate::StreamDriver<'static>>>,
    }

    impl GuestCompletionStream for QueuedStream {
        fn next(&self) -> Option<WasmEvent> {
            let mut slot = self.driver.borrow_mut();
            let driver = slot.as_mut()?;
            match driver.next_event() {
                None => {
                    *slot = None;
                    None
                }
                Some(Ok(event)) => Some(to_wit_event(event)),
                Some(Err(failure)) => {
                    *slot = None;
                    Some(WasmEvent::Error((failure.message, failure.retryable)))
                }
            }
        }
    }

    impl ModelsGuest for AntigravityWasm {
        fn list_models() -> Vec<WasmModel> {
            crate::list_models(&GuestCap)
                .into_iter()
                .map(|model| WasmModel {
                    id: model.id,
                    name: model.name,
                    context_window: model.context_window,
                    max_tokens: model.max_tokens,
                    extras: Vec::new(),
                })
                .collect()
        }
    }

    impl CompletionGuest for AntigravityWasm {
        type CompletionStream = QueuedStream;

        fn stream_completion(
            request: exports::lca::ext::provider_completion::CompletionRequest,
        ) -> Result<CompletionStream, String> {
            // The WIT record -> the protocol shape (the exact inverse of
            // the host's conversion; reasoning never crosses).
            let messages = request
                .messages
                .iter()
                .map(|message| lca_protocol::ChatMessage {
                    role: match message.role.as_str() {
                        "system" => lca_protocol::MessageRole::System,
                        "user" => lca_protocol::MessageRole::User,
                        "assistant" => lca_protocol::MessageRole::Assistant,
                        _ => lca_protocol::MessageRole::Tool,
                    },
                    content: message
                        .content
                        .iter()
                        .map(|block| match block {
                            lca::ext::types::ContentBlock::Text(text) => {
                                lca_protocol::ContentBlock::Text { text: text.clone() }
                            }
                            lca::ext::types::ContentBlock::Image((media_type, bytes)) => {
                                lca_protocol::ContentBlock::Image {
                                    media_type: media_type.clone(),
                                    bytes: bytes.clone(),
                                }
                            }
                        })
                        .collect(),
                    tool_calls: message
                        .tool_calls
                        .iter()
                        .map(|call| lca_protocol::ToolCall {
                            call_id: call.call_id.clone(),
                            name: call.name.clone(),
                            arguments: call.arguments.clone(),
                        })
                        .collect(),
                    tool_call_id: message.tool_call_id.clone(),
                    usage: None,
                    extras: Default::default(),
                })
                .collect();
            let tools = request
                .tools
                .iter()
                .map(|tool| lca_protocol::ToolSpec {
                    name: tool.name.clone(),
                    description: tool.description.clone(),
                    parameters: serde_json::from_str(&tool.parameters)
                        .unwrap_or_else(|_| serde_json::json!({ "type": "object" })),
                    extras: tool
                        .extras
                        .iter()
                        .map(|pair| (pair.key.clone(), pair.value.clone()))
                        .collect(),
                })
                .collect();
            let protocol_request = lca_protocol::CompletionRequest {
                messages,
                tools,
                model: request.model,
                stable_prefix: request.stable_prefix as usize,
                extras: request
                    .extras
                    .iter()
                    .map(|pair| (pair.key.clone(), pair.value.clone()))
                    .collect(),
            };
            let driver = crate::StreamDriver::open(&GUEST_CAP, &protocol_request)
                .map_err(|failure| failure.message)?;
            Ok(CompletionStream::new(QueuedStream {
                driver: RefCell::new(Some(driver)),
            }))
        }
    }

    impl IdentityGuest for AntigravityWasm {
        fn login() -> WasmOutcome {
            match run_login(&GuestCap, &GuestCap) {
                Ok(outcome) => to_wit_outcome(outcome),
                Err(err) => WasmOutcome::Failed(err.0),
            }
        }

        fn logout() -> WasmOutcome {
            to_wit_outcome(run_logout(&GuestCap, &GuestCap))
        }

        fn usage() -> Result<TokenUsage, WasmOutcome> {
            match run_usage(&GuestCap) {
                Ok(usage) => Ok(TokenUsage {
                    input: usage.input,
                    output: usage.output,
                    cache_read: usage.cache_read,
                    cache_write: usage.cache_write,
                    cache_write_hour: usage.cache_write_1h,
                    cost: usage.cost,
                    extras: usage
                        .extras
                        .iter()
                        .map(|(key, value)| ExtraPair {
                            key: key.clone(),
                            value: value.clone(),
                        })
                        .collect(),
                }),
                Err(err) => Err(WasmOutcome::Failed(err.0)),
            }
        }
    }

    export_provider!(AntigravityWasm);
}

#[cfg(test)]
mod tests {
    use super::*;

    // Verifies: ADR-0029 - an image maps to Gemini's `inlineData` part with
    // the base64 payload; text stays a `text` part.
    #[test]
    fn an_image_maps_to_an_inline_data_part() {
        let request = lca_protocol::CompletionRequest {
            messages: vec![lca_protocol::ChatMessage {
                role: lca_protocol::MessageRole::User,
                content: vec![
                    lca_protocol::ContentBlock::Text {
                        text: "look".to_string(),
                    },
                    lca_protocol::ContentBlock::Image {
                        media_type: "image/png".to_string(),
                        bytes: vec![1, 2, 3],
                    },
                ],
                tool_calls: Vec::new(),
                tool_call_id: None,
                usage: None,
                extras: Default::default(),
            }],
            tools: Vec::new(),
            model: "gemini".to_string(),
            stable_prefix: 0,
            extras: Default::default(),
        };
        let body = build_request(&request, "sys");
        let parts = body["contents"][0]["parts"].as_array().expect("parts");
        assert_eq!(parts[0]["text"], "look");
        assert_eq!(parts[1]["inlineData"]["mimeType"], "image/png");
        assert_eq!(parts[1]["inlineData"]["data"], "AQID");
    }
}
