//! Moonshot Kimi models via the Kimi Code subscription (gh #187):
//! the RFC 8628 device flow, the token triple with its retrying
//! refresh, chat completions on the shared OpenAI wire kit. pi's
//! `packages/ai/src/auth/oauth/kimi-coding.ts` is the port source.

use lca_protocol::{CompletionRequest, OauthCap, ProviderCap, StreamEvent, Usage};
use lca_subscription::{
    IdentityFailure, device_code_url, poll_device_code, post_form_accept, request_device_code,
};
use lca_wire_openai::StreamFailure;

/// pi's public client id (ships in every copy, like pi's own).
pub const CLIENT_ID: &str = "17e5f671-d194-4dfb-9706-5516cb48c098";
/// pi's default OAuth host (`oauth_host` overrides).
pub const OAUTH_HOST_DEFAULT: &str = "https://auth.kimi.com";
/// The inference base default (`api_base` overrides).
pub const API_BASE_DEFAULT: &str = "https://api.kimi.com/coding/v1";
/// The chat path default.
pub const CHAT_PATH: &str = "/chat/completions";
/// pi's refresh retries on rate limits and 5xx.
const REFRESH_MAX_RETRIES: u32 = 3;

/// The grants the manifest declares (the identity host, the API host,
/// this provider's own credential namespace — no loopback flow, so no
/// `oauth` section).
#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::expect_used)] // the extension's own literal manifest patterns are valid by construction.
pub fn manifest_grants() -> lca_tools::CapabilityGrants {
    lca_tools::CapabilityGrants {
        net: vec![
            lca_permissions::parse_net_pattern("auth.kimi.com").expect("auth.kimi.com parses"),
            lca_permissions::parse_net_pattern("api.kimi.com").expect("api.kimi.com parses"),
        ],
        credentials: true,
        ..Default::default()
    }
}

pub(crate) fn stored(cap: &dyn ProviderCap, key: &str) -> String {
    cap.credentials_get(key).unwrap_or_default()
}

pub(crate) fn endpoint(cap: &dyn ProviderCap, key: &str, default: &str) -> String {
    let value = stored(cap, key);
    if value.is_empty() {
        default.to_string()
    } else {
        value
    }
}

/// The manifest next to this source, so tests pin the file the
/// installer reads to the grants the native form carries.
pub const MANIFEST: &str = include_str!("../extension.toml");

/// The OAuth host: pi honors `KIMI_CODE_OAUTH_HOST`/`KIMI_OAUTH_HOST`;
/// the stored override plays that role here (native env reads stay
/// out of the sandbox).
fn oauth_host(cap: &dyn ProviderCap) -> String {
    endpoint(cap, "oauth_host", OAUTH_HOST_DEFAULT)
        .trim_end_matches('/')
        .to_string()
}

/// The login options: none — Kimi signs in through its identity flow
/// directly (the TUI routes a provider with no options to it, and a
/// check never launches a flow from an empty set).
pub fn login_options() -> Vec<lca_protocol::LoginOption> {
    Vec::new()
}

/// `login`: pi's `loginKimiCoding` — the device authorization, the
/// complete page with its code, the poll to the token triple.
pub fn run_login(
    cap: &dyn ProviderCap,
    oauth: &dyn OauthCap,
) -> Result<lca_protocol::IdentityOutcome, IdentityFailure> {
    let host = oauth_host(cap);
    let auth_url = format!("{host}/api/oauth/device_authorization");
    let authorization = request_device_code(cap, &auth_url, CLIENT_ID, None)?;
    let _ = oauth.oauth_open(&device_code_url(&authorization));
    let token_url = format!("{host}/api/oauth/token");
    let pairs = [
        ("client_id", CLIENT_ID),
        ("device_code", authorization.device_code.as_str()),
        ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
    ];
    let json = poll_device_code(cap, &authorization, &token_url, &pairs)?;
    store_tokens(cap, &json, "poll")?;
    Ok(lca_protocol::IdentityOutcome::Ok)
}

/// `logout`: clear the namespace (refresh tokens die server-side on
/// their own schedule; the site revokes).
pub fn run_logout(cap: &dyn ProviderCap) -> lca_protocol::IdentityOutcome {
    for key in ["access", "refresh", "expires"] {
        let _ = cap.credentials_delete(key);
    }
    lca_protocol::IdentityOutcome::Ok
}

/// Persist one token reply (pi's `parseTokenResponse`): all three
/// fields or nothing.
fn store_tokens(
    cap: &dyn ProviderCap,
    json: &serde_json::Value,
    operation: &str,
) -> Result<(), IdentityFailure> {
    let access = json
        .get("access_token")
        .and_then(|value| value.as_str())
        .filter(|token| !token.is_empty())
        .ok_or_else(|| {
            IdentityFailure(format!(
                "kimi Code token {operation} response missing fields"
            ))
        })?;
    let refresh = json
        .get("refresh_token")
        .and_then(|value| value.as_str())
        .filter(|token| !token.is_empty())
        .ok_or_else(|| {
            IdentityFailure(format!(
                "kimi Code token {operation} response missing fields"
            ))
        })?;
    let expires_in = json
        .get("expires_in")
        .and_then(|value| value.as_u64())
        .filter(|expires| *expires > 0)
        .ok_or_else(|| {
            IdentityFailure(format!(
                "kimi Code token {operation} response missing fields"
            ))
        })?;
    for (slot, value) in [
        ("access", access),
        ("refresh", refresh),
        ("expires", (now_epoch() + expires_in).to_string().as_str()),
    ] {
        cap.credentials_set(slot, value)
            .map_err(|err| IdentityFailure(format!("cannot store {slot}: {err}")))?;
    }
    Ok(())
}

