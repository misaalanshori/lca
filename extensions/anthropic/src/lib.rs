//! Anthropic Claude via the Messages API (gh #183): API-key and
//! Claude Pro/Max subscription login, `POST /v1/messages` streaming on
//! the shared wire kit, a static model table. The OAuth shapes (browser
//! loopback + copy-code paste, JSON token exchange) are pi's
//! `packages/ai/src/auth/oauth/anthropic.ts`; the request shapes are
//! pi's `anthropic-messages.ts` where they map. The flow lives here,
//! not in the subscription kit: the JSON exchange, the missing account
//! lookup, and the copy-code redirect fit neither `run_login` nor the
//! form-speaking gateways (the Antigravity precedent for non-standard
//! OAuth). Shared primitives (`pkce_pair`, `post_json`) come from the kit.

use lca_protocol::{
    CompletionRequest, ContentBlock, IdentityOutcome, LoginAnswer, LoginOption, MessageRole,
    ModelInfo, OauthCap, ProviderCap, StreamEvent, Usage,
};
use lca_subscription::{IdentityFailure, percent_encode, pkce_pair, post_json};

/// pi's public OAuth client id (its source carries it base64-encoded;
/// decoded here once: it is a public client, not a secret, and ships
/// in every copy like pi's own — the Antigravity precedent).
pub const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
/// pi's `AUTHORIZE_URL`.
pub const AUTHORIZE_URL: &str = "https://claude.ai/oauth/authorize";
/// pi's `TOKEN_URL` (the token exchange lives on `platform`, not `claude.ai`).
pub const TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
/// pi's `COPY_CODE_REDIRECT_URI`: the page that shows the pasted code.
pub const COPY_CODE_REDIRECT_URI: &str = "https://platform.claude.com/oauth/code/callback";
/// pi's `SCOPES` verbatim.
pub const SCOPE: &str = "org:create_api_key user:profile user:inference \
    user:sessions:claude_code user:mcp_servers user:file_upload";
/// The inference base default (`api_base` overrides).
pub const API_BASE_DEFAULT: &str = "https://api.anthropic.com";
/// The Messages path default.
pub const MESSAGES_PATH: &str = "/v1/messages";
/// The API version header every request carries (the issue's shape).
pub const ANTHROPIC_VERSION: &str = "2023-06-01";
/// The cache + thinking betas (the issue's shape).
pub const ANTHROPIC_BETAS: &str = "prompt-caching-2024-07-31,thinking-2025-02-19";
/// pi's OAuth-identity betas: a subscription token must prove it is
/// Claude Code talking, or the gateway refuses the call.
pub const OAUTH_BETAS: &str = "claude-code-20250219,oauth-2025-04-20";

/// The login choices (`/login anthropic` offers all three; headless
/// `lca auth login` runs the browser flow, pi's default).
pub const CHOICE_API_KEY: &str = "api-key";
/// The browser loopback flow.
pub const CHOICE_SUBSCRIPTION: &str = "subscription";
/// The headless copy-code flow (paste what the platform page shows).
pub const CHOICE_COPY_CODE: &str = "subscription-copy-code";

/// The kit's failure shape, re-exported so both drivers name one
/// type (the consistency gate forbids a second `StreamFailure`).
pub use lca_wire_anthropic::StreamFailure;

/// An identity failure as a stream failure (the orphan rule keeps
/// the `From` impl out of reach: both types are foreign here).
fn identity_failure(err: IdentityFailure) -> StreamFailure {
    StreamFailure {
        message: err.0,
        class: "invalid",
        retryable: false,
    }
}

