//! Meta Llama and Muse models via the Muse subscription (gh #186):
//! the RFC 8628 device flow, the Model API key mint, chat completions
//! on the shared OpenAI wire kit. pi's
//! `packages/ai/src/auth/oauth/meta.ts` is the port source; the two
//! halves split identity from access exactly like pi (the identity
//! token rides as `refresh`, the minted day-key as `access`, so the
//! standard refresh path re-mints with no bespoke machinery).

use lca_protocol::{CompletionRequest, OauthCap, ProviderCap, StreamEvent, Usage};
use lca_subscription::{
    IdentityFailure, device_code_url, poll_device_code, post_json_with_headers, request_device_code,
};
use lca_wire_openai::StreamFailure;

/// pi's Muse Code CLI client id (public, ships in every copy).
pub const CLIENT_ID: &str = "1031625952748946";
/// pi's device authorization endpoint.
pub const DEVICE_AUTHORIZATION_URL: &str = "https://auth.meta.com/oidc/device/authorization/";
/// pi's device token endpoint.
pub const DEVICE_TOKEN_URL: &str = "https://auth.meta.com/oidc/device/token/";
/// pi's key-mint endpoint (minted keys live about a day).
pub const API_KEY_MINT_URL: &str = "https://api.meta.ai/muse-code/key";
/// The inference base default (`api_base` overrides).
pub const API_BASE_DEFAULT: &str = "https://api.meta.ai/v1";
/// The chat path default.
pub const CHAT_PATH: &str = "/chat/completions";

/// The grants the manifest declares (the identity host, the API host,
/// this provider's own credential namespace — no loopback flow, so no
/// `oauth` section).
#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::expect_used)] // the extension's own literal manifest patterns are valid by construction.
pub fn manifest_grants() -> lca_tools::CapabilityGrants {
    lca_tools::CapabilityGrants {
        net: vec![
            lca_permissions::parse_net_pattern("auth.meta.com").expect("auth.meta.com parses"),
            lca_permissions::parse_net_pattern("api.meta.ai").expect("api.meta.ai parses"),
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

/// The login options: none — Meta signs in through its identity flow
/// directly (the TUI routes a provider with no options to it, and a
/// check never launches a flow from an empty set).
pub fn login_options() -> Vec<lca_protocol::LoginOption> {
    Vec::new()
}

/// `login`: pi's `loginMeta` — the device authorization, the page with
/// its code, the poll to an identity token, then the first mint.
pub fn run_login(
    cap: &dyn ProviderCap,
    oauth: &dyn OauthCap,
) -> Result<lca_protocol::IdentityOutcome, IdentityFailure> {
    let auth_url = endpoint(
        cap,
        "device_authorization_endpoint",
        DEVICE_AUTHORIZATION_URL,
    );
    let authorization = request_device_code(cap, &auth_url, CLIENT_ID, None)?;
    let _ = oauth.oauth_open(&device_code_url(&authorization));
    let token_url = endpoint(cap, "device_token_endpoint", DEVICE_TOKEN_URL);
    let pairs = [
        ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        ("device_code", authorization.device_code.as_str()),
        ("client_id", CLIENT_ID),
    ];
    let json = poll_device_code(cap, &authorization, &token_url, &pairs)?;
    let identity = json
        .get("access_token")
        .and_then(|value| value.as_str())
        .ok_or_else(|| IdentityFailure("the device poll answered without a token".to_string()))?;
    mint_api_key(cap, identity)?;
    Ok(lca_protocol::IdentityOutcome::Ok)
}

/// `logout`: clear the namespace (the identity token is not renewable;
/// only a fresh device flow helps).
pub fn run_logout(cap: &dyn ProviderCap) -> lca_protocol::IdentityOutcome {
    for key in ["access", "refresh", "expires"] {
        let _ = cap.credentials_delete(key);
    }
    lca_protocol::IdentityOutcome::Ok
}

/// Mint one Model API key from an identity token (pi's `mintApiKey`):
/// the identity lands as `refresh`, the day-key as `access`. A
/// 401/403 means the session is dead — purge, so the next call
/// re-authenticates instead of looping.
fn mint_api_key(cap: &dyn ProviderCap, identity_token: &str) -> Result<(), IdentityFailure> {
    let url = endpoint(cap, "api_key_mint_endpoint", API_KEY_MINT_URL);
    let bearer = format!("Bearer {identity_token}");
    // pi's shape: the version and the identity travel as headers,
    // the body is empty.
    let (status, text) = post_json_with_headers(
        cap,
        &url,
        &[
            ("x-api-version", "1.0.0"),
            ("authorization", bearer.as_str()),
        ],
        &serde_json::json!({}),
    )?;
    if status == 401 || status == 403 {
        for key in ["access", "refresh", "expires"] {
            let _ = cap.credentials_delete(key);
        }
        return Err(IdentityFailure(
            "meta session expired; run /login meta to sign in again".to_string(),
        ));
    }
    if !(200..300).contains(&status) {
        return Err(IdentityFailure(format!(
            "meta API key mint failed: {}",
            lca_wire_openai::json_error_message(&text)
        )));
    }
    let key: String = serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|json| json.get("api_key")?.as_str().map(str::to_string))
        .filter(|key| !key.is_empty())
        .ok_or_else(|| IdentityFailure("meta did not issue an API key".to_string()))?;
    for (slot, value) in [
        ("refresh", identity_token),
        ("access", key.as_str()),
        ("expires", (now_epoch() + 24 * 3600).to_string().as_str()),
    ] {
        cap.credentials_set(slot, value)
            .map_err(|err| IdentityFailure(format!("cannot store {slot}: {err}")))?;
    }
    Ok(())
}

/// The minted key, re-minted first when it is within an hour of
/// expiry (pi's refresh is exactly this mint; the identity token
/// itself never renews).
fn access_token(cap: &dyn ProviderCap) -> Result<String, IdentityFailure> {
    let access = stored(cap, "access");
    let expires: u64 = stored(cap, "expires").parse().unwrap_or(0);
    if !access.is_empty() && now_epoch() + 3600 < expires {
        return Ok(access);
    }
    let identity = stored(cap, "refresh");
    if identity.is_empty() {
        return Err(IdentityFailure(
            "no meta login yet; run /login meta".to_string(),
        ));
    }
    mint_api_key(cap, &identity)?;
    let access = stored(cap, "access");
    if access.is_empty() {
        return Err(IdentityFailure(
            "the mint answered without a key; run /login meta".to_string(),
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

/// `usage`: the credential-validity probe — a live key is `Ok` with
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

/// Build one chat call: the minted bearer over the Meta base.
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

/// The static table (the issue's rows): Llama 3.3 70B carries Meta's
/// published 128k context; `muse-spark` publishes none, so it reads
/// unknown (0 — never compacts) until livedata says otherwise.
pub fn list_models(_cap: &dyn ProviderCap) -> Vec<lca_protocol::ModelInfo> {
    vec![
        lca_protocol::ModelInfo {
            id: "llama-3.3-70b-instruct".to_string(),
            name: "Llama 3.3 70B Instruct".to_string(),
            context_window: 128000,
            max_tokens: 0,
            extras: Default::default(),
        },
        lca_protocol::ModelInfo {
            id: "muse-spark".to_string(),
            name: "Muse Spark".to_string(),
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
pub use native::Meta;

#[cfg(target_arch = "wasm32")]
mod wasm_mode;
