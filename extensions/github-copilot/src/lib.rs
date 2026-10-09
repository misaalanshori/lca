//! GitHub Copilot subscription provider (gh #184): the device-code
//! flow, the Copilot token exchange, chat completions on the shared
//! OpenAI wire kit. pi's
//! `packages/ai/src/auth/oauth/github-copilot.ts` is the port source;
//! the device flow lives on the kit's device primitives, the rest is
//! this gateway's own shapes (token-in-token-out exchange, the
//! `proxy-ep` base URL, the Copilot header block).

use lca_protocol::{
    CompletionRequest, LoginAnswer, LoginOption, OauthCap, ProviderCap, StreamEvent, Usage,
};
use lca_subscription::{IdentityFailure, device_code_url, poll_device_code, request_device_code};
use lca_wire_openai::StreamFailure;

/// pi's public OAuth client id (its source carries it base64-encoded;
/// decoded here once — a public client, not a secret).
pub const CLIENT_ID: &str = "Iv1.b507a08c87ecfe98";
/// The device scope pi requests.
pub const SCOPE: &str = "read:user";
/// The default GitHub domain (an enterprise domain overrides).
pub const GITHUB_DOMAIN_DEFAULT: &str = "github.com";
/// The default inference base (a token's `proxy-ep` wins when present).
pub const API_BASE_DEFAULT: &str = "https://api.individual.githubcopilot.com";
/// pi's `COPILOT_API_VERSION`.
pub const COPILOT_API_VERSION: &str = "2026-06-01";
/// The device login choice.
pub const CHOICE_DEVICE: &str = "device";

/// pi's `COPILOT_HEADERS`: the agent identifies as Copilot Chat.
fn copilot_headers() -> Vec<(&'static str, &'static str)> {
    vec![
        ("User-Agent", "GitHubCopilotChat/0.35.0"),
        ("Editor-Version", "vscode/1.107.0"),
        ("Editor-Plugin-Version", "copilot-chat/0.35.0"),
        ("Copilot-Integration-Id", "vscode-chat"),
    ]
}

