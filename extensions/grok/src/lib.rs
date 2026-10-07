//! xAI Grok models via SuperGrok / X subscription login (gh #181):
//! the loopback OAuth flow, the Grok responses proxy, a static model
//! table. The flow and the Responses protocol live in the subscription
//! kit; this crate is the spec table over it (gh #63).

use lca_protocol::ProviderCap;
use lca_subscription::{AccountStrategy, IdentityFailure, OAuthSpec};
use lca_wire_openai::StreamFailure;

/// Everything about the Grok gateway that is data (port sources:
/// `~/gits/my-fx-fork/src/core/auth/grok_oauth.zig` and
/// `src/gateway/xai_grok.zig`; pi's `xai.ts` where it maps).
pub const SPEC: OAuthSpec = OAuthSpec {
    name: "grok",
    auth_endpoint: "https://auth.x.ai/oauth2/authorize",
    token_endpoint: "https://auth.x.ai/oauth2/token",
    revoke_endpoint: Some("https://auth.x.ai/oauth2/revoke"),
    userinfo_endpoint: Some("https://auth.x.ai/oauth2/userinfo"),
    api_base: "https://cli-chat-proxy.grok.com",
    client_id: "b1a00492-073a-47ea-816f-4c329264a828",
    scope: "openid profile email offline_access grok-cli:access api:access",
    extra_auth_params: &[("referrer", "lca")],
    account: AccountStrategy::Userinfo { field: "sub" },
};

/// The inference path default (`api_base` overrides).
pub const RESPONSES_PATH: &str = "/v1/responses";

/// The proxy's compatibility version (fx's `proxy_compatibility_version`).
const PROXY_VERSION: &str = "1.0.6";

/// The grants the manifest declares (the Grok gateway, the loopback
/// flow, this provider's own credential namespace).
#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::expect_used)] // the extension's own literal manifest patterns are valid by construction.
pub fn manifest_grants() -> lca_tools::CapabilityGrants {
    lca_tools::CapabilityGrants {
        net: vec![
            lca_permissions::parse_net_pattern("cli-chat-proxy.grok.com")
                .expect("cli-chat-proxy.grok.com parses"),
            lca_permissions::parse_net_pattern("auth.x.ai").expect("auth.x.ai parses"),
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

/// `login`: the kit's loopback OAuth flow over this gateway's spec.
pub fn run_login(
    cap: &dyn ProviderCap,
    oauth: &dyn lca_protocol::OauthCap,
) -> Result<lca_protocol::IdentityOutcome, IdentityFailure> {
    lca_subscription::run_login(cap, oauth, &SPEC)
}

/// `logout`: revoke the refresh token server-side, then clear the
/// namespace. A revoke that fails still ends cleared.
pub fn run_logout(cap: &dyn ProviderCap) -> lca_protocol::IdentityOutcome {
    lca_subscription::run_logout(cap, &SPEC)
}

/// `usage`: the credential-validity probe these gateways get — no
/// quota endpoint exists, so an unexpired (or refreshable) token is
/// `Ok` with an empty count and anything else is the re-login error.
/// This is what makes `auth check` answer `ready` for a live login.
pub fn run_usage(cap: &dyn ProviderCap) -> Result<lca_protocol::Usage, IdentityFailure> {
    lca_subscription::access_token(cap, &SPEC)?;
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
        extras: Default::default(),
    })
}

/// The request headers (pi's `buildSSEHeaders`): the bearer, the JWT
/// account, pi's `originator` spelled `lca`, the beta flag.
/// The proxy headers (fx's extra header block): the bearer, the
/// token-auth marker, the client version and identifier spelled for
/// this agent, the model override, and the account.
pub(crate) fn headers(account_id: &str, model: &str, token: &str) -> Vec<(String, String)> {
    vec![
        ("authorization".to_string(), format!("Bearer {token}")),
        ("accept".to_string(), "text/event-stream".to_string()),
        ("content-type".to_string(), "application/json".to_string()),
        ("X-XAI-Token-Auth".to_string(), "xai-grok-cli".to_string()),
        (
            "x-authenticateresponse".to_string(),
            "authenticate-response".to_string(),
        ),
        (
            "x-grok-client-version".to_string(),
            PROXY_VERSION.to_string(),
        ),
        ("x-grok-client-identifier".to_string(), "lca".to_string()),
        ("x-grok-model-override".to_string(), model.to_string()),
        ("x-grok-user-id".to_string(), account_id.to_string()),
    ]
}

/// One streamed completion over capabilities: authenticate, build the
/// Responses body, send it, and map the SSE frames. `emit` returning
/// `false` stops the read (FR-CONC-3).
pub fn run_provider_stream(
    cap: &dyn ProviderCap,
    request: &lca_protocol::CompletionRequest,
    emit: &mut dyn FnMut(lca_protocol::StreamEvent) -> bool,
) -> Result<(), StreamFailure> {
    let token = lca_subscription::access_token(cap, &SPEC).map_err(StreamFailure::from)?;
    let account_id = stored(cap, "account_id");
    if account_id.is_empty() {
        return Err(StreamFailure {
            message: "no Grok account stored; run /login grok".to_string(),
            class: "invalid",
            retryable: false,
        });
    }
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
    let effort = request.extras.get("reasoning-effort").map(String::as_str);
    let body = lca_wire_openai::build_responses_body(request, &system, &request.model, effort);
    let body_bytes = serde_json::to_vec(&body).map_err(|err| StreamFailure {
        message: format!("cannot build request: {err}"),
        class: "invalid",
        retryable: false,
    })?;
    let url = format!(
        "{}{RESPONSES_PATH}",
        endpoint(cap, "api_base", SPEC.api_base).trim_end_matches('/')
    );
    let headers = headers(&account_id, &request.model, &token);
    let refs: Vec<(&str, &str)> = headers
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    let mut driver =
        lca_wire_openai::ResponseStreamDriver::open(cap, &url, &refs, &body_bytes, &|| {
            lca_subscription::purge_tokens(cap)
        })?;
    while let Some(event) = driver.next_event() {
        if !emit(event?) {
            break;
        }
    }
    Ok(())
}

/// The static model table. One grounded row: `grok-4.20`, the id fx's
/// gateway tests drive; the window is unpublished, so `0` (never a
/// guess). The table grows with livedata.
pub fn list_models(_cap: &dyn ProviderCap) -> Vec<lca_protocol::ModelInfo> {
    vec![lca_protocol::ModelInfo {
        id: "grok-4.20".to_string(),
        name: "Grok 4.20".to_string(),
        context_window: 0,
        max_tokens: 0,
        extras: Default::default(),
    }]
}

/// The outcome alias identity returns on the success path.
pub type IdentityOutcomeAlias = lca_protocol::IdentityOutcome;

/// The manifest next to this source, so tests pin the file the
/// installer reads to the grants the native form carries.
pub const MANIFEST: &str = include_str!("../extension.toml");

/// The login options: none — Grok signs in through its identity flow
/// directly (the TUI routes a provider with no options to it, and a
/// check never launches a flow from an empty set).
pub fn login_options() -> Vec<lca_protocol::LoginOption> {
    Vec::new()
}

#[cfg(not(target_arch = "wasm32"))]
mod native;

#[cfg(not(target_arch = "wasm32"))]
pub use native::Grok;

#[cfg(target_arch = "wasm32")]
mod wasm_mode;
