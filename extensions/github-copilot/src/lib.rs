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
///
/// Gh #282: new writes go through [`parse_enterprise_domain`], so the
/// trimming below is for values stored by an older build (which accepted a
/// pasted `https://…/`). Kept rather than removed: a stored credential is
/// not migrated by an upgrade, and a bare `trim_start_matches` here is
/// cheaper than making an existing enterprise account re-enter its domain.
fn domain(cap: &dyn ProviderCap) -> String {
    let enterprise = stored(cap, "enterprise_domain").trim().to_string();
    if enterprise.is_empty() {
        GITHUB_DOMAIN_DEFAULT.to_string()
    } else {
        enterprise
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .trim_end_matches('/')
            // Hostnames are case-insensitive; the derived hosts and the
            // manifest patterns compare lowercase (gh #282).
            .to_ascii_lowercase()
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
    let enterprise = answer.value("enterprise-domain").unwrap_or("");
    if enterprise.is_empty() {
        let _ = cap.credentials_delete("enterprise_domain");
    } else {
        // Gh #282: validate before anything is stored or requested. A pasted
        // URL or a stray wildcard would otherwise reach a `net` pattern and a
        // URL, and the failure would surface as a fetch error far from the
        // field the user can fix.
        let domain = parse_enterprise_domain(enterprise)?;
        cap.credentials_set("enterprise_domain", &domain)
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
    let authorization = request_device_code(cap, &auth_url, CLIENT_ID, Some(SCOPE))
        .map_err(|err| enterprise_grant_failure(err, &domain))?;
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
        // Gh #282: the same derivation `enterprise_hosts` grants, so the
        // consented host and the requested host are one answer. Diverging
        // here is how a grant for `copilot-api.<d>` would not cover the
        // request or vice versa.
        return enterprise_inference_url(&domain);
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

/// What the enterprise field must look like (gh #282), one line the
/// host's message and this provider's own refusal share.
pub const ENTERPRISE_HOSTNAME_HELP: &str =
    "enter an enterprise hostname like `ghe.example.com` (no scheme, path, port, or wildcard)";

/// The same help, for the one shape a user most plausibly gets wrong.
///
/// pi repairs a pasted `https://host/` silently (`normalizeDomain` builds a
/// URL and keeps the hostname). LCA refuses instead, for a reason that is
/// not pedantry: pi also swallows a **port** (`ghe.corp.com:8443` →
/// `ghe.corp.com`), silently redirecting the sign-in to a different
/// endpoint than the one typed - and the port a user types is often the
/// one their instance actually serves. Refusing says so; repairing decides
/// for them. The copy names the corrected form so the fix is one keystroke.
pub const ENTERPRISE_URL_HELP: &str =
    "drop the `https://` and the trailing `/` - just the hostname";

/// Validate the `enterprise-domain` login field (gh #282): it is a
/// hostname, and nothing else.
///
/// The field feeds a URL (`https://{domain}/login/device/code`) and a
/// `net` pattern, so every shape that is not a bare hostname is a
/// mistake worth catching at the field, where the user can fix it in
/// place: a pasted `https://…` URL, a path, a port, stray whitespace, or
/// a user-supplied wildcard (a wildcard is a grant decision, never a
/// domain). Returns the lowercased hostname.
pub fn parse_enterprise_domain(raw: &str) -> Result<String, String> {
    if raw.is_empty() {
        return Err(format!(
            "no enterprise hostname given; {ENTERPRISE_HOSTNAME_HELP}"
        ));
    }
    let refuse =
        |why: &str| format!("`{raw}` is not a hostname: {why}; {ENTERPRISE_HOSTNAME_HELP}");
    // Whitespace is refused rather than trimmed: this value becomes a URL
    // and a `net` pattern, and silently "fixing" what the user typed is how
    // a domain they did not mean gets consented to. The message names the
    // fix instead.
    if raw.chars().any(char::is_whitespace) {
        return Err(refuse("it contains whitespace - remove the spaces"));
    }
    if raw.contains("://") {
        return Err(format!(
            "`{raw}` is not a hostname: it carries a scheme - {ENTERPRISE_URL_HELP}; \
             {ENTERPRISE_HOSTNAME_HELP}"
        ));
    }
    if raw.contains('/') {
        return Err(refuse("it carries a path"));
    }
    if raw.contains('@') {
        return Err(refuse("it carries user info"));
    }
    if raw.contains(':') {
        return Err(refuse("it carries a port"));
    }
    if raw.contains('*') {
        return Err(refuse("a wildcard is a grant decision, not a domain"));
    }
    let domain = raw.to_ascii_lowercase();
    // A hostname is dot-separated labels of `[a-z0-9-]`, each 1..=63
    // characters, not starting or ending with a hyphen, with at least two
    // labels (an enterprise account always names a real domain).
    let labels: Vec<&str> = domain.split('.').collect();
    if labels.len() < 2 {
        return Err(refuse("an enterprise domain needs at least two labels"));
    }
    for label in &labels {
        if label.is_empty() {
            return Err(refuse("it has an empty label"));
        }
        if label.len() > 63 {
            return Err(refuse("a label is longer than 63 characters"));
        }
        if label.starts_with('-') || label.ends_with('-') {
            return Err(refuse("a label starts or ends with a hyphen"));
        }
        if !label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return Err(refuse("it contains a character a hostname cannot carry"));
        }
    }
    Ok(domain)
}

/// The ad-hoc `net` hosts an enterprise `domain` needs (gh #282), in the
/// order the flow reaches them: the domain itself (device code and token
/// exchange), `api.<domain>` (the Copilot token mint), and
/// `copilot-api.<domain>` (inference, pi's `getGitHubCopilotBaseUrl`
/// enterprise fallback).
///
/// Empty for the default domain and for any host the manifest already
/// declares: an enterprise account is the only case that reaches a host
/// the shipped manifest cannot know, and asking for one that is already
/// granted would be a prompt for a decision the user has made.
///
/// One derivation feeds both the grant offer and `api_base`, so the hosts
/// consented to and the hosts requested cannot drift (gh #282's other
/// half is exactly that drift).
#[must_use]
pub fn enterprise_hosts(domain: &str) -> Vec<String> {
    let domain = domain.trim().trim_end_matches('.').to_ascii_lowercase();
    if domain.is_empty() || domain == GITHUB_DOMAIN_DEFAULT {
        return Vec::new();
    }
    let mut hosts = Vec::new();
    for candidate in [
        domain.clone(),
        format!("api.{domain}"),
        format!("copilot-api.{domain}"),
    ] {
        if is_declared_host(&candidate) {
            continue;
        }
        if !hosts.contains(&candidate) {
            hosts.push(candidate);
        }
    }
    hosts
}

/// Turn a refusal at the device-code request into the fix (gh #282).
///
/// An enterprise domain's hosts are field data, so the `net` offer that
/// runs after a successful submit never gets the chance to name them: the
/// submit's own first request is what gets denied, and the flow has no
/// recovery from inside the sandbox. What the user needs is the three host
/// names and the command that grants them - not `matches no granted
/// pattern` with no way forward.
///
/// Everything that is not a grant refusal passes through untouched; a 404
/// from a live enterprise host, a DNS failure and a malformed answer are
/// all real answers and must read as themselves.
fn enterprise_grant_failure(err: IdentityFailure, domain: &str) -> IdentityFailure {
    if domain == GITHUB_DOMAIN_DEFAULT {
        return err;
    }
    let denied = err.0.contains("permission denied") || err.0.contains("not granted");
    if !denied {
        return err;
    }
    let hosts = enterprise_hosts(domain);
    if hosts.is_empty() {
        return err;
    }
    let allow = hosts
        .iter()
        .map(|host| format!("--allow-host {host}"))
        .collect::<Vec<_>>()
        .join(" ");
    IdentityFailure(format!(
        "{} - this account's enterprise hosts are not granted. Sign in from an \
         interactive `lca` (it offers them for consent), or grant them for one run \
         with `{allow}`; `lca /grants` lists what is stored.",
        err.0
    ))
}

/// The hosts this provider's manifest declares, as text (gh #282).
///
/// This derivation runs inside the component as well as natively, so it
/// cannot use `manifest_grants` (native-only, and it needs
/// `lca-permissions`) or `lca_ext_host` (not a guest dependency). The list
/// is therefore spelled once here and `the_declared_hosts_match_the_manifest`
/// pins it against `extension.toml` in both directions, so the two cannot
/// drift without a test failing.
pub const DECLARED_HOSTS: [&str; 6] = [
    "github.com",
    "api.github.com",
    "api.githubcopilot.com",
    "api.individual.githubcopilot.com",
    "api.business.githubcopilot.com",
    "api.enterprise.githubcopilot.com",
];

/// Whether `host` is one the manifest already declares - exact or a
/// wildcard suffix, with no port, which is all this provider's manifest
/// uses. Kept beside [`DECLARED_HOSTS`] so the rule has one home.
#[must_use]
pub fn is_declared_host(host: &str) -> bool {
    DECLARED_HOSTS.iter().any(|declared| {
        if let Some(suffix) = declared.strip_prefix('*') {
            // A manifest wildcard is stored with its leading dot.
            host.ends_with(suffix) && host.len() > suffix.len()
        } else {
            host == *declared
        }
    })
}

/// The inference URL an enterprise `domain` uses (gh #282): pi's
/// `getGitHubCopilotBaseUrl` enterprise fallback. Shared with
/// [`enterprise_hosts`] so the granted host is the requested host.
fn enterprise_inference_url(domain: &str) -> String {
    format!("https://copilot-api.{domain}")
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
    let initiator = request
        .messages
        .last()
        .map(|m| {
            if m.role == lca_protocol::MessageRole::User {
                "user"
            } else {
                "agent"
            }
        })
        .unwrap_or("user");
    let has_images = request.messages.iter().any(|m| {
        m.content
            .iter()
            .any(|block| matches!(block, lca_protocol::ContentBlock::Image { .. }))
    });
    let mut headers: Vec<(String, String)> = vec![
        ("content-type".to_string(), "application/json".to_string()),
        ("authorization".to_string(), bearer),
        (
            "X-GitHub-Api-Version".to_string(),
            COPILOT_API_VERSION.to_string(),
        ),
        ("X-Initiator".to_string(), initiator.to_string()),
        (
            "Openai-Intent".to_string(),
            "conversation-edits".to_string(),
        ),
    ];
    if has_images {
        headers.push(("Copilot-Vision-Request".to_string(), "true".to_string()));
    }
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
/// Parse the gateway's `/models` answer into rows, applying gh #184's
/// filters (tool-less and policy-disabled rows never list) and keeping
/// every field the provider can honestly use (gh #283).
fn parse_models(text: &str) -> Vec<LiveRow> {
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
            rows.push(LiveRow {
                id: id.to_string(),
                ..LiveRow::from_entry(item)
            });
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
///
/// Gh #283: a live row's numbers come from `resolve_row`, not from a
/// hardcoded zero. A zero window means the footer shows `ctx ?` and the
/// FR-SESS-4 threshold falls back to `docs/compaction.md`'s 128k
/// estimate - right only for a model whose real window is 128k. Copilot's
/// curated figures are 200k, so stamping zero on every row gave those
/// sessions a threshold far below the real one.
pub fn list_models(cap: &dyn ProviderCap) -> Vec<lca_protocol::ModelInfo> {
    if let Ok(token) = access_token(cap)
        && let Some(live) = discover_models(cap, &token)
        && !live.is_empty()
    {
        return live.into_iter().map(resolve_row).collect();
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

/// One discovered row, with its numbers resolved (gh #283).
///
/// Precedence, mirroring `docs/providers/openai-compatible.md`'s
/// four-source order adapted to what this gateway publishes:
///
/// 1. **The curated table** wins for an id it knows. Its figures are
///    vendor-documented; the endpoint's are the account's own view and
///    can be a narrower tier. A wrong denominator silently mis-triggers
///    compaction, which that page calls out as worse than a missing one.
/// 2. **The endpoint's declared window** when the table does not know the
///    id - `capabilities.limits.max_context_window_tokens`, falling back
///    to `max_prompt_tokens` when only that is present (opencode's
///    decoder reads the pair the same way).
/// 3. **`0`** otherwise, honestly. The footer renders `ctx ?` and the
///    compaction estimate governs; a fabricated number is the one thing
///    this must never do.
///
/// Everything non-structural (display name, modality hints) rides
/// `extras` under the vocabulary the tree already reads, so no ABI
/// change: `image.vision` from `capabilities.supports.vision` and
/// `thinking_levels` from `capabilities.supports.reasoning_effort`.
/// A hint the endpoint omits stays *absent* (unknown), never `false`.
fn resolve_row(row: LiveRow) -> lca_protocol::ModelInfo {
    let curated = CURATED_MODELS.iter().find(|(id, _, _)| *id == row.id);
    let window = curated
        .map(|(_, _, window)| *window)
        .or_else(|| row.declared_window())
        .unwrap_or(0);
    let mut extras = std::collections::BTreeMap::new();
    if let Some(vision) = row.vision {
        extras.insert(
            lca_protocol::IMAGE_VISION_EXTRA.to_string(),
            vision.to_string(),
        );
    }
    if !row.reasoning_effort.is_empty() {
        extras.insert(
            lca_protocol::THINKING_LEVELS_EXTRA.to_string(),
            row.reasoning_effort.join(","),
        );
    }
    lca_protocol::ModelInfo {
        name: match (curated, row.name) {
            (Some((_, curated_name, _)), _) => (*curated_name).to_string(),
            (None, Some(name)) if !name.is_empty() => name,
            // No name from either source: the id, never a blank row.
            (None, _) => row.id.clone(),
        },
        id: row.id,
        context_window: window,
        max_tokens: row.max_output_tokens.unwrap_or(0),
        extras,
    }
}

/// One row of the gateway's `/models` answer, keeping the fields this
/// provider can honestly use (`LiveRow::from_entry`).
#[derive(Debug, Clone, Default)]
struct LiveRow {
    /// The model id the gateway keys on.
    id: String,
    /// The endpoint's display name, when it supplies one.
    name: Option<String>,
    /// `capabilities.limits.max_context_window_tokens`.
    max_context_window_tokens: Option<u32>,
    /// `capabilities.limits.max_prompt_tokens` - the window fallback.
    max_prompt_tokens: Option<u32>,
    /// `capabilities.limits.max_output_tokens`.
    max_output_tokens: Option<u32>,
    /// `capabilities.supports.vision`, absent when unreported.
    vision: Option<bool>,
    /// `capabilities.supports.reasoning_effort`, empty when unreported.
    reasoning_effort: Vec<String>,
}

impl LiveRow {
    /// The endpoint's window for this row, if it declared one.
    fn declared_window(&self) -> Option<u32> {
        self.max_context_window_tokens
            .filter(|tokens| *tokens > 0)
            .or(self.max_prompt_tokens.filter(|tokens| *tokens > 0))
    }

    /// Read the fields this provider uses off one `/models` entry.
    ///
    /// The paths are GitHub's, not guesses: `capabilities.limits.
    /// max_context_window_tokens` / `max_prompt_tokens` /
    /// `max_output_tokens` and `capabilities.supports.vision` /
    /// `reasoning_effort` are what the live API answers and what
    /// opencode's `github-copilot/models.ts` decodes. A missing field
    /// stays `None`, so "the endpoint did not say" never turns into a
    /// number or a `false`.
    fn from_entry(item: &serde_json::Value) -> Self {
        fn number(item: &serde_json::Value, path: &[&str]) -> Option<u32> {
            let mut cursor = item;
            for key in path {
                cursor = cursor.get(*key)?;
            }
            cursor.as_u64().and_then(|value| u32::try_from(value).ok())
        }
        let capabilities = item.get("capabilities");
        let field = |path: &[&str]| -> Option<&serde_json::Value> {
            let mut cursor = capabilities?;
            for key in path {
                cursor = cursor.get(*key)?;
            }
            Some(cursor)
        };
        LiveRow {
            id: item
                .get("id")
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_string(),
            name: item
                .get("name")
                .and_then(|value| value.as_str())
                .map(str::to_string)
                .filter(|name| !name.is_empty()),
            max_context_window_tokens: number(
                item,
                &["capabilities", "limits", "max_context_window_tokens"],
            ),
            max_prompt_tokens: number(item, &["capabilities", "limits", "max_prompt_tokens"]),
            max_output_tokens: number(item, &["capabilities", "limits", "max_output_tokens"]),
            vision: field(&["supports", "vision"]).and_then(serde_json::Value::as_bool),
            reasoning_effort: field(&["supports", "reasoning_effort"])
                .and_then(serde_json::Value::as_array)
                .map(|levels| {
                    levels
                        .iter()
                        .filter_map(serde_json::Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
        }
    }
}

/// One bounded catalog read over the token-derived base.
fn discover_models(cap: &dyn ProviderCap, token: &str) -> Option<Vec<LiveRow>> {
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

#[cfg(test)]
mod enterprise_grant_failure_tests {
    use super::{ENTERPRISE_HOSTNAME_HELP, enterprise_grant_failure, enterprise_hosts};
    use lca_subscription::IdentityFailure;

    // Verifies: gh #282 - grant advice is only for grant refusals. A live
    // enterprise host that answers 404, or fails DNS, must read as its own
    // answer: rewriting it as "your hosts are not granted" would send the
    // user to fix something that is not broken.
    //
    // A unit row rather than an integration one: exercising this through
    // `login_submit` would mean a real request to a fake domain, which
    // hangs on DNS instead of failing fast.
    #[test]
    fn a_non_grant_failure_is_left_alone() {
        for message in [
            "device authorization failed: 404 Not Found",
            "dns: failed to resolve ghe.corp.com",
            "tls: certificate error",
            "invalid device code response",
        ] {
            let out =
                enterprise_grant_failure(IdentityFailure(message.to_string()), "ghe.corp.com");
            assert_eq!(
                out.0, message,
                "`{message}` is not a consent failure and must pass through"
            );
        }
    }

    // Verifies: gh #282 - a grant refusal on an enterprise domain carries
    // every derived host and the command that grants it.
    #[test]
    fn a_grant_refusal_on_an_enterprise_domain_names_the_fix() {
        let err = IdentityFailure(
            "permission denied: ghe.corp.com:443 matches no granted pattern".to_string(),
        );
        let out = enterprise_grant_failure(err, "ghe.corp.com");
        for host in enterprise_hosts("ghe.corp.com") {
            assert!(
                out.0.contains(&format!("--allow-host {host}")),
                "the fix for `{host}` is spelled out: {}",
                out.0
            );
        }
        assert!(
            out.0.contains("interactive"),
            "and it points at the path that offers consent: {}",
            out.0
        );
        assert!(
            out.0.contains("matches no granted pattern"),
            "the original refusal is preserved: {}",
            out.0
        );
    }

    // Verifies: gh #282 - the default domain's failures are never rewritten
    // (acceptance 2): nothing to grant, so nothing to advise.
    #[test]
    fn the_default_domain_is_never_rewritten() {
        let err = IdentityFailure(
            "permission denied: github.com:443 matches no granted pattern".to_string(),
        );
        let out = enterprise_grant_failure(err.clone(), "github.com");
        assert_eq!(out.0, err.0, "github.com behaves exactly as before");
        assert!(!out.0.contains("--allow-host"));
    }

    // Verifies: gh #282 - the help line is a hostname instruction, so the
    // refusal teaches rather than merely rejects.
    #[test]
    fn the_field_help_names_a_hostname() {
        assert!(ENTERPRISE_HOSTNAME_HELP.contains("hostname"));
        assert!(ENTERPRISE_HOSTNAME_HELP.contains("no scheme"));
    }
}