/// The refresh (pi's `refreshToken`): 429/5xx retry with backoff, a
/// 401/403/`invalid_grant` kills the credential with the re-login
/// error, anything else fails loudly.
fn refresh_tokens(cap: &dyn ProviderCap) -> Result<(), IdentityFailure> {
    let refresh = stored(cap, "refresh");
    if refresh.is_empty() {
        return Err(IdentityFailure(
            "the stored kimi login is expired and has no refresh token; run /login kimi-coding"
                .to_string(),
        ));
    }
    let url = format!("{}/api/oauth/token", oauth_host(cap));
    let mut last_error = String::new();
    for attempt in 0..=REFRESH_MAX_RETRIES {
        if attempt > 0 {
            std::thread::sleep(std::time::Duration::from_secs(1 << (attempt - 1)));
        }
        let pairs = [
            ("client_id", CLIENT_ID),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh.as_str()),
        ];
        // The refresh endpoint speaks form, like the device endpoints.
        let (status, text) = match post_form_accept(cap, &url, &pairs, "application/json") {
            Ok(reply) => reply,
            Err(err) => {
                last_error = err.0;
                continue;
            }
        };
        let json: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
        if (200..300).contains(&status) {
            if store_tokens(cap, &json, "refresh").is_ok() {
                return Ok(());
            }
            last_error = "kimi Code token refresh response missing fields".to_string();
            continue;
        }
        if status == 401
            || status == 403
            || json.get("error").and_then(|value| value.as_str()) == Some("invalid_grant")
        {
            for key in ["access", "refresh", "expires"] {
                let _ = cap.credentials_delete(key);
            }
            return Err(IdentityFailure(
                "kimi Code token refresh unauthorized; run /login kimi-coding".to_string(),
            ));
        }
        if (status == 429 || status >= 500) && attempt < REFRESH_MAX_RETRIES {
            last_error = format!("kimi Code token refresh failed with status {status}");
            continue;
        }
        return Err(IdentityFailure(format!(
            "kimi Code token refresh failed with status {status}"
        )));
    }
    Err(IdentityFailure(if last_error.is_empty() {
        "kimi Code token refresh failed".to_string()
    } else {
        last_error
    }))
}

/// The access token, refreshed first when it is within a minute of
/// expiry and a refresh token exists.
fn access_token(cap: &dyn ProviderCap) -> Result<String, IdentityFailure> {
    let access = stored(cap, "access");
    if access.is_empty() {
        return Err(IdentityFailure(
            "no kimi-coding login yet; run /login kimi-coding".to_string(),
        ));
    }
    let expires: u64 = stored(cap, "expires").parse().unwrap_or(0);
    if now_epoch() + 60 < expires {
        return Ok(access);
    }
    refresh_tokens(cap)?;
    let access = stored(cap, "access");
    if access.is_empty() {
        return Err(IdentityFailure(
            "the refresh answered without a token; run /login kimi-coding".to_string(),
        ));
    }
    Ok(access)
}

/// Seconds since the Unix epoch.
fn now_epoch() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

/// `usage`: the credential-validity probe — a live token is `Ok` with
/// empty counts (no quota endpoint exists).
pub fn run_usage(cap: &dyn ProviderCap) -> Result<Usage, IdentityFailure> {
    access_token(cap)?;
    Ok(Usage {
        input: 0,
        output: 0,
        cache_read: 0,
        cache_write: 0,
        cache_write_1h: 0,
        cost: 0.0,
        cost_input: 0.0,
        cost_cache_read: 0.0,
        cost_cache_write: 0.0,
        extras: Default::default(),
    })
}

/// The wire parts of one call (URL, headers, body): the native driver
/// and the WASM guest share it, so the two deliveries send
/// byte-identical requests.
pub type RequestParts = (String, Vec<(String, String)>, Vec<u8>);

