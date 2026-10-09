//! The shared kit for subscription-gateway provider extensions (gh
//! #63): one OAuth PKCE login/refresh flow, parameterized by small
//! spec tables instead of branches. `extensions/codex` and
//! `extensions/grok` are thin specs over this; provider quirks live in
//! their tables, never in core (#157). The Responses wire protocol
//! this kit used to carry lives in `lca-wire-openai` now (gh #189).

pub mod wasm;

use lca_protocol::{OauthCap, ProviderCap};
use lca_wire_openai::StreamFailure;

/// Re-exported so extensions name the outcome in their signatures.
pub use lca_protocol::IdentityOutcome;

/// The kit's error helpers, still used by the token calls below.
pub use lca_wire_openai::json_error_message;

/// An identity operation failed; the string is shown to the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityFailure(pub String);

impl From<StreamFailure> for IdentityFailure {
    fn from(err: StreamFailure) -> Self {
        IdentityFailure(err.message)
    }
}

impl From<IdentityFailure> for StreamFailure {
    fn from(err: IdentityFailure) -> Self {
        StreamFailure {
            message: err.0,
            class: "invalid",
            retryable: false,
        }
    }
}

fn stored(cap: &dyn ProviderCap, key: &str) -> String {
    cap.credentials_get(key).unwrap_or_default()
}

/// Read an endpoint from the extension's own namespace first, so tests
/// (or a gateway) redirect every call; otherwise the compiled default.
fn endpoint(cap: &dyn ProviderCap, key: &str, default: &str) -> String {
    let value = stored(cap, key);
    if value.is_empty() {
        default.to_string()
    } else {
        value
    }
}

/// POST a form and read the whole response (the token calls are small
/// and request/response, unlike the stream).
fn post_form(
    cap: &dyn ProviderCap,
    url: &str,
    pairs: &[(&str, &str)],
) -> Result<(u16, String), IdentityFailure> {
    let body: String = pairs
        .iter()
        .map(|(key, value)| format!("{}={}", percent(key), percent(value)))
        .collect::<Vec<_>>()
        .join("&");
    let headers = [("content-type", "application/x-www-form-urlencoded")];
    let handle = cap
        .net_request("POST", url, &headers, Some(body.as_bytes()))
        .map_err(|err| IdentityFailure(err.to_string()))?;
    let status = cap
        .net_response_status(handle)
        .map_err(|err| IdentityFailure(err.to_string()))?;
    let mut text = Vec::new();
    while let Some(chunk) = cap
        .net_read_body(handle, 64 * 1024)
        .map_err(|err| IdentityFailure(err.to_string()))?
    {
        text.extend_from_slice(&chunk);
        if text.len() > 1024 * 1024 {
            break;
        }
    }
    let _ = cap.net_close_response(handle);
    Ok((status, String::from_utf8_lossy(&text).into_owned()))
}

/// POST one JSON object over capabilities and read the bounded
/// reply (gh #183, gh #185): the Anthropic and OpenRouter token
/// exchanges speak JSON where the older gateways speak form. Same
/// one-megabyte bound as the form POST below; the caller names the status.
pub fn post_json(
    cap: &dyn ProviderCap,
    url: &str,
    body: &serde_json::Value,
) -> Result<(u16, String), IdentityFailure> {
    post_json_with_headers(cap, url, &[], body)
}

/// POST one JSON object with extra headers (gh #186: Meta's mint
/// authenticates by header, not by body).
pub fn post_json_with_headers(
    cap: &dyn ProviderCap,
    url: &str,
    extra: &[(&str, &str)],
    body: &serde_json::Value,
) -> Result<(u16, String), IdentityFailure> {
    let bytes = serde_json::to_vec(body)
        .map_err(|err| IdentityFailure(format!("cannot build request: {err}")))?;
    let mut headers = vec![
        ("content-type", "application/json"),
        ("accept", "application/json"),
    ];
    headers.extend_from_slice(extra);
    let handle = cap
        .net_request("POST", url, &headers, Some(&bytes))
        .map_err(|err| IdentityFailure(err.to_string()))?;
    let status = cap
        .net_response_status(handle)
        .map_err(|err| IdentityFailure(err.to_string()))?;
    let mut text = Vec::new();
    while let Some(chunk) = cap
        .net_read_body(handle, 64 * 1024)
        .map_err(|err| IdentityFailure(err.to_string()))?
    {
        text.extend_from_slice(&chunk);
        if text.len() > 1024 * 1024 {
            break;
        }
    }
    let _ = cap.net_close_response(handle);
    Ok((status, String::from_utf8_lossy(&text).into_owned()))
}

