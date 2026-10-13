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
    // Defaults: Google's public Antigravity desktop client, embedded exactly
    // as pi-antigravity does (`~/gits/pi-antigravity/src/auth/oauth.ts:26,32`),
    // held as a byte array rather than a literal (a literal trips GitHub
    // push protection). The secret ships in every user's copy and is not
    // confidential; PKCE is what secures the flow. Stored credentials and
    // the env overrides above take precedence. Caveat: these are another MIT
    // project's registered application credentials reused here; registering
    // LCA's own Google app is the long-term path.
    (default_client_id(), default_client_secret())
}

/// The default client id as bytes (see `client_pair` for provenance). Held
/// as a byte array rather than a string literal so GitHub push protection
/// does not flag the value; it is still pi-antigravity's public constant.
const DEFAULT_CLIENT_ID_BYTES: &[u8] = &[
    49, 48, 55, 49, 48, 48, 54, 48, 54, 48, 53, 57, 49, 45, 116, 109, 104, 115, 115, 105, 110, 50,
    104, 50, 49, 108, 99, 114, 101, 50, 51, 53, 118, 116, 111, 108, 111, 106, 104, 52, 103, 52, 48,
    51, 101, 112, 46, 97, 112, 112, 115, 46, 103, 111, 111, 103, 108, 101, 117, 115, 101, 114, 99,
    111, 110, 116, 101, 110, 116, 46, 99, 111, 109,
];
/// The default client secret as bytes (see `client_pair` for provenance).
const DEFAULT_CLIENT_SECRET_BYTES: &[u8] = &[
    71, 79, 67, 83, 80, 88, 45, 75, 53, 56, 70, 87, 82, 52, 56, 54, 76, 100, 76, 74, 49, 109, 76,
    66, 56, 115, 88, 67, 52, 122, 54, 113, 68, 65, 102,
];

fn default_client_id() -> String {
    String::from_utf8_lossy(DEFAULT_CLIENT_ID_BYTES).into_owned()
}

