//! The shared kit for subscription-gateway provider extensions (gh
//! #63): one OAuth PKCE login/refresh flow and one Responses-protocol
//! stream core, parameterized by small spec tables instead of branches.
//! `extensions/codex` and `extensions/grok` are thin specs over this;
//! provider quirks live in their tables, never in core (#157).

pub mod responses;

pub use responses::{ResponsesStream, build_responses_body, responses_usage};

use lca_protocol::{OauthCap, ProviderCap};

/// Re-exported so extensions name the outcome in their signatures.
pub use lca_protocol::IdentityOutcome;

/// An identity operation failed; the string is shown to the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityFailure(pub String);

impl From<StreamFailure> for IdentityFailure {
    fn from(err: StreamFailure) -> Self {
        IdentityFailure(err.message)
    }
}

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
            E::Timeout(_) => ("transport", true),
            E::Io(_) => ("transport", true),
        };
        StreamFailure {
            message: err.to_string(),
            class,
            retryable,
        }
    }
}

/// Classify an HTTP status into the headless envelope's classes.
pub fn failure_for_status(status: u16, detail: &str) -> StreamFailure {
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
fn account_from_jwt(token: &str, claim: &str, field: &str) -> Option<String> {
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

/// The one-line error inside a vendor JSON error envelope, else the
/// first 200 chars of the body.
pub fn json_error_message(text: &str) -> String {
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

/// Seconds since the Unix epoch (bounds, labels, expiries).
pub fn now_epoch() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or(0)
}
