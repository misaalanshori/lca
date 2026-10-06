//! OpenAI Codex models via ChatGPT subscription login (gh #180):
//! the loopback OAuth flow, the Codex responses endpoint, a static
//! model table. The flow and the Responses protocol live in the
//! subscription kit; this crate is the spec table over it (gh #63).

use lca_protocol::ProviderCap;
use lca_subscription::{AccountStrategy, IdentityFailure, OAuthSpec, StreamFailure};

/// Everything about the Codex gateway that is data (port sources:
/// `~/gits/my-fx-fork/src/core/auth/chatgpt_oauth.zig` and
/// `src/gateway/openai_codex.zig`; pi's `openai-codex.ts` and
/// `openai-codex-responses.ts` where they map).
pub const SPEC: OAuthSpec = OAuthSpec {
    name: "codex",
    auth_endpoint: "https://auth.openai.com/oauth/authorize",
    token_endpoint: "https://auth.openai.com/oauth/token",
    revoke_endpoint: None,
    userinfo_endpoint: None,
    api_base: "https://chatgpt.com/backend-api",
    client_id: "app_EMoamEEZ73f0CkXaXp7hrann",
    scope: "openid profile email offline_access",
    extra_auth_params: &[
        ("id_token_add_organizations", "true"),
        ("codex_cli_simplified_flow", "true"),
        ("originator", "lca"),
    ],
    account: AccountStrategy::JwtClaim {
        claim: "https://api.openai.com/auth",
        field: "chatgpt_account_id",
    },
};

/// The inference path default (`api_base` overrides).
pub const RESPONSES_PATH: &str = "/codex/responses";

/// The grants the manifest declares (the Codex gateway, the loopback
/// flow, this provider's own credential namespace).
#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::expect_used)] // the extension's own literal manifest patterns are valid by construction.
pub fn manifest_grants() -> lca_tools::CapabilityGrants {
    lca_tools::CapabilityGrants {
        net: vec![
            lca_permissions::parse_net_pattern("chatgpt.com").expect("chatgpt.com parses"),
            lca_permissions::parse_net_pattern("auth.openai.com").expect("auth.openai.com parses"),
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

/// `logout`: clear the namespace (the gateway exposes no revoke for
/// subscription tokens).
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
pub(crate) fn headers(account_id: &str, token: &str) -> Vec<(String, String)> {
    vec![
        ("authorization".to_string(), format!("Bearer {token}")),
        ("chatgpt-account-id".to_string(), account_id.to_string()),
        ("originator".to_string(), "lca".to_string()),
        (
            "openai-beta".to_string(),
            "responses=experimental".to_string(),
        ),
        ("accept".to_string(), "text/event-stream".to_string()),
        ("content-type".to_string(), "application/json".to_string()),
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
            message: "no Codex account stored; run /login codex".to_string(),
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
    let body = lca_subscription::build_responses_body(request, &system, &request.model, effort);
    let body_bytes = serde_json::to_vec(&body).map_err(|err| StreamFailure {
        message: format!("cannot build request: {err}"),
        class: "invalid",
        retryable: false,
    })?;
    let url = format!(
        "{}{RESPONSES_PATH}",
        endpoint(cap, "api_base", SPEC.api_base).trim_end_matches('/')
    );
    let headers = headers(&account_id, &token);
    let refs: Vec<(&str, &str)> = headers
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    let mut driver = lca_subscription::ResponseStreamDriver::open(cap, &url, &refs, &body_bytes)?;
    while let Some(event) = driver.next_event() {
        if !emit(event?) {
            break;
        }
    }
    Ok(())
}

/// The static model table. One grounded row: `gpt-5.4-mini` with the
/// window fx's catalog fixture records (272000); the table grows with
/// livedata, never guesses.
pub fn list_models(_cap: &dyn ProviderCap) -> Vec<lca_protocol::ModelInfo> {
    vec![lca_protocol::ModelInfo {
        id: "gpt-5.4-mini".to_string(),
        name: "GPT 5.4 Mini (Codex)".to_string(),
        context_window: 272000,
        max_tokens: 0,
        extras: Default::default(),
    }]
}

/// The outcome alias identity returns on the success path.
pub type IdentityOutcomeAlias = lca_protocol::IdentityOutcome;

/// The manifest next to this source, so tests pin the file the
/// installer reads to the grants the native form carries.
pub const MANIFEST: &str = include_str!("../extension.toml");

/// The login options: none — Codex signs in through its identity flow
/// directly (the TUI routes a provider with no options to it, and a
/// check never launches a flow from an empty set).
pub fn login_options() -> Vec<lca_protocol::LoginOption> {
    Vec::new()
}

#[cfg(not(target_arch = "wasm32"))]
mod native;

#[cfg(not(target_arch = "wasm32"))]
pub use native::Codex;

#[cfg(target_arch = "wasm32")]
mod wasm_mode;