/// One PKCE S256 pair (gh #183, gh #185): the verifier crosses on the
/// code exchange, the challenge rides the authorize URL. The same
/// CSPRNG and base64url the kit's own flow uses.
pub fn pkce_pair() -> Result<(String, String), IdentityFailure> {
    let verifier = base64url(&random_bytes(32)?);
    let challenge = {
        use sha2::Digest as _;
        base64url(&sha2::Sha256::digest(verifier.as_bytes()))
    };
    Ok((verifier, challenge))
}

/// The URL-encoder for one authorize query value (gh #183, gh #185):
/// the form field encoder's shape, shared so the extensions build
/// URLs exactly like the kit's own flow does.
pub fn percent_encode(text: &str) -> String {
    percent(text)
}

/// Percent-encode a form field (uppercase hex, like every OAuth server
/// and `URLSearchParams` produce).
fn percent(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

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

fn base64url_decode(text: &str) -> Option<Vec<u8>> {
    let unpadded = text.trim_end_matches('=');
    let mut bits = 0u32;
    let mut count = 0;
    let mut out = Vec::new();
    for byte in unpadded.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            _ => return None,
        } as u32;
        bits = (bits << 6) | value;
        count += 6;
        if count >= 8 {
            count -= 8;
            out.push((bits >> count) as u8);
            bits &= (1 << count) - 1;
        }
    }
    Some(out)
}

fn random_bytes(len: usize) -> Result<Vec<u8>, IdentityFailure> {
    let mut bytes = vec![0u8; len];
    getrandom::fill(&mut bytes)
        .map_err(|err| IdentityFailure(format!("the platform CSPRNG failed: {err}")))?;
    Ok(bytes)
}

/// Where the subscription account id comes from after the exchange.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountStrategy {
    /// Decode it from the access token's JWT payload
    /// (`claim` object, then `field`).
    JwtClaim {
        /// The namespaced claim object.
        claim: &'static str,
        /// The account field inside it.
        field: &'static str,
    },
    /// GET the userinfo endpoint with the token and read `field`
    /// (usually `sub`).
    Userinfo {
        /// The JSON field carrying the account id.
        field: &'static str,
    },
}

/// Everything about one subscription gateway that is data: endpoints,
/// client, scope, account lookup. The flows below are identical for
/// every provider carrying one of these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OAuthSpec {
    /// The extension name, for messages.
    pub name: &'static str,
    /// The authorize endpoint default (`auth_endpoint` overrides).
    pub auth_endpoint: &'static str,
    /// The token endpoint default (`token_endpoint` overrides).
    pub token_endpoint: &'static str,
    /// The revoke endpoint default, when the server has one.
    pub revoke_endpoint: Option<&'static str>,
    /// The userinfo endpoint default (needed for [`AccountStrategy::Userinfo`]).
    pub userinfo_endpoint: Option<&'static str>,
    /// The gateway's inference base default (`api_base` overrides).
    pub api_base: &'static str,
    /// The public OAuth client id (embedded like pi embeds its own).
    pub client_id: &'static str,
    /// The authorize scope.
    pub scope: &'static str,
    /// Extra authorize params (`originator`, `referrer`, ...).
    pub extra_auth_params: &'static [(&'static str, &'static str)],
    /// How the account id is learned after the exchange.
    pub account: AccountStrategy,
}

/// Clear the stored tokens after a rejection: a revoked or expired
/// token must not short-circuit the next login or poison the next
/// call. Best effort — the caller's error carries the news.
pub fn purge_tokens(cap: &dyn ProviderCap) {
    for key in ["access", "refresh", "expires"] {
        let _ = cap.credentials_delete(key);
    }
}