/// The grants the manifest declares (the device + token hosts, every
/// Copilot API host, this provider's own credential namespace — no
/// loopback flow, so no `oauth` section).
#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::expect_used)] // the extension's own literal manifest patterns are valid by construction.
pub fn manifest_grants() -> lca_tools::CapabilityGrants {
    lca_tools::CapabilityGrants {
        net: vec![
            lca_permissions::parse_net_pattern("github.com").expect("github.com parses"),
            lca_permissions::parse_net_pattern("api.github.com").expect("api.github.com parses"),
            lca_permissions::parse_net_pattern("api.githubcopilot.com")
                .expect("api.githubcopilot.com parses"),
            lca_permissions::parse_net_pattern("api.individual.githubcopilot.com")
                .expect("individual parses"),
            lca_permissions::parse_net_pattern("api.business.githubcopilot.com")
                .expect("business parses"),
            lca_permissions::parse_net_pattern("api.enterprise.githubcopilot.com")
                .expect("enterprise parses"),
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

/// The GitHub domain: the stored enterprise domain, else github.com.
/// A custom domain's hosts ride the ad-hoc grant the login-time
/// request prompts for (least privilege keeps them out of the file).
fn domain(cap: &dyn ProviderCap) -> String {
    let enterprise = stored(cap, "enterprise_domain").trim().to_string();
    if enterprise.is_empty() {
        GITHUB_DOMAIN_DEFAULT.to_string()
    } else {
        enterprise
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .trim_end_matches('/')
            .to_string()
    }
}

/// The login options: one device flow, with pi's enterprise-domain
/// prompt as an optional field (empty means github.com).
pub fn login_options() -> Vec<LoginOption> {
    vec![LoginOption {
        id: CHOICE_DEVICE.to_string(),
        name: "GitHub Copilot (device code)".to_string(),
        kind: "oauth".to_string(),
        host: "github.com".to_string(),
        fields: vec!["enterprise-domain".to_string()],
        extras: Default::default(),
    }]
}

/// Consume one login answer: remember the enterprise domain, then run
/// the device flow (the Waiting modal shows the code and the page;
/// the extension polls to completion).
pub fn login_submit(
    cap: &dyn ProviderCap,
    oauth: &dyn OauthCap,
    answer: &LoginAnswer,
) -> Result<Vec<(String, String)>, String> {
    if answer.choice != CHOICE_DEVICE {
        return Err(format!(
            "unknown github-copilot login choice `{}`",
            answer.choice
        ));
    }
    let enterprise = answer
        .value("enterprise-domain")
        .unwrap_or("")
        .trim()
        .to_string();
    if enterprise.is_empty() {
        let _ = cap.credentials_delete("enterprise_domain");
    } else {
        cap.credentials_set("enterprise_domain", &enterprise)
            .map_err(|err| format!("cannot store the domain: {err}"))?;
    }
    run_device_login(cap, oauth)
        .map_err(|err| err.0)
        .map(|()| Vec::new())
}

/// `login`: the device flow on the default domain (headless runs here;
/// the picker route carries the enterprise field).
pub fn run_login(
    cap: &dyn ProviderCap,
    oauth: &dyn OauthCap,
) -> Result<lca_protocol::IdentityOutcome, IdentityFailure> {
    run_device_login(cap, oauth).map(|()| lca_protocol::IdentityOutcome::Ok)
}

/// `logout`: clear the namespace (GitHub device grants are revoked on
/// the site; no revoke endpoint exists).
pub fn run_logout(cap: &dyn ProviderCap) -> lca_protocol::IdentityOutcome {
    for key in ["github_token", "access", "expires", "enterprise_domain"] {
        let _ = cap.credentials_delete(key);
    }
    lca_protocol::IdentityOutcome::Ok
}

/// The device flow (pi's `loginGitHubCopilot` minus the model-policy
/// enabling: enabling happens on github.com, best-effort POSTs stay
/// out of the login path — see the provider doc).
fn run_device_login(cap: &dyn ProviderCap, oauth: &dyn OauthCap) -> Result<(), IdentityFailure> {
    let domain = domain(cap);
    let auth_url = endpoint(
        cap,
        "device_code_endpoint",
        &format!("https://{domain}/login/device/code"),
    );
    let authorization = request_device_code(cap, &auth_url, CLIENT_ID, Some(SCOPE))?;
    // The host shows the code beside the page and polls nothing — the
    // poll below is the whole wait (device flows never loop back).
    let _ = oauth.oauth_open(&device_code_url(&authorization));
    let token_url = endpoint(
        cap,
        "device_token_endpoint",
        &format!("https://{domain}/login/oauth/access_token"),
    );
    let pairs = [
        ("client_id", CLIENT_ID),
        ("device_code", authorization.device_code.as_str()),
        ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
    ];
    let json = poll_device_code(cap, &authorization, &token_url, &pairs)?;
    let github_token = json
        .get("access_token")
        .and_then(|value| value.as_str())
        .ok_or_else(|| IdentityFailure("the device poll answered without a token".to_string()))?;
    mint_copilot_token(cap, github_token)?;
    Ok(())
}

/// Exchange a GitHub token for a Copilot token (pi's
/// `refreshGitHubCopilotAccessToken`): `GET`-shaped POST to the
/// internal token endpoint, storing the short-lived token beside the
/// long-lived GitHub one.
fn mint_copilot_token(cap: &dyn ProviderCap, github_token: &str) -> Result<(), IdentityFailure> {
    let domain = domain(cap);
    let url = endpoint(
        cap,
        "copilot_token_endpoint",
        &format!("https://api.{domain}/copilot_internal/v2/token"),
    );
    let bearer = format!("Bearer {github_token}");
    let mut headers: Vec<(&str, &str)> = vec![
        ("accept", "application/json"),
        ("authorization", bearer.as_str()),
    ];
    for (key, value) in copilot_headers() {
        headers.push((key, value));
    }
    let handle = cap
        .net_request("GET", &url, &headers, None)
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
    let text = String::from_utf8_lossy(&text);
    if !(200..300).contains(&status) {
        return Err(IdentityFailure(format!(
            "copilot token exchange failed: {}",
            lca_wire_openai::json_error_message(&text)
        )));
    }
    let json: serde_json::Value = serde_json::from_str(&text)
        .map_err(|_| IdentityFailure("invalid copilot token response".to_string()))?;
    let token = json
        .get("token")
        .and_then(|value| value.as_str())
        .filter(|token| !token.is_empty())
        .ok_or_else(|| IdentityFailure("invalid copilot token response fields".to_string()))?;
    let expires_at = json
        .get("expires_at")
        .and_then(|value| value.as_u64())
        .unwrap_or(0);
    for (key, value) in [
        ("github_token", github_token),
        ("access", token),
        (
            "expires",
            (expires_at.saturating_sub(300)).to_string().as_str(),
        ),
    ] {
        cap.credentials_set(key, value)
            .map_err(|err| IdentityFailure(format!("cannot store {key}: {err}")))?;
    }
    Ok(())
}

/// The inference base (pi's `getGitHubCopilotBaseUrl`): the token's
/// `proxy-ep` wins (`proxy.` becomes `api.`), then the enterprise
/// shape, then the individual default.
fn api_base(cap: &dyn ProviderCap) -> String {
    let override_base = endpoint(cap, "api_base", "");
    if !override_base.is_empty() {
        return override_base;
    }
    if let Some(api) = base_from_token(&stored(cap, "access")) {
        return api;
    }
    let domain = domain(cap);
    if domain != GITHUB_DOMAIN_DEFAULT {
        return format!("https://copilot-api.{domain}");
    }
    API_BASE_DEFAULT.to_string()
}

/// `proxy.individual.githubcopilot.com` → `api.individual...`.
fn base_from_token(token: &str) -> Option<String> {
    let proxy = token.split("proxy-ep=").nth(1)?.split(';').next()?;
    if proxy.is_empty() {
        return None;
    }
    let api = match proxy.strip_prefix("proxy.") {
        Some(rest) => format!("api.{rest}"),
        None => proxy.to_string(),
    };
    Some(format!("https://{api}"))
}

/// The Copilot token, minted first when it is within five minutes of
/// expiry (the mint stamps the ceiling itself).
fn access_token(cap: &dyn ProviderCap) -> Result<String, IdentityFailure> {
    let access = stored(cap, "access");
    let expires: u64 = stored(cap, "expires").parse().unwrap_or(0);
    if !access.is_empty() && now_epoch() + 300 < expires {
        return Ok(access);
    }
    let github_token = stored(cap, "github_token");
    if github_token.is_empty() {
        return Err(IdentityFailure(
            "no github-copilot login yet; run /login github-copilot".to_string(),
        ));
    }
    mint_copilot_token(cap, &github_token)?;
    let access = stored(cap, "access");
    if access.is_empty() {
        return Err(IdentityFailure(
            "the mint answered without a token; run /login github-copilot".to_string(),
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

/// Build one chat call: the token-derived base, the Copilot header
/// block with the API version, the kit body.
pub fn build_request(
    cap: &dyn ProviderCap,
    request: &CompletionRequest,
) -> Result<RequestParts, StreamFailure> {
    let token = access_token(cap).map_err(|err| StreamFailure {
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
    let base = api_base(cap);
    let url = format!("{}/chat/completions", base.trim_end_matches('/'));
    let bearer = format!("Bearer {token}");
    let mut headers: Vec<(String, String)> = vec![
        ("content-type".to_string(), "application/json".to_string()),
        ("authorization".to_string(), bearer),
        (
            "X-GitHub-Api-Version".to_string(),
            COPILOT_API_VERSION.to_string(),
        ),
    ];
    for (key, value) in copilot_headers() {
        headers.push((key.to_string(), value.to_string()));
    }
    Ok((url, headers, body_bytes))
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

/// Parse one `/models` answer into picker rows (pi's
/// `parseGitHubCopilotModelCatalog` minus the policy fallback: rows
/// the picker enables and policies not disabled; enabling happens on
/// github.com).
fn parse_models(text: &str) -> Vec<(String, bool)> {
    let json: serde_json::Value = serde_json::from_str(text).unwrap_or_default();
    let Some(data) = json.get("data").and_then(|value| value.as_array()) else {
        return Vec::new();
    };
    let mut rows = Vec::new();
    for item in data {
        let Some(id) = item.get("id").and_then(|value| value.as_str()) else {
            continue;
        };
        if id.is_empty() {
            continue;
        }
        let supports = item
            .get("capabilities")
            .and_then(|caps| caps.get("supports"))
            .and_then(|supports| supports.get("tool_calls"))
            .and_then(|tools| tools.as_bool())
            .unwrap_or(true);
        if !supports {
            continue;
        }
        let picker = item
            .get("model_picker_enabled")
            .and_then(|value| value.as_bool())
            == Some(true);
        let policy = item
            .get("policy")
            .and_then(|policy| policy.get("state"))
            .and_then(|state| state.as_str())
            .unwrap_or("");
        if policy == "disabled" {
            continue;
        }
        if picker || policy == "enabled" {
            rows.push((id.to_string(), picker));
        }
    }
    rows
}

/// The static fallback table (the issue's rows): vendor-documented
/// windows, no guesses. Live rows carry no window (the catalog
/// publishes none) and win when the gateway answers.
pub const CURATED_MODELS: [(&str, &str, u32); 5] = [
    ("gpt-4o", "GPT-4o (Copilot)", 128000),
    ("claude-3.5-sonnet", "Claude 3.5 Sonnet (Copilot)", 200000),
    ("claude-3.7-sonnet", "Claude 3.7 Sonnet (Copilot)", 200000),
    ("o1", "o1 (Copilot)", 200000),
    ("o3-mini", "o3-mini (Copilot)", 200000),
];

/// The model catalog: the gateway's own `/models` when the token is
/// live (per-account policies included), the curated floor otherwise.
pub fn list_models(cap: &dyn ProviderCap) -> Vec<lca_protocol::ModelInfo> {
    if let Ok(token) = access_token(cap)
        && let Some(live) = discover_models(cap, &token)
        && !live.is_empty()
    {
        return live
            .into_iter()
            .map(|(id, _)| lca_protocol::ModelInfo {
                id: id.clone(),
                name: id,
                context_window: 0,
                max_tokens: 0,
                extras: Default::default(),
            })
            .collect();
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

/// One bounded catalog read over the token-derived base.
fn discover_models(cap: &dyn ProviderCap, token: &str) -> Option<Vec<(String, bool)>> {
    let base = api_base(cap);
    let url = format!("{}/models", base.trim_end_matches('/'));
    let bearer = format!("Bearer {token}");
    let mut headers: Vec<(&str, &str)> = vec![
        ("accept", "application/json"),
        ("authorization", bearer.as_str()),
    ];
    for (key, value) in copilot_headers() {
        headers.push((key, value));
    }
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
    let rows = parse_models(&String::from_utf8_lossy(&body));
    (!rows.is_empty()).then_some(rows)
}

/// The outcome alias identity returns on the success path.
pub type IdentityOutcomeAlias = lca_protocol::IdentityOutcome;

#[cfg(not(target_arch = "wasm32"))]
mod native;

#[cfg(not(target_arch = "wasm32"))]
pub use native::GitHubCopilot;

#[cfg(target_arch = "wasm32")]
mod wasm_mode;