/// Build one chat call: the token bearer over the Kimi coding base.
pub fn build_request(
    cap: &dyn ProviderCap,
    request: &CompletionRequest,
) -> Result<RequestParts, StreamFailure> {
    let key = access_token(cap).map_err(|err| StreamFailure {
        message: err.0,
        class: "invalid",
        retryable: false,
    })?;
    let model = if request.model.is_empty() {
        return Err(StreamFailure {
            message: "this provider has no model selected; run /model to pick one".to_string(),
            class: "invalid",
            retryable: false,
        });
    } else {
        request.model.clone()
    };
    let mut body = serde_json::json!({
        "model": model,
        "messages": lca_wire_openai::to_wire(&request.messages),
        "stream": true,
        "stream_options": { "include_usage": true },
    });
    if let Some(budget) = request
        .extras
        .get("max-tokens")
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|budget| *budget > 0)
    {
        body["max_tokens"] = serde_json::json!(budget);
    }
    let tools = lca_wire_openai::tools_wire(&request.tools);
    if !tools.is_empty() {
        body["tools"] = serde_json::Value::Array(tools);
    }
    let body_bytes = serde_json::to_vec(&body).map_err(|err| StreamFailure {
        message: format!("cannot build request: {err}"),
        class: "invalid",
        retryable: false,
    })?;
    let base = endpoint(cap, "api_base", API_BASE_DEFAULT);
    let url = format!("{}{CHAT_PATH}", base.trim_end_matches('/'));
    Ok((
        url,
        vec![
            ("content-type".to_string(), "application/json".to_string()),
            ("authorization".to_string(), format!("Bearer {key}")),
        ],
        body_bytes,
    ))
}

/// One streamed completion over capabilities. `emit` returning `false`
/// stops the read (FR-CONC-3).
pub fn run_provider_stream(
    cap: &dyn ProviderCap,
    request: &CompletionRequest,
    emit: &mut dyn FnMut(StreamEvent) -> bool,
) -> Result<(), StreamFailure> {
    let (url, headers, body_bytes) = build_request(cap, request)?;
    let refs: Vec<(&str, &str)> = headers
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    let mut driver = ChatDriver::open(cap, &url, &refs, &body_bytes)?;
    while let Some(event) = driver.next_event() {
        if !emit(event?) {
            break;
        }
    }
    Ok(())
}

/// The SSE driver over capabilities: the openrouter chunk loop over
/// the shared chat decoder (provenance noted, not forked).
pub struct ChatDriver<'a> {
    cap: &'a dyn ProviderCap,
    handle: u32,
    decoder: lca_wire_openai::SseDecoder,
    pending: std::collections::VecDeque<StreamEvent>,
    finished: bool,
}

impl<'a> ChatDriver<'a> {
    /// Send the request and check the status. A non-2xx response is
    /// read (bounded) and classified by the kit.
    pub fn open(
        cap: &'a dyn ProviderCap,
        url: &str,
        headers: &[(&str, &str)],
        body: &[u8],
    ) -> Result<ChatDriver<'a>, StreamFailure> {
        let handle = cap.net_request("POST", url, headers, Some(body))?;
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
            let message = format!(
                "provider returned HTTP {status}: {}",
                lca_wire_openai::json_error_message(&text)
            );
            let classified = lca_wire_openai::failure_for_status(status, &message);
            return Err(StreamFailure {
                message,
                class: classified.class,
                retryable: classified.retryable,
            });
        }
        Ok(ChatDriver {
            cap,
            handle,
            decoder: lca_wire_openai::SseDecoder::default(),
            pending: std::collections::VecDeque::new(),
            finished: false,
        })
    }

    /// The next typed event, reading more of the body when the buffer is
    /// empty; `None` at end of stream.
    pub fn next_event(&mut self) -> Option<Result<StreamEvent, StreamFailure>> {
        loop {
            if let Some(event) = self.pending.pop_front() {
                return Some(Ok(event));
            }
            if self.finished {
                return None;
            }
            match self.cap.net_read_body(self.handle, 64 * 1024) {
                Ok(Some(chunk)) => {
                    let mut events = Vec::new();
                    self.decoder.feed(&chunk, &mut |event| events.push(event));
                    self.pending.extend(events);
                }
                Ok(None) => {
                    self.finished = true;
                    let mut events = Vec::new();
                    self.decoder.finish(&mut |event| events.push(event));
                    self.pending.extend(events);
                }
                Err(err) => {
                    self.finished = true;
                    return Some(Err(StreamFailure::from(err)));
                }
            }
        }
    }
}

impl Drop for ChatDriver<'_> {
    fn drop(&mut self) {
        let _ = self.cap.net_close_response(self.handle);
    }
}

/// The static table (the issue's rows): the vendor publishes no
/// windows for these ids, so both read unknown (0 — never compacts)
/// until livedata says otherwise.
pub fn list_models(_cap: &dyn ProviderCap) -> Vec<lca_protocol::ModelInfo> {
    vec![
        lca_protocol::ModelInfo {
            id: "kimi-k2-coding".to_string(),
            name: "Kimi K2 Coding".to_string(),
            context_window: 0,
            max_tokens: 0,
            extras: Default::default(),
        },
        lca_protocol::ModelInfo {
            id: "kimi-latest".to_string(),
            name: "Kimi Latest".to_string(),
            context_window: 0,
            max_tokens: 0,
            extras: Default::default(),
        },
    ]
}

/// The outcome alias identity returns on the success path.
pub type IdentityOutcomeAlias = lca_protocol::IdentityOutcome;

#[cfg(not(target_arch = "wasm32"))]
mod native;

#[cfg(not(target_arch = "wasm32"))]
pub use native::KimiCoding;

#[cfg(target_arch = "wasm32")]
mod wasm_mode;