/// The access token, refreshed first when it is within a minute of
/// expiry and a refresh token exists (FR-PROV-5). A rejected refresh
/// purges, so the next call re-authenticates instead of looping.
pub fn access_token(cap: &dyn ProviderCap, spec: &OAuthSpec) -> Result<String, IdentityFailure> {
    let access = stored(cap, "access");
    if access.is_empty() {
        return Err(IdentityFailure(format!(
            "no {} login yet; run /login {}",
            spec.name, spec.name
        )));
    }
    let expires: u64 = stored(cap, "expires").parse().unwrap_or(0);
    if crate::now_epoch() + 60 < expires {
        return Ok(access);
    }
    let refresh = stored(cap, "refresh");
    if refresh.is_empty() {
        return Err(IdentityFailure(format!(
            "the stored {} token is expired and there is no refresh token; run /login {}",
            spec.name, spec.name
        )));
    }
    let url = endpoint(cap, "token_endpoint", spec.token_endpoint);
    let (status, text) = post_form(
        cap,
        &url,
        &[
            ("grant_type", "refresh_token"),
            ("client_id", spec.client_id),
            ("refresh_token", &refresh),
        ],
    )?;
    if !(200..300).contains(&status) {
        if status == 401 {
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
        .and_then(|value| value.as_u64())
        .unwrap_or(3600);
    cap.credentials_set("access", &access)
        .map_err(|err| IdentityFailure(format!("cannot store the token: {err}")))?;
    cap.credentials_set("expires", &(crate::now_epoch() + expires_in).to_string())
        .map_err(|err| IdentityFailure(format!("cannot store the expiry: {err}")))?;
    if let Some(rotated) = json.get("refresh_token").and_then(|value| value.as_str()) {
        cap.credentials_set("refresh", rotated)
            .map_err(|err| IdentityFailure(format!("cannot store the refresh token: {err}")))?;
    }
    Ok(access)
}

/// Decode the account id from the access token's JWT payload.
/// Exported for live smokes, which seed a scratch namespace from an
/// operator-provided token.
pub fn account_from_jwt(token: &str, claim: &str, field: &str) -> Option<String> {
    let payload = token.split('.').nth(1)?;
    let json: serde_json::Value = serde_json::from_slice(&base64url_decode(payload)?).ok()?;
    json.get(claim)?.get(field)?.as_str().map(str::to_string)
}

/// GET the userinfo endpoint with the token and read one field.
fn account_from_userinfo(
    cap: &dyn ProviderCap,
    url: &str,
    token: &str,
    field: &str,
) -> Result<String, IdentityFailure> {
    let bearer = format!("Bearer {token}");
    let handle = cap
        .net_request("GET", url, &[("authorization", bearer.as_str())], None)
        .map_err(|err| IdentityFailure(err.to_string()))?;
    let status = cap
        .net_response_status(handle)
        .map_err(|err| IdentityFailure(err.to_string()))?;
    let mut text = Vec::new();
    while let Some(chunk) = cap
        .net_read_body(handle, 64 * 1024)
        .map_err(|err| IdentityFailure(err.to_string()))?
    {
        text.extend_from_slice(&chunk);
        if text.len() > 1024 * 1024 {
            break;
        }
    }
    let _ = cap.net_close_response(handle);
    if !(200..300).contains(&status) {
        return Err(IdentityFailure(format!(
            "account lookup failed: {}",
            json_error_message(&String::from_utf8_lossy(&text))
        )));
    }
    serde_json::from_slice::<serde_json::Value>(&text)
        .ok()
        .and_then(|json| json.get(field)?.as_str().map(str::to_string))
        .ok_or_else(|| IdentityFailure("account lookup returned no account".to_string()))
}

/// `login`: the loopback OAuth PKCE flow every subscription gateway
/// shares — bind, authorize, await the callback (loopback or pasted),
/// exchange, learn the account, store. A login always runs the flow,
/// even with tokens stored: the invocation itself is the overwrite
/// prompt (gh #179's deadlock, ported forward).
pub fn run_login(
    cap: &dyn ProviderCap,
    oauth: &dyn OauthCap,
    spec: &OAuthSpec,
) -> Result<IdentityOutcome, IdentityFailure> {
    let verifier = base64url(&random_bytes(32)?);
    let challenge = {
        use sha2::Digest as _;
        base64url(&sha2::Sha256::digest(verifier.as_bytes()))
    };
    let state = base64url(&random_bytes(32)?);
    let (redirect, flow) = oauth
        .oauth_begin("/callback")
        .map_err(|err| IdentityFailure(format!("cannot start the loopback flow: {err}")))?;
    let mut url = format!(
        "{}?response_type=code&client_id={}&redirect_uri={}&scope={}&code_challenge={}&\
         code_challenge_method=S256&state={}",
        endpoint(cap, "auth_endpoint", spec.auth_endpoint),
        percent(spec.client_id),
        percent(&redirect),
        percent(spec.scope),
        challenge,
        state,
    );
    for (key, value) in spec.extra_auth_params {
        url.push_str(&format!("&{}={}", percent(key), percent(value)));
    }
    // Best effort like every provider: a machine with no browser
    // launcher still completes through the printed URL and a paste.
    let _ = oauth.oauth_open(&url);
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
    if get("state") != state {
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
    let token_url = endpoint(cap, "token_endpoint", spec.token_endpoint);
    let (status, text) = post_form(
        cap,
        &token_url,
        &[
            ("grant_type", "authorization_code"),
            ("client_id", spec.client_id),
            ("code", &code),
            ("code_verifier", &verifier),
            ("redirect_uri", &redirect),
        ],
    )?;
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
        .and_then(|value| value.as_str())
        .ok_or_else(|| IdentityFailure("no access token in the reply".to_string()))?
        .to_string();
    let refresh = json
        .get("refresh_token")
        .and_then(|value| value.as_str())
        .ok_or_else(|| {
            IdentityFailure(
                "no refresh token received; re-run login and allow offline access".to_string(),
            )
        })?
        .to_string();
    let expires_in = json
        .get("expires_in")
        .and_then(|value| value.as_u64())
        .unwrap_or(3600);
    let account = match spec.account {
        AccountStrategy::JwtClaim { claim, field } => account_from_jwt(&access, claim, field)
            .ok_or_else(|| IdentityFailure("the access token carries no account".to_string()))?,
        AccountStrategy::Userinfo { field } => {
            let url = spec
                .userinfo_endpoint
                .ok_or_else(|| IdentityFailure("no userinfo endpoint configured".to_string()))?;
            let url = endpoint(cap, "userinfo_endpoint", url);
            account_from_userinfo(cap, &url, &access, field)?
        }
    };
    for (key, value) in [
        ("access", access.as_str()),
        ("refresh", refresh.as_str()),
        (
            "expires",
            (crate::now_epoch() + expires_in).to_string().as_str(),
        ),
        ("account_id", account.as_str()),
    ] {
        cap.credentials_set(key, value)
            .map_err(|err| IdentityFailure(format!("cannot store {key}: {err}")))?;
    }
    Ok(IdentityOutcome::Ok)
}

/// `logout`: revoke server-side where the gateway supports it, then
/// clear the namespace. A revoke that fails still ends cleared — the
/// local state is what the user asked about.
pub fn run_logout(cap: &dyn ProviderCap, spec: &OAuthSpec) -> IdentityOutcome {
    if let Some(revoke) = spec.revoke_endpoint {
        // RFC 7009 shape (the refresh token, like fx revokes).
        let refresh = stored(cap, "refresh");
        if !refresh.is_empty() {
            let url = endpoint(cap, "revoke_endpoint", revoke);
            let _ = post_form(
                cap,
                &url,
                &[("token", &refresh), ("client_id", spec.client_id)],
            );
        }
    }
    for key in ["access", "refresh", "expires", "account_id"] {
        if cap.credentials_delete(key).is_err() {
            // A missing key is already the desired end state.
        }
    }
    IdentityOutcome::Ok
}

/// One RFC 8628 device authorization (gh #184, #186, #187): the
/// code the user types at the verification page, and the knobs the
/// poll loop runs on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceAuthorization {
    /// The `device_code` the token poll redeems.
    pub device_code: String,
    /// The `user_code` the person types.
    pub user_code: String,
    /// Where they type it (`verification_uri_complete` when the
    /// server sends one, else `verification_uri`).
    pub verification_uri: String,
    /// Seconds between polls (the server's `interval`, else 5 —
    /// RFC 8628 section 3.2).
    pub interval_secs: u64,
    /// The poll loop's own expiry (the server's `expires_in`, else a
    /// 15-minute client ceiling).
    pub expires_in_secs: u64,
}

/// Start one device authorization: form-POST the client (plus scope
/// where the gateway wants one) and validate the answer. The
/// verification URI must be http(s) — it opens in the user's browser
/// (pi's `trustedHttpUrl`).
pub fn request_device_code(
    cap: &dyn ProviderCap,
    url: &str,
    client_id: &str,
    scope: Option<&str>,
) -> Result<DeviceAuthorization, IdentityFailure> {
    let mut pairs = vec![("client_id", client_id)];
    if let Some(scope) = scope {
        pairs.push(("scope", scope));
    }
    let (status, text) = post_form_accept(cap, url, &pairs, "application/json")?;
    if !(200..300).contains(&status) {
        return Err(IdentityFailure(format!(
            "device authorization failed: {}",
            json_error_message(&text)
        )));
    }
    let json: serde_json::Value = serde_json::from_str(&text)
        .map_err(|_| IdentityFailure("invalid device code response".to_string()))?;
    let get = |key: &str| {
        json.get(key)
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_string()
    };
    let device_code = get("device_code");
    let user_code = get("user_code");
    if device_code.is_empty() || user_code.is_empty() {
        return Err(IdentityFailure(
            "invalid device code response fields".to_string(),
        ));
    }
    let complete = get("verification_uri_complete");
    let plain = get("verification_uri");
    let verification_uri = if is_http_url(&complete) {
        complete
    } else if is_http_url(&plain) {
        plain
    } else {
        return Err(IdentityFailure(
            "untrusted verification_uri in device code response".to_string(),
        ));
    };
    Ok(DeviceAuthorization {
        device_code,
        user_code,
        verification_uri,
        interval_secs: json
            .get("interval")
            .and_then(|value| value.as_u64())
            .filter(|interval| *interval > 0)
            .unwrap_or(5),
        expires_in_secs: json
            .get("expires_in")
            .and_then(|value| value.as_u64())
            .filter(|expires| *expires > 0)
            .unwrap_or(900),
    })
}

/// Only http(s) URLs open in the user's browser.
fn is_http_url(value: &str) -> bool {
    value.starts_with("http://") || value.starts_with("https://")
}

/// The URL the extension asks the host to open for one device
/// authorization (gh #184): the verification page with the user code
/// in the fragment. Fragments never reach the server, so navigation
/// is unaffected; the host splits the code back off for display (the
/// only freeze-safe channel — no new host import).
pub fn device_code_url(authorization: &DeviceAuthorization) -> String {
    format!(
        "{}#code={}",
        authorization.verification_uri, authorization.user_code
    )
}

/// One device-token poll's verdict (pi's `device-code.ts` statuses).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DevicePollOutcome {
    /// The server answered a token: the whole JSON body.
    Complete(serde_json::Value),
    /// `authorization_pending`: keep polling.
    Pending,
    /// `slow_down`: wait longer (the server's new `interval`, if valid).
    SlowDown(Option<u64>),
    /// Anything else: the message ends the flow.
    Failed(String),
}

/// POST one device-token poll and map the answer (all three gateways
/// share the mapping: `access_token` wins, the two wait codes wait,
/// everything else — including a transport-shaped 5xx — ends it).
pub fn poll_device_once(
    cap: &dyn ProviderCap,
    url: &str,
    pairs: &[(&str, &str)],
) -> Result<DevicePollOutcome, IdentityFailure> {
    let (status, text) = post_form_accept(cap, url, pairs, "application/json")?;
    let json: serde_json::Value = serde_json::from_str(&text).ok().unwrap_or_default();
    if (200..300).contains(&status)
        && let Some(token) = json.get("access_token").and_then(|value| value.as_str())
        && !token.is_empty()
    {
        return Ok(DevicePollOutcome::Complete(json));
    }
    let error = json
        .get("error")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    match error {
        "authorization_pending" => Ok(DevicePollOutcome::Pending),
        "slow_down" => Ok(DevicePollOutcome::SlowDown(
            json.get("interval")
                .and_then(|value| value.as_u64())
                .filter(|interval| *interval > 0),
        )),
        _ => {
            let description = json
                .get("error_description")
                .and_then(|value| value.as_str())
                .map(|description| format!(": {description}"))
                .unwrap_or_default();
            let message = if error.is_empty() {
                format!("device token request failed with status {status}")
            } else {
                format!("device flow failed: {error}{description}")
            };
            Ok(DevicePollOutcome::Failed(message))
        }
    }
}

/// Drive the poll loop to a token (pi's `pollOAuthDeviceCodeFlow`):
/// wait one interval first, floor intervals at one second, grow 5
/// seconds per `slow_down` (RFC 8628 section 3.5, the server's own
/// interval winning when valid), and expire on the authorization's
/// own clock. Sleeps run in-guest on the login thread; a host cancel
/// lands on the next poll's network call.
pub fn poll_device_code(
    cap: &dyn ProviderCap,
    authorization: &DeviceAuthorization,
    url: &str,
    pairs: &[(&str, &str)],
) -> Result<serde_json::Value, IdentityFailure> {
    use std::time::{Duration, Instant};
    let deadline = Instant::now() + Duration::from_secs(authorization.expires_in_secs);
    let mut interval = Duration::from_secs(authorization.interval_secs.max(1));
    let mut slow_downs = 0u32;
    std::thread::sleep(interval.min(deadline.saturating_duration_since(Instant::now())));
    while Instant::now() < deadline {
        match poll_device_once(cap, url, pairs)? {
            DevicePollOutcome::Complete(json) => return Ok(json),
            DevicePollOutcome::Failed(message) => return Err(IdentityFailure(message)),
            DevicePollOutcome::SlowDown(server) => {
                slow_downs += 1;
                interval = match server {
                    Some(secs) => Duration::from_secs(secs.max(1)),
                    None => interval + Duration::from_secs(5),
                };
            }
            DevicePollOutcome::Pending => {}
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        std::thread::sleep(interval.min(remaining));
    }
    Err(IdentityFailure(if slow_downs > 0 {
        "device flow timed out after one or more slow_down responses; check the machine clock and try again"
            .to_string()
    } else {
        "device flow timed out".to_string()
    }))
}

/// POST form fields and read the bounded reply, asking for JSON back
/// (the device endpoints answer url-encoded without the `Accept`).
/// Public for the form-speaking token endpoints (gh #187's refresh).
pub fn post_form_accept(
    cap: &dyn ProviderCap,
    url: &str,
    pairs: &[(&str, &str)],
    accept: &str,
) -> Result<(u16, String), IdentityFailure> {
    let body: String = pairs
        .iter()
        .map(|(key, value)| format!("{}={}", percent(key), percent(value)))
        .collect::<Vec<_>>()
        .join("&");
    let headers = [
        ("content-type", "application/x-www-form-urlencoded"),
        ("accept", accept),
    ];
    let handle = cap
        .net_request("POST", url, &headers, Some(body.as_bytes()))
        .map_err(|err| IdentityFailure(err.to_string()))?;
    let status = cap
        .net_response_status(handle)
        .map_err(|err| IdentityFailure(err.to_string()))?;
    let mut text = Vec::new();
    while let Some(chunk) = cap
        .net_read_body(handle, 64 * 1024)
        .map_err(|err| IdentityFailure(err.to_string()))?
    {
        text.extend_from_slice(&chunk);
        if text.len() > 1024 * 1024 {
            break;
        }
    }
    let _ = cap.net_close_response(handle);
    Ok((status, String::from_utf8_lossy(&text).into_owned()))
}

/// Seconds since the Unix epoch (bounds, labels, expiries).
pub fn now_epoch() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or(0)
}
