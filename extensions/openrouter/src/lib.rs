//! OpenRouter multi-model gateway via one-click OAuth (gh #185):
//! the loopback PKCE flow whose exchange provisions a permanent API
//! key, chat completions on the shared OpenAI wire kit, live model
//! discovery with a grounded curated fallback. pi's
//! `packages/ai/src/auth/oauth/openrouter.ts` is the port source; the
//! exchange (JSON, no client id, no state, no refresh) fits neither
//! `run_login` nor the form-speaking gateways, so the flow lives here
//! on the kit's primitives (the #183 precedent). Keys stay in the
//! `openai-compatible` preset — this extension never takes one.

use lca_protocol::{CompletionRequest, OauthCap, ProviderCap, StreamEvent, Usage};
use lca_subscription::{IdentityFailure, percent_encode, pkce_pair, post_json};
use lca_wire_openai::StreamFailure;

/// pi's `AUTHORIZE_URL`.
pub const AUTHORIZE_URL: &str = "https://openrouter.ai/auth";
/// pi's `TOKEN_URL`: the exchange provisions the key.
pub const TOKEN_URL: &str = "https://openrouter.ai/api/v1/auth/keys";
/// The gateway base default (`api_base` overrides).
pub const API_BASE_DEFAULT: &str = "https://openrouter.ai/api/v1";
/// The chat path default.
pub const CHAT_PATH: &str = "/chat/completions";
/// The model catalog path (reports `context_length` per id).
pub const MODELS_PATH: &str = "/models";