/// The grants the manifest declares (the inference host, both OAuth
/// hosts, the loopback flow, this provider's own credential namespace).
#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::expect_used)] // the extension's own literal manifest patterns are valid by construction.
pub fn manifest_grants() -> lca_tools::CapabilityGrants {
    lca_tools::CapabilityGrants {
        net: vec![
            lca_permissions::parse_net_pattern("api.anthropic.com")
                .expect("api.anthropic.com parses"),
            lca_permissions::parse_net_pattern("claude.ai").expect("claude.ai parses"),
            lca_permissions::parse_net_pattern("platform.claude.com")
                .expect("platform.claude.com parses"),
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

/// The login options: an API key, or the Claude subscription through
/// the browser or the copy-code paste (pi offers the same choice).
pub fn login_options() -> Vec<LoginOption> {
    vec![
        LoginOption {
            id: CHOICE_API_KEY.to_string(),
            name: "API key".to_string(),
            kind: "api-key".to_string(),
            host: "api.anthropic.com".to_string(),
            fields: vec!["api-key".to_string()],
            extras: Default::default(),
        },
        LoginOption {
            id: CHOICE_SUBSCRIPTION.to_string(),
            name: "Claude subscription (browser)".to_string(),
            kind: "oauth".to_string(),
            host: "claude.ai".to_string(),
            fields: Vec::new(),
            extras: Default::default(),
        },
        LoginOption {
            id: CHOICE_COPY_CODE.to_string(),
            name: "Claude subscription (copy code)".to_string(),
            kind: "oauth".to_string(),
            host: "platform.claude.com".to_string(),
            fields: Vec::new(),
            extras: Default::default(),
        },
    ]
}

/// Consume one login answer: store the API key, or run the chosen
/// subscription flow (the Waiting modal shows the authorization URL
/// and the paste fallback delivers the callback, like every OAuth
/// provider).
pub fn login_submit(
    cap: &dyn ProviderCap,
    oauth: &dyn OauthCap,
    answer: &LoginAnswer,
) -> Result<Vec<(String, String)>, String> {
    match answer.choice.as_str() {
        CHOICE_API_KEY => {
            let key = answer.value("api-key").unwrap_or("").trim().to_string();
            if key.is_empty() {
                return Err("no API key entered".to_string());
            }
            cap.credentials_set("api_key", &key)
                .map_err(|err| format!("cannot store the key: {err}"))?;
            Ok(Vec::new())
        }
        CHOICE_COPY_CODE => run_copy_code_login(cap, oauth)
            .map(|()| Vec::new())
            .map_err(|err| err.0),
        CHOICE_SUBSCRIPTION => run_browser_login(cap, oauth)
            .map(|()| Vec::new())
            .map_err(|err| err.0),
        other => Err(format!("unknown anthropic login choice `{other}`")),
    }
}

/// `login`: the browser loopback flow, pi's default method.
pub fn run_login(
    cap: &dyn ProviderCap,
    oauth: &dyn OauthCap,
) -> Result<IdentityOutcome, IdentityFailure> {
    run_browser_login(cap, oauth).map(|()| IdentityOutcome::Ok)
}

/// `logout`: clear the namespace (pi exposes no revoke for Claude
/// subscription tokens; the Codex precedent).
pub fn run_logout(cap: &dyn ProviderCap) -> IdentityOutcome {
    for key in ["access", "refresh", "expires", "api_key"] {
        let _ = cap.credentials_delete(key);
    }
    IdentityOutcome::Ok
}

/// `usage`: the credential-validity probe these gateways get — no
/// quota endpoint exists, so a live token is `Ok` with empty counts
/// and anything else is the re-login error.
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

/// The browser loopback flow (pi's `loginAnthropic`): PKCE, the
/// authorize URL with pi's `code=true` flag, the loopback-or-paste
/// wait, then the JSON exchange.
fn run_browser_login(cap: &dyn ProviderCap, oauth: &dyn OauthCap) -> Result<(), IdentityFailure> {
    let (verifier, challenge) = pkce_pair()?;
    // pi sends the verifier as the state; the equality check below
    // then covers both values at once.
    let state = verifier.clone();
    let (redirect, flow) = oauth
        .oauth_begin("/callback")
        .map_err(|err| IdentityFailure(format!("cannot start the loopback flow: {err}")))?;
    let url = authorize_url(
        &endpoint(cap, "auth_endpoint", AUTHORIZE_URL),
        &redirect,
        &challenge,
        &state,
    );
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
        return Err(IdentityFailure(
            "the authorization server returned no code".to_string(),
        ));
    }
    exchange_code(cap, &code, &state, &verifier, &redirect)
}

/// The copy-code flow (pi's `loginAnthropicCopyCode`): the platform
/// page shows `code#state`, the user pastes it, the host delivers the
/// two pairs, and the exchange names the copy-code redirect.
fn run_copy_code_login(cap: &dyn ProviderCap, oauth: &dyn OauthCap) -> Result<(), IdentityFailure> {
    let (verifier, challenge) = pkce_pair()?;
    let state = verifier.clone();
    // No loopback listener would ever fire (the redirect lands on
    // pi's platform page, not here), but the flow handle is the seam
    // the paste fallback delivers through — the browser method's own
    // shape, minus the bind anyone waits on.
    let (_, flow) = oauth
        .oauth_begin("/callback")
        .map_err(|err| IdentityFailure(format!("cannot start the login flow: {err}")))?;
    let url = authorize_url(
        &endpoint(cap, "auth_endpoint", AUTHORIZE_URL),
        COPY_CODE_REDIRECT_URI,
        &challenge,
        &state,
    );
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
    let pasted_state = get("state");
    if !pasted_state.is_empty() && pasted_state != state {
        return Err(IdentityFailure("OAuth state mismatch".to_string()));
    }
    let code = get("code");
    if code.is_empty() {
        return Err(IdentityFailure(
            "no code pasted; copy what the Anthropic page shows".to_string(),
        ));
    }
    exchange_code(
        cap,
        &code,
        if pasted_state.is_empty() {
            &state
        } else {
            &pasted_state
        },
        &verifier,
        COPY_CODE_REDIRECT_URI,
    )
}

/// One authorize URL (pi's params, in pi's order): `code=true` first,
/// then the standard PKCE set.
fn authorize_url(base: &str, redirect_uri: &str, challenge: &str, state: &str) -> String {
    format!(
        "{}?code=true&client_id={}&response_type=code&redirect_uri={}&scope={}&code_challenge={}&\
         code_challenge_method=S256&state={}",
        base,
        percent_encode(CLIENT_ID),
        percent_encode(redirect_uri),
        percent_encode(SCOPE),
        percent_encode(challenge),
        percent_encode(state),
    )
}

/// The JSON code exchange (pi's `exchangeAuthorizationCode`): form
/// would 400 here — the token endpoint speaks JSON only.
fn exchange_code(
    cap: &dyn ProviderCap,
    code: &str,
    state: &str,
    verifier: &str,
    redirect_uri: &str,
) -> Result<(), IdentityFailure> {
    let url = endpoint(cap, "token_endpoint", TOKEN_URL);
    let (status, text) = post_json(
        cap,
        &url,
        &serde_json::json!({
            "grant_type": "authorization_code",
            "client_id": CLIENT_ID,
            "code": code,
            "state": state,
            "redirect_uri": redirect_uri,
            "code_verifier": verifier,
        }),
    )?;
    if !(200..300).contains(&status) {
        return Err(IdentityFailure(format!(
            "token exchange failed: {}",
            error_line(&text)
        )));
    }
    store_tokens(cap, &text)
}

/// The JSON refresh (pi's `refreshAnthropicToken`).
fn refresh_tokens(cap: &dyn ProviderCap) -> Result<(), IdentityFailure> {
    let refresh = stored(cap, "refresh");
    if refresh.is_empty() {
        return Err(IdentityFailure(
            "the stored login is expired and there is no refresh token; run /login anthropic"
                .to_string(),
        ));
    }
    let url = endpoint(cap, "token_endpoint", TOKEN_URL);
    let (status, text) = post_json(
        cap,
        &url,
        &serde_json::json!({
            "grant_type": "refresh_token",
            "client_id": CLIENT_ID,
            "refresh_token": refresh,
        }),
    )?;
    if !(200..300).contains(&status) {
        for key in ["access", "refresh", "expires"] {
            let _ = cap.credentials_delete(key);
        }
        return Err(IdentityFailure(format!(
            "token refresh failed; run /login anthropic ({})",
            error_line(&text)
        )));
    }
    store_tokens(cap, &text)
}

/// Persist one token reply (both exchange and refresh answer the same
/// shape). pi keeps no account: the tokens are opaque, so neither do we.
fn store_tokens(cap: &dyn ProviderCap, text: &str) -> Result<(), IdentityFailure> {
    let json: serde_json::Value =
        serde_json::from_str(text).map_err(|err| IdentityFailure(format!("token reply: {err}")))?;
    let access = json
        .get("access_token")
        .and_then(|value| value.as_str())
        .ok_or_else(|| IdentityFailure("no access token in the reply".to_string()))?
        .to_string();
    let refresh = json
        .get("refresh_token")
        .and_then(|value| value.as_str())
        .ok_or_else(|| IdentityFailure("no refresh token in the reply".to_string()))?
        .to_string();
    let expires_in = json
        .get("expires_in")
        .and_then(|value| value.as_u64())
        .unwrap_or(3600);
    for (key, value) in [
        ("access", access.as_str()),
        ("refresh", refresh.as_str()),
        ("expires", (now_epoch() + expires_in).to_string().as_str()),
    ] {
        cap.credentials_set(key, value)
            .map_err(|err| IdentityFailure(format!("cannot store {key}: {err}")))?;
    }
    Ok(())
}

/// Seconds since the Unix epoch (bounds, labels, expiries).
fn now_epoch() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

/// One error line out of a JSON failure body: Anthropic nests it
/// (`{"type":"error","error":{"message":...}}`), so the nested message
/// wins, then the flat OAuth shapes, then truncated raw text.
fn error_line(text: &str) -> String {
    serde_json::from_str::<serde_json::Value>(text)
        .ok()
        .and_then(|json| {
            json.get("error")
                .and_then(|error| {
                    error
                        .get("message")
                        .and_then(|message| message.as_str())
                        .or_else(|| error.as_str())
                })
                .or_else(|| {
                    json.get("error_description")
                        .or_else(|| json.get("message"))
                        .and_then(|value| value.as_str())
                })
                .map(str::to_string)
        })
        .unwrap_or_else(|| text.trim().chars().take(200).collect())
}

/// The subscription access token, refreshed first when it is within a
/// minute of expiry (the kit's `access_token` shape, over the JSON
/// refresh — an API key never reaches here).
fn subscription_token(cap: &dyn ProviderCap) -> Result<String, IdentityFailure> {
    let access = stored(cap, "access");
    if access.is_empty() {
        return Err(IdentityFailure(
            "no anthropic login yet; run /login anthropic".to_string(),
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
            "the refresh answered without a token; run /login anthropic".to_string(),
        ));
    }
    Ok(access)
}

/// The credential for one call: a stored or environment API key wins
/// (plain billing, no OAuth headers); otherwise the subscription token.
/// Native only reads the environment — the sandbox must not see the
/// host's (the openai-compatible precedent; the `[capabilities.env]`
/// grant waits on #170).
fn access_token(cap: &dyn ProviderCap) -> Result<(String, bool), IdentityFailure> {
    let key = stored(cap, "api_key");
    if !key.is_empty() {
        return Ok((key, false));
    }
    #[cfg(not(target_arch = "wasm32"))]
    if let Ok(key) = std::env::var("ANTHROPIC_API_KEY")
        && !key.trim().is_empty()
    {
        return Ok((key.trim().to_string(), false));
    }
    subscription_token(cap).map(|token| (token, true))
}

/// The request headers: the key (an API key or the subscription access
/// token — pi sends both as `x-api-key`), the version, and the betas
/// (plus pi's OAuth-identity pair on a subscription token).
fn headers(token: &str, oauth: bool) -> Vec<(String, String)> {
    let betas = if oauth {
        format!("{ANTHROPIC_BETAS},{OAUTH_BETAS}")
    } else {
        ANTHROPIC_BETAS.to_string()
    };
    vec![
        ("x-api-key".to_string(), token.to_string()),
        (
            "anthropic-version".to_string(),
            ANTHROPIC_VERSION.to_string(),
        ),
        ("anthropic-beta".to_string(), betas),
        ("content-type".to_string(), "application/json".to_string()),
        ("accept".to_string(), "text/event-stream".to_string()),
    ]
}

/// The token budget: the turn's explicit budget first, then the
/// model's table ceiling, then Anthropic's documented legacy default.
/// Messages refuses a call without one, so there is always a number.
fn max_tokens(cap: &dyn ProviderCap, request: &CompletionRequest) -> u32 {
    if let Some(budget) = request
        .extras
        .get("max-tokens")
        .and_then(|value| value.parse::<u64>().ok())
        .and_then(|budget| u32::try_from(budget).ok())
        .filter(|budget| *budget > 0)
    {
        return budget;
    }
    if let Some(ceiling) = list_models(cap)
        .iter()
        .find(|model| model.id == request.model)
        .map(|model| model.max_tokens)
        .filter(|ceiling| *ceiling > 0)
    {
        return ceiling;
    }
    8192
}

/// The wire parts of one call: the URL, the headers, the body, and
/// whether the credential is a subscription token.
pub type RequestParts = (String, Vec<(String, String)>, Vec<u8>, bool);

/// The wire parts of one call: the native driver and the WASM guest
/// share the builder, so the two deliveries send byte-identical
/// requests.
pub fn build_request(
    cap: &dyn ProviderCap,
    request: &CompletionRequest,
) -> Result<RequestParts, StreamFailure> {
    let (token, oauth) = access_token(cap).map_err(identity_failure)?;
    let system: String = request
        .messages
        .iter()
        .filter(|message| message.role == MessageRole::System)
        .flat_map(|message| {
            message.content.iter().filter_map(|block| match block {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
        })
        .collect::<Vec<_>>()
        .join("\n");
    if request.model.is_empty() {
        return Err(StreamFailure {
            message: "this provider has no model selected; run /model to pick one".to_string(),
            class: "invalid",
            retryable: false,
        });
    }
    let budget = max_tokens(cap, request);
    let body =
        lca_wire_anthropic::build_messages_body(request, &system, &request.model, budget, true);
    let body_bytes = serde_json::to_vec(&body).map_err(|err| StreamFailure {
        message: format!("cannot build request: {err}"),
        class: "invalid",
        retryable: false,
    })?;
    let base = endpoint(cap, "api_base", API_BASE_DEFAULT);
    #[cfg(not(target_arch = "wasm32"))]
    let base = std::env::var("ANTHROPIC_BASE_URL")
        .map(|value| {
            if value.trim().is_empty() {
                base.clone()
            } else {
                value
            }
        })
        .unwrap_or(base);
    let url = format!("{}{MESSAGES_PATH}", base.trim_end_matches('/'));
    let headers = headers(&token, oauth);
    Ok((url, headers, body_bytes, oauth))
}

/// One streamed completion over capabilities: authenticate, build the
/// Messages body (cache breakpoints on, thinking budgets and
/// signatures from P5A riding the kit), send it, and map the SSE
/// frames. `emit` returning `false` stops the read (FR-CONC-3).
pub fn run_provider_stream(
    cap: &dyn ProviderCap,
    request: &CompletionRequest,
    emit: &mut dyn FnMut(StreamEvent) -> bool,
) -> Result<(), StreamFailure> {
    let (url, headers, body_bytes, oauth) = build_request(cap, request)?;
    let refs: Vec<(&str, &str)> = headers
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    let mut driver = match MessagesDriver::open(cap, &url, &refs, &body_bytes) {
        Ok(driver) => driver,
        Err(failure) => {
            if oauth && failure.message.contains("HTTP 401") {
                // A rejected subscription must not short-circuit the
                // next login or poison the next call (the kit's own
                // purge shape; an API key has no trio to clear).
                for key in ["access", "refresh", "expires"] {
                    let _ = cap.credentials_delete(key);
                }
            }
            return Err(failure);
        }
    };
    while let Some(event) = driver.next_event() {
        if !emit(event?) {
            break;
        }
    }
    Ok(())
}

/// The SSE driver over capabilities: split the byte stream into
/// `data:` frames, feed each decoded payload to the kit mapper, and
/// surface HTTP failures with the body's own message. The chunk loop
/// is the openai-compatible shape; the feed is the Messages kit.
pub struct MessagesDriver<'a> {
    cap: &'a dyn ProviderCap,
    handle: u32,
    stream: lca_wire_anthropic::AnthropicStream,
    buf: Vec<u8>,
    pending: std::collections::VecDeque<StreamEvent>,
    finished: bool,
}

impl<'a> MessagesDriver<'a> {
    /// Send the request and check the status. A non-2xx response is
    /// read (bounded) and reported with its own message.
    pub fn open(
        cap: &'a dyn ProviderCap,
        url: &str,
        headers: &[(&str, &str)],
        body: &[u8],
    ) -> Result<MessagesDriver<'a>, StreamFailure> {
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
            return Err(StreamFailure {
                message: format!("provider returned HTTP {status}: {}", error_line(&text)),
                class: match status {
                    401 | 403 => "auth",
                    400..=499 => "invalid",
                    _ => "transport",
                },
                retryable: matches!(status, 429 | 500 | 502 | 503 | 529),
            });
        }
        Ok(MessagesDriver {
            cap,
            handle,
            stream: lca_wire_anthropic::AnthropicStream::new(),
            buf: Vec::new(),
            pending: std::collections::VecDeque::new(),
            finished: false,
        })
    }

    /// Feed one decoded frame: `data:` lines concatenated,
    /// everything else (`event:`, comments) skipped.
    fn feed_frame(&mut self, frame: &str) {
        let mut data = String::new();
        for line in frame.lines() {
            if let Some(payload) = line.strip_prefix("data:") {
                data.push_str(payload.trim_start());
                data.push('\n');
            }
        }
        let data = data.trim_end();
        if data.is_empty() || data == "[DONE]" {
            return;
        }
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(data) {
            self.pending.extend(self.stream.feed(&value));
        }
    }

    /// Split buffered bytes into blank-line-separated frames; at EOF
    /// the trailing bytes are one last frame when non-blank.
    fn push_chunk(&mut self, chunk: &[u8], eof: bool) {
        self.buf.extend_from_slice(chunk);
        loop {
            let end = self.buf.windows(2).position(|pair| pair == b"\n\n");
            let Some(end) = end else { break };
            let frame: Vec<u8> = self.buf.drain(..end + 2).collect();
            self.feed_frame(&String::from_utf8_lossy(&frame));
        }
        if eof && self.buf.iter().any(|byte| !byte.is_ascii_whitespace()) {
            let tail: Vec<u8> = std::mem::take(&mut self.buf);
            self.feed_frame(&String::from_utf8_lossy(&tail));
        }
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
                Ok(Some(chunk)) => self.push_chunk(&chunk, false),
                Ok(None) => {
                    self.finished = true;
                    self.push_chunk(&[], true);
                }
                Err(err) => {
                    self.finished = true;
                    return Some(Err(StreamFailure::from(err)));
                }
            }
        }
    }
}