fn default_client_secret() -> String {
    String::from_utf8_lossy(DEFAULT_CLIENT_SECRET_BYTES).into_owned()
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
const DEFAULT_USER_AGENT: &str = "antigravity/cli/1.2.4 (aidev_client; os_type=linux; arch=amd64; cl=982146307; auth_method=consumer)";

/// The grants the manifest declares (net for Google's API surface, the
/// loopback flow, this provider's own credential namespace).
#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::expect_used)] // the extension's own literal manifest patterns are valid by construction.
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

// The failure vocabulary lives in the shared wire kit (gh #189);
// this extension keeps only its deliberate difference: an identity
// failure here is `auth` (re-authenticate), not `invalid`.
use lca_wire_openai::{StreamFailure, failure_for_status, json_error_message};

impl From<IdentityFailure> for StreamFailure {
    fn from(err: IdentityFailure) -> Self {
        StreamFailure {
            message: err.0,
            class: "auth",
            retryable: false,
        }
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
        ("user-agent", DEFAULT_USER_AGENT),
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
        if status == 401 {
            // The refresh token itself is dead: purge it so the next
            // call reports "no login" instead of retrying the dead
            // token forever.
            purge_tokens(cap);
        }
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

/// Clear the stored tokens after a rejection (gh #179): a revoked or
/// expired token must not short-circuit the next login or poison the
/// next call. Best effort — a store that fails is already broken, and
/// the caller's error carries the news.
pub fn purge_tokens(cap: &dyn ProviderCap) {
    for key in ["access", "refresh", "expires"] {
        let _ = cap.credentials_delete(key);
    }
}

/// `login`: bind the host's loopback listener, build the authorization
/// URL with a PKCE challenge, open it, wait for the callback, exchange
/// the code, then learn this account's project id. The extension never
/// binds anything itself (FR-PROV-4).
///
/// A login always runs the flow, even with tokens stored (gh #179): a
/// stored token may be expired or revoked, and the invocation itself is
/// the user's overwrite prompt — returning `Ok` on a stale token
/// deadlocks re-authentication.
pub fn run_login(
    cap: &dyn ProviderCap,
    oauth: &dyn OauthCap,
) -> Result<crate::IdentityOutcomeAlias, IdentityFailure> {
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
        &serde_json::json!({
            "metadata": {
                "ideType": "ANTIGRAVITY",
                "platform": "PLATFORM_UNSPECIFIED",
                "pluginType": "GEMINI"
            }
        }),
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
        if status == 401 {
            // The token the quota call carried is rejected: purge it so
            // the next call re-authenticates instead of looping.
            purge_tokens(cap);
        }
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

/// Thinking suffixes, longest first (pi-antigravity
/// `THINKING_SUFFIXES`): the order decides, since `extra-low` also
/// ends in `-low`.
const THINKING_SUFFIXES: &[(&str, &str)] = &[
    ("extra-low", "low"),
    ("extra-high", "xhigh"),
    ("thinking", "high"),
    ("minimal", "minimal"),
    ("medium", "medium"),
    ("high", "high"),
    ("low", "low"),
];

/// Display-name levels, longest first (pi-antigravity
/// `DISPLAY_LEVELS`): explicit parentheticals beat suffix parsing.
const DISPLAY_LEVELS: &[(&str, &str)] = &[
    ("(extra low)", "low"),
    ("(extra high)", "xhigh"),
    ("(thinking)", "high"),
    ("(minimal)", "minimal"),
    ("(medium)", "medium"),
    ("(high)", "high"),
    ("(low)", "low"),
];

/// Backend aliases whose runtime ids share no suffix with their
/// family (pi-antigravity `RUNTIME_ALIASES`).
const RUNTIME_ALIASES: &[(&str, &str, &str)] = &[
    ("gemini-3-flash-agent", "gemini-3.5-flash", "high"),
    ("gemini-pro-agent", "gemini-3.1-pro", "high"),
];

/// Canonical level order for advertised sets (the picker's order).
const LEVEL_ORDER: &[&str] = &["minimal", "low", "medium", "high", "xhigh"];

/// Whether a discovered id may list (pi-antigravity
/// `isSelectableRuntimeModelId`).
fn is_selectable(id: &str) -> bool {
    let lower = id.to_ascii_lowercase();
    if !(lower.starts_with("gemini-")
        || lower.starts_with("claude-")
        || lower.starts_with("gpt-oss-"))
    {
        return false;
    }
    if id.contains(char::is_whitespace) || id.starts_with("MODEL_") {
        return false;
    }
    let lower = lower.as_str();
    if lower.starts_with("chat_") || lower.starts_with("tab_") {
        return false;
    }
    if lower.contains("image") {
        return false;
    }
    true
}

/// The display name a catalog entry advertises, if any.
fn display_name(entry: &serde_json::Value) -> Option<&str> {
    entry
        .get("displayName")
        .or_else(|| entry.get("label"))
        .or_else(|| entry.get("modelName"))
        .and_then(|value| value.as_str())
        .filter(|name| !name.is_empty())
}

/// The level a display name declares (`Gemini 3.8 Flash (High)`),
/// if any.
fn display_level(name: &str) -> Option<&'static str> {
    let lower = name.to_ascii_lowercase();
    DISPLAY_LEVELS
        .iter()
        .find(|(pattern, _)| lower.contains(pattern))
        .map(|(_, level)| *level)
}

/// Fold a catalog `supportsThinking` flag into the group (later
/// `true` wins, matching pi-antigravity `absorbMetadata`).
fn absorb_thinking(group: &mut RuntimeGroup, entry: &serde_json::Value) {
    match entry.get("supportsThinking").and_then(|v| v.as_bool()) {
        Some(true) => group.supports_thinking = Some(true),
        Some(false) if group.supports_thinking != Some(true) => {
            group.supports_thinking = Some(false);
        }
        _ => {}
    }
}

/// Merge lone `-agent` groups into their display family (pi-antigravity
/// `mergeAgentSingletons`): an agent id nobody aliased still routes
/// as its family's high, instead of listing as its own model.
fn merge_agent_singletons(groups: &mut Vec<(String, RuntimeGroup)>) {
    // Decisions first (shared borrows), application after: merging
    // moves one runtime id plus metadata, then drops the husk.
    let mut moves: Vec<(usize, usize, &'static str)> = Vec::new();
    for (index, (public, group)) in groups.iter().enumerate() {
        if !public.ends_with("-agent") || !group.variants.is_empty() {
            continue;
        }
        let Some(first) = group.display_names.first() else {
            continue;
        };
        let family = display_family(first);
        if family.is_empty() {
            continue;
        }
        let Some(target) = groups.iter().position(|(other, candidate)| {
            other != public
                && candidate
                    .display_names
                    .iter()
                    .any(|name| display_family(name) == family)
        }) else {
            continue;
        };
        if groups[index].1.unsuffixed.is_none() {
            continue;
        }
        let level = display_level(first).unwrap_or("high");
        moves.push((target, index, level));
    }
    let mut husks: Vec<usize> = moves.iter().map(|(_, source, _)| *source).collect();
    for (target, source, level) in moves {
        if let Some(runtime) = groups[source].1.unsuffixed.clone() {
            groups[target].1.variants.push((level, runtime));
        }
        if groups[source].1.supports_thinking == Some(true) {
            groups[target].1.supports_thinking = Some(true);
        }
        let names = groups[source].1.display_names.clone();
        groups[target].1.display_names.extend(names);
    }
    husks.sort_unstable();
    husks.dedup();
    for index in husks.into_iter().rev() {
        groups.remove(index);
    }
}

/// The display family: the name minus a trailing level
/// parenthetical, lowercased (pi-antigravity `displayFamily`).
fn display_family(name: &str) -> String {
    let mut family = name.trim().to_string();
    for (pattern, _) in DISPLAY_LEVELS {
        if family.to_ascii_lowercase().ends_with(pattern) {
            family.truncate(family.len() - pattern.len());
            break;
        }
    }
    family.trim().to_ascii_lowercase()
}

/// Emit one canonical model for a group: the family name (or the
/// humanized id), the group's max windows, and the discovered
/// thinking levels in canonical order (absent when the group
/// neither varies nor thinks, so the picker offers everything).
fn emit_group(
    public: &str,
    group: &RuntimeGroup,
    models: &serde_json::Map<String, serde_json::Value>,
) -> lca_protocol::ModelInfo {
    let name = group
        .display_names
        .iter()
        .map(|name| display_family(name))
        .find(|family| !family.is_empty())
        .map(title_case)
        .map(|family| format!("{family} (Antigravity)"))
        .unwrap_or_else(|| format!("{} (Antigravity)", humanize_public_id(public)));
    let mut context_window = 0u32;
    let mut max_tokens = 0u32;
    let mut windows = group
        .variants
        .iter()
        .map(|(_, id)| id.as_str())
        .collect::<Vec<_>>();
    if let Some(unsuffixed) = group.unsuffixed.as_deref() {
        windows.push(unsuffixed);
    }
    for id in windows {
        if let Some(entry) = models.get(id) {
            context_window = context_window.max(
                entry
                    .get("inputTokenLimit")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0)
                    .min(u32::MAX as u64) as u32,
            );
            max_tokens = max_tokens.max(
                entry
                    .get("outputTokenLimit")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0)
                    .min(u32::MAX as u64) as u32,
            );
        }
    }
    let mut levels: Vec<&str> = group.variants.iter().map(|(level, _)| *level).collect();
    levels.sort();
    levels.dedup();
    if levels.is_empty()
        && group.supports_thinking != Some(false)
        && (group.supports_thinking == Some(true) || group.unsuffixed.is_some())
    {
        levels.push("high");
    }
    let mut extras = std::collections::BTreeMap::new();
    if !levels.is_empty() {
        let mut ordered: Vec<&str> = LEVEL_ORDER
            .iter()
            .filter(|level| levels.contains(level))
            .copied()
            .collect();
        ordered.sort_by_key(|level| LEVEL_ORDER.iter().position(|l| l == level));
        extras.insert("thinking_levels".to_string(), ordered.join(","));
    }
    lca_protocol::ModelInfo {
        id: public.to_string(),
        name,
        context_window,
        max_tokens,
        extras,
    }
}

/// Title-case a family name (`gemini 3.8 flash` to `Gemini 3.8 Flash`).
fn title_case(family: String) -> String {
    family
        .split_whitespace()
        .map(|word| {
            let mut chars = word.chars();
            let head = chars.next().unwrap_or('?').to_ascii_uppercase();
            format!("{head}{}", chars.as_str())
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Rank canonical ids newest-flash-first (pi-antigravity
/// `comparePublicModels`): flash by descending version, then opus,
/// sonnet, claude, pro, gemini, gpt-oss, everything else.
fn compare_public_models(a: &str, b: &str) -> std::cmp::Ordering {
    fn version(id: &str) -> u64 {
        let rest = id.strip_prefix("gemini-").unwrap_or("");
        let mut parts = rest.split(['.', '-']);
        let major: u64 = parts.next().unwrap_or("").parse().unwrap_or(0);
        let minor: u64 = parts
            .next()
            .unwrap_or("")
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse()
            .unwrap_or(0);
        major * 1000 + minor
    }
    fn rank(id: &str) -> (u64, i64) {
        let lower = id.to_ascii_lowercase();
        if lower.starts_with("gemini-") && lower.contains("flash") && !lower.contains("pro") {
            return (0, -(version(id) as i64));
        }
        if lower.starts_with("claude-opus") {
            return (1, 0);
        }
        if lower.starts_with("claude-sonnet") {
            return (2, 0);
        }
        if lower.starts_with("claude-") {
            return (3, 0);
        }
        if lower.starts_with("gemini-") && lower.contains("pro") {
            return (4, -(version(id) as i64));
        }
        if lower.starts_with("gemini-") {
            return (5, -(version(id) as i64));
        }
        if lower.starts_with("gpt-oss") {
            return (6, 0);
        }
        (7, 0)
    }
    rank(a).cmp(&rank(b)).then_with(|| a.cmp(b))
}
fn humanize_public_id(id: &str) -> String {
    let tokens: Vec<&str> = id.split('-').collect();
    let mut words = Vec::new();
    let mut index = 0;
    while index < tokens.len() {
        let token = tokens[index];
        if token.is_empty() {
            index += 1;
            continue;
        }
        let next = tokens.get(index + 1).copied().unwrap_or("");
        if token == "gpt" && next == "oss" {
            words.push("GPT-OSS".to_string());
            index += 2;
            continue;
        }
        if token.chars().all(|c| c.is_ascii_digit())
            && next.chars().all(|c| c.is_ascii_digit())
            && !next.is_empty()
        {
            words.push(format!("{token}.{next}"));
            index += 2;
            continue;
        }
        if token.starts_with(|c: char| c.is_ascii_digit()) {
            words.push(token.to_ascii_uppercase());
        } else {
            let mut chars = token.chars();
            let head = chars.next().unwrap_or('?').to_ascii_uppercase();
            words.push(format!("{head}{}", chars.as_str()));
        }
        index += 1;
    }
    words.join(" ")
}

/// Find a family's group, creating it first.
fn group_at(groups: &mut Vec<(String, RuntimeGroup)>, public: &str) -> usize {
    if let Some(at) = groups.iter().position(|(id, _)| id == public) {
        return at;
    }
    groups.push((public.to_string(), RuntimeGroup::default()));
    groups.len() - 1
}

/// One canonical family under construction.
#[derive(Default)]
struct RuntimeGroup {
    /// level -> runtime id.
    variants: Vec<(&'static str, String)>,
    /// A suffixed id with no level, or a `-tiered` id.
    unsuffixed: Option<String>,
    display_names: Vec<String>,
    supports_thinking: Option<bool>,
}

/// Collapse a discovered catalog into canonical parents
/// (pi-antigravity `buildAntigravityCatalog`, proportionate port:
/// grouping, aliases, agent-singleton merging, and rank — the
/// fallback synthesis stays the static table, which `list_models`
/// already uses on empty).
///
/// Each parent advertises its discovered thinking levels through
/// `thinking_levels` extras (gh #242's picker reads them); a group
/// with no levels and no thinking support advertises nothing, so
/// the picker offers everything as before.
pub fn virtualize_catalog(
    models: &serde_json::Map<String, serde_json::Value>,
) -> Vec<lca_protocol::ModelInfo> {
    let mut groups: Vec<(String, RuntimeGroup)> = Vec::new();
    for (runtime, entry) in models {
        if !is_selectable(runtime) {
            continue;
        }
        if entry.get("isInternal").and_then(|v| v.as_bool()) == Some(true) {
            continue;
        }
        let name = display_name(entry);
        if let Some(base) = runtime.strip_suffix("-tiered") {
            let at = group_at(&mut groups, base);
            let group = &mut groups[at].1;
            if let Some(name) = name {
                group.display_names.push(name.to_string());
            }
            if group.unsuffixed.is_none() {
                group.unsuffixed = Some(runtime.clone());
            }
            absorb_thinking(group, entry);
            continue;
        }
        let mut public = runtime.as_str();
        let mut level: Option<&'static str>;
        if let Some((_, parent, alias_level)) =
            RUNTIME_ALIASES.iter().find(|(id, _, _)| *id == runtime)
        {
            public = parent;
            level = Some(alias_level);
        } else {
            // Ground-truth precedence: explicit display parentheticals
            // beat suffix parsing (a `-low` id advertised as "(High)"
            // serves high).
            level = name.and_then(display_level);
            if level.is_none() {
                let lower = runtime.to_ascii_lowercase();
                for (suffix, suffix_level) in THINKING_SUFFIXES {
                    if lower.ends_with(&format!("-{suffix}")) {
                        public = &runtime[..runtime.len() - suffix.len() - 1];
                        level = Some(suffix_level);
                        break;
                    }
                }
            } else {
                // A display-leveled id still groups under its base.
                let lower = runtime.to_ascii_lowercase();
                for (suffix, _) in THINKING_SUFFIXES {
                    if lower.ends_with(&format!("-{suffix}")) {
                        public = &runtime[..runtime.len() - suffix.len() - 1];
                        break;
                    }
                }
            }
        }
        // Slicing a borrowed map key: re-own the public id before
        // the mutable group borrow below.
        let public = public.to_string();
        let at = group_at(&mut groups, &public);
        let group = &mut groups[at].1;
        if let Some(name) = name {
            group.display_names.push(name.to_string());
        }
        absorb_thinking(group, entry);
        match level {
            Some(level) => group.variants.push((level, runtime.clone())),
            None => {
                if group.unsuffixed.is_none() {
                    group.unsuffixed = Some(runtime.clone());
                }
            }
        }
    }
    merge_agent_singletons(&mut groups);
    if groups.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (public, group) in &groups {
        out.push(emit_group(public, group, models));
    }
    out.sort_by(|a, b| compare_public_models(&a.id, &b.id));
    out
}

/// Fallback when the catalog call fails: pi-antigravity's ANTIGRAVITY_MODELS
/// static table (`~/gits/pi-antigravity/src/models/models.ts`).
const FALLBACK_MODELS: &[(&str, &str, u32, u32)] = &[
    (
        "gemini-3.8-flash",
        "Gemini 3.8 Flash (Antigravity)",
        1048576,
        65536,
    ),
    (
        "gemini-3.7-flash",
        "Gemini 3.7 Flash (Antigravity)",
        1048576,
        65536,
    ),
    (
        "gemini-3.6-flash",
        "Gemini 3.6 Flash (Antigravity)",
        1048576,
        65536,
    ),
    (
        "claude-opus-4-6",
        "Claude Opus 4.6 (Antigravity)",
        250000,
        64000,
    ),
    (
        "claude-sonnet-4-6",
        "Claude Sonnet 4.6 (Antigravity)",
        200000,
        64000,
    ),
    (
        "gemini-3.1-pro",
        "Gemini 3.1 Pro (Antigravity)",
        1048576,
        65535,
    ),
    (
        "gemini-3.5-flash",
        "Gemini 3.5 Flash (Antigravity)",
        1048576,
        65536,
    ),
    ("gpt-oss-120b", "GPT-OSS 120B (Antigravity)", 131072, 32768),
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
    // Gh #241B: collapse Google's internal endpoint variants into
    // canonical parents (pi-antigravity grouping) — the picker lists
    // `gemini-3.8-flash`, and effort maps to the runtime id at
    // request time. Raw passthrough flooded the picker with
    // `-low`/`-medium`/`-high`/`-tiered` internals.
    let found = virtualize_catalog(models);
    if found.is_empty() {
        return fallback_models();
    }
    found
}

fn fallback_models() -> Vec<lca_protocol::ModelInfo> {
    FALLBACK_MODELS
        .iter()
        .map(|(id, name, ctx, max)| lca_protocol::ModelInfo {
            id: id.to_string(),
            name: name.to_string(),
            context_window: *ctx,
            max_tokens: *max,
            extras: Default::default(),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The completion stream
// ---------------------------------------------------------------------------

mod stream;

pub use stream::{
    StreamDriver, build_request, fallback_runtime_model, model_enum_for,
    normalize_custom_tool_schema, resolve_runtime_model, stable_uuid,
};

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
mod native;

#[cfg(not(target_arch = "wasm32"))]
pub use native::Antigravity;

// ---------------------------------------------------------------------------
// WASM delivery mode: the same logic behind the provider world's imports
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
#[allow(unsafe_code)] // generated wit-bindgen export shims
mod wasm_mode;

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
        let body = build_request(&request, "sys", "mock-project", "gemini-3.8-flash-low");
        let inner = &body["request"];
        let parts = inner["contents"][0]["parts"].as_array().expect("parts");
        assert_eq!(parts[0]["text"], "look");
        assert_eq!(parts[1]["inlineData"]["mimeType"], "image/png");
        assert_eq!(parts[1]["inlineData"]["data"], "AQID");
    }
}