/// The grants the manifest declares (the gateway, the loopback flow,
/// this provider's own credential namespace).
#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::expect_used)] // the extension's own literal manifest patterns are valid by construction.
pub fn manifest_grants() -> lca_tools::CapabilityGrants {
    lca_tools::CapabilityGrants {
        net: vec![
            lca_permissions::parse_net_pattern("openrouter.ai").expect("openrouter.ai parses"),
        ],
        oauth: Some(lca_permissions::OAuthSettings {
            redirect_path: "/callback".to_string(),
            timeout_seconds: 300,
        }),
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

/// The login options: none — OpenRouter signs in through its identity
/// flow directly (the TUI routes a provider with no options to it, and
/// a check never launches a flow from an empty set). Keys stay manual
/// in the `openai-compatible` preset.
pub fn login_options() -> Vec<lca_protocol::LoginOption> {
    Vec::new()
}

/// `login`: pi's `loginOpenRouter` — PKCE, the authorize URL (no
/// client id, no scope, no state: OpenRouter's shape), the
/// loopback-or-paste wait, then the JSON key exchange. pi binds a
/// random callback path since no state crosses; we take the fixed
/// `/callback` every other provider uses (single-flight login, the
/// flow handle still gates delivery — noted, not silently dropped).
pub fn run_login(
    cap: &dyn ProviderCap,
    oauth: &dyn OauthCap,
) -> Result<lca_protocol::IdentityOutcome, IdentityFailure> {
    let (verifier, challenge) = pkce_pair()?;
    let (redirect, flow) = oauth
        .oauth_begin("/callback")
        .map_err(|err| IdentityFailure(format!("cannot start the loopback flow: {err}")))?;
    let url = format!(
        "{}?callback_url={}&code_challenge={}&code_challenge_method=S256",
        endpoint(cap, "auth_endpoint", AUTHORIZE_URL),
        percent_encode(&redirect),
        percent_encode(&challenge),
    );
    let _ = oauth.oauth_open(&url);
    let callback = oauth.oauth_await(flow);
    let _ = oauth.oauth_end(flow);
    let callback = callback.map_err(|err| IdentityFailure(err.to_string()))?;
    let code = callback
        .iter()
        .find(|(name, _)| name == "code")
        .map(|(_, value)| value.clone())
        .unwrap_or_default();
    if code.is_empty() {
        return Err(IdentityFailure(
            "the authorization server returned no code".to_string(),
        ));
    }
    exchange_code(cap, &code, &verifier)?;
    Ok(lca_protocol::IdentityOutcome::Ok)
}

/// The JSON key exchange (pi's `exchangeAuthorizationCode`): no
/// redirect back (the loopback already proved possession), no client
/// id, and the answer is a permanent key — not a token pair.
fn exchange_code(cap: &dyn ProviderCap, code: &str, verifier: &str) -> Result<(), IdentityFailure> {
    let url = endpoint(cap, "token_endpoint", TOKEN_URL);
    let (status, text) = post_json(
        cap,
        &url,
        &serde_json::json!({
            "code": code,
            "code_verifier": verifier,
            "code_challenge_method": "S256",
        }),
    )?;
    if !(200..300).contains(&status) {
        return Err(IdentityFailure(format!(
            "key exchange failed: {}",
            lca_wire_openai::json_error_message(&text)
        )));
    }
    let key: String = serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|json| json.get("key")?.as_str().map(str::to_string))
        .filter(|key| !key.is_empty())
        .ok_or_else(|| IdentityFailure("the exchange carried no key".to_string()))?;
    // pi's credential: the key as the access token, no refresh, an
    // expiry that never arrives.
    for (slot, value) in [
        ("access", key.as_str()),
        ("refresh", ""),
        ("expires", u64::MAX.to_string().as_str()),
    ] {
        cap.credentials_set(slot, value)
            .map_err(|err| IdentityFailure(format!("cannot store {slot}: {err}")))?;
    }
    Ok(())
}

/// `logout`: clear the namespace (OpenRouter keys are revoked by the
/// user on the site; no revoke endpoint exists).
pub fn run_logout(cap: &dyn ProviderCap) -> lca_protocol::IdentityOutcome {
    for key in ["access", "refresh", "expires"] {
        let _ = cap.credentials_delete(key);
    }
    lca_protocol::IdentityOutcome::Ok
}

/// The provisioned key: stored first, the environment second (native
/// only — the sandbox must not see the host's). An empty store and no
/// variable is the re-login error.
fn access_key(cap: &dyn ProviderCap) -> Result<String, IdentityFailure> {
    let key = stored(cap, "access");
    if !key.is_empty() {
        return Ok(key);
    }
    #[cfg(not(target_arch = "wasm32"))]
    if let Ok(key) = std::env::var("OPENROUTER_API_KEY")
        && !key.trim().is_empty()
    {
        return Ok(key.trim().to_string());
    }
    Err(IdentityFailure(
        "no openrouter login yet; run /login openrouter".to_string(),
    ))
}

/// `usage`: the credential-validity probe — the key never expires, so
/// a present key is `Ok` with empty counts (no quota endpoint exists).
pub fn run_usage(cap: &dyn ProviderCap) -> Result<Usage, IdentityFailure> {
    access_key(cap)?;
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

/// The wire parts of one call: the URL, the headers, the body.
pub type RequestParts = (String, Vec<(String, String)>, Vec<u8>);

/// The wire parts of one call: the native driver and the WASM guest
/// share the builder, so the two deliveries send byte-identical
/// requests.
pub fn build_request(
    cap: &dyn ProviderCap,
    request: &CompletionRequest,
) -> Result<RequestParts, StreamFailure> {
    let key = access_key(cap).map_err(|err| StreamFailure {
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
    // gh #169's generation budget, same seam as every OpenAI-shaped
    // endpoint: without it the gateway's default cuts long summaries.
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

/// One streamed completion over capabilities: authenticate, build the
/// chat body on the shared kit, send it, and map the SSE frames.
/// `emit` returning `false` stops the read (FR-CONC-3).
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

/// The SSE driver over capabilities: the openai-compatible chunk loop
/// over the shared chat decoder.
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

/// The curated fallback table: the `openai-compatible` preset's three
/// rows, with windows from the endpoint's own `/models` answer where
/// livedata exists (`openai/gpt-4o` read 128000 live, 2026-10-03) and
/// the vendors' documented contexts otherwise. Discovery answers when
/// the gateway is reachable; this table is the offline floor.
pub const CURATED_MODELS: [(&str, &str, u32); 3] = [
    ("openai/gpt-4o", "GPT-4o (OpenRouter)", 128000),
    (
        "anthropic/claude-3.5-sonnet",
        "Claude 3.5 Sonnet (OpenRouter)",
        200000,
    ),
    (
        "google/gemini-2.0-flash",
        "Gemini 2.0 Flash (OpenRouter)",
        1048576,
    ),
];

/// The model catalog: the gateway's own `/models` when it answers
/// (every entry's `context_length` rides, the field OpenRouter
/// publishes and the preset discovery already reads), the curated
/// floor otherwise.
pub fn list_models(cap: &dyn ProviderCap) -> Vec<lca_protocol::ModelInfo> {
    if let Some(live) = discover_models(cap)
        && !live.is_empty()
    {
        return live;
    }
    CURATED_MODELS
        .iter()
        .map(|(id, name, window)| lca_protocol::ModelInfo {
            id: (*id).to_string(),
            name: (*name).to_string(),
            context_window: *window,
            max_tokens: 0,
            extras: Default::default(),
        })
        .collect()
}

/// One bounded catalog read (the openai-compatible discovery shape,
/// cut to what a gateway catalog needs: id + `context_length`).
fn discover_models(cap: &dyn ProviderCap) -> Option<Vec<lca_protocol::ModelInfo>> {
    let key = access_key(cap).ok()?;
    let base = endpoint(cap, "api_base", API_BASE_DEFAULT);
    let url = format!("{}{MODELS_PATH}", base.trim_end_matches('/'));
    let bearer = format!("Bearer {key}");
    let headers = [
        ("accept", "application/json"),
        ("authorization", bearer.as_str()),
    ];
    let handle = cap.net_request("GET", &url, &headers, None).ok()?;
    let status = cap.net_response_status(handle).ok();
    let mut body = Vec::new();
    for _ in 0..100 {
        match cap.net_read_body(handle, 64 * 1024) {
            Ok(Some(chunk)) => {
                body.extend_from_slice(&chunk);
                if body.len() > 1024 * 1024 {
                    break;
                }
            }
            _ => break,
        }
    }
    let _ = cap.net_close_response(handle);
    if !matches!(status, Some(200..=299)) {
        return None;
    }
    let json: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&body)).ok()?;
    let data = json.get("data")?.as_array()?;
    let mut models = Vec::new();
    for entry in data {
        let id = entry.get("id")?.as_str()?;
        if id.is_empty() {
            continue;
        }
        let window = entry
            .get("context_length")
            .and_then(|value| value.as_u64())
            .and_then(|window| u32::try_from(window).ok())
            .unwrap_or(0);
        models.push(lca_protocol::ModelInfo {
            id: id.to_string(),
            name: id.to_string(),
            context_window: window,
            max_tokens: 0,
            extras: Default::default(),
        });
    }
    (!models.is_empty()).then_some(models)
}

/// The outcome alias identity returns on the success path.
pub type IdentityOutcomeAlias = lca_protocol::IdentityOutcome;

#[cfg(not(target_arch = "wasm32"))]
mod native;

#[cfg(not(target_arch = "wasm32"))]
pub use native::OpenRouter;

#[cfg(target_arch = "wasm32")]
mod wasm_mode;