impl Drop for MessagesDriver<'_> {
    fn drop(&mut self) {
        let _ = self.cap.net_close_response(self.handle);
    }
}

/// The static model table (the issue's catalog): every row's window is
/// Anthropic's documented 200k context for the Claude 3 generation;
/// the ceilings are its documented max-output per model. The table
/// grows with livedata, never guesses.
pub fn list_models(_cap: &dyn ProviderCap) -> Vec<ModelInfo> {
    vec![
        ModelInfo {
            id: "claude-3-7-sonnet".to_string(),
            name: "Claude 3.7 Sonnet".to_string(),
            context_window: 200000,
            max_tokens: 64000,
            extras: Default::default(),
        },
        ModelInfo {
            id: "claude-3-5-sonnet".to_string(),
            name: "Claude 3.5 Sonnet".to_string(),
            context_window: 200000,
            max_tokens: 8192,
            extras: Default::default(),
        },
        ModelInfo {
            id: "claude-3-5-haiku".to_string(),
            name: "Claude 3.5 Haiku".to_string(),
            context_window: 200000,
            max_tokens: 8192,
            extras: Default::default(),
        },
        ModelInfo {
            id: "claude-3-opus".to_string(),
            name: "Claude 3 Opus".to_string(),
            context_window: 200000,
            max_tokens: 4096,
            extras: Default::default(),
        },
    ]
}

/// The outcome alias identity returns on the success path.
pub type IdentityOutcomeAlias = IdentityOutcome;

#[cfg(not(target_arch = "wasm32"))]
mod native;

#[cfg(not(target_arch = "wasm32"))]
pub use native::Anthropic;

#[cfg(target_arch = "wasm32")]
mod wasm_mode;
