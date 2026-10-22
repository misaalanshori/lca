//! The OAuth flow for remote MCP servers (gh #53 phase 2; split out at gh #418
//! Phase D so discovery, sign-in, exchange, and refresh keep their own file, as
//! pi's `packages/mcp/src/oauth/` does).
//!
//! Everything here runs through the capability engine, and every request is
//! **flow-scoped**: a host discovered from metadata is reachable while the flow
//! runs and gone when it ends (gh #418 Phase D, gh #290).
//!
//! The items stay re-exported from [`crate::remote`], so callers see no change.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::remote::percent_encode;
use crate::remote::{
    AttemptOutcome, DEFAULT_TOKEN_LIFETIME_SECS, TokenStore, get_document, now_ms, pkce_pair,
    post_attempt, with_timeout,
};
use crate::{HttpServerConfig, StoredToken, merge_scopes};

/// The authorization server behind one MCP server (RFC 8414's
/// document, however it was found).
struct IdpMeta {
    authorization_endpoint: String,
    token_endpoint: String,
    registration_endpoint: Option<String>,
}

/// The hosts this discovery must reach, and where they came from.
impl IdpMeta {
    /// Every host the flow will talk to: the authorization endpoint, the
    /// token endpoint, and the registration endpoint when it exists.
    fn hosts(&self) -> Vec<String> {
        let mut hosts = Vec::new();
        for url in [
            Some(&self.authorization_endpoint),
            Some(&self.token_endpoint),
            self.registration_endpoint.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            if let Some(host) = flow_grant_for(url)
                && !hosts.contains(&host)
            {
                hosts.push(host);
            }
        }
        hosts
    }

    /// The sign-in copy naming any host outside the MCP server's own, so the
    /// user sees where the credential goes (gh #418 Phase D's consent
    /// property). Empty when the authorization server is the MCP server
    /// itself.
    pub(crate) fn via_report(&self, server: &HttpServerConfig, hosts: &[String]) -> String {
        let own = flow_grant_for(&server.url);
        let outside: Vec<&String> = hosts
            .iter()
            .filter(|host| Some(*host) != own.as_ref())
            .collect();
        if outside.is_empty() {
            return String::new();
        }
        let named: Vec<String> = outside.iter().map(|host| (*host).clone()).collect();
        format!(
            "via {}: authorization host {}",
            server.name,
            named.join(", ")
        )
    }
}

impl IdpMeta {
    fn from_document(document: &serde_json::Value) -> Result<IdpMeta, String> {
        let string = |key: &str| {
            document
                .get(key)
                .and_then(|value| value.as_str())
                .map(str::to_string)
        };
        let authorization_endpoint = string("authorization_endpoint")
            .ok_or_else(|| "the metadata names no authorization endpoint".to_string())?;
        let token_endpoint = string("token_endpoint")
            .ok_or_else(|| "the metadata names no token endpoint".to_string())?;
        let registration_endpoint = string("registration_endpoint");
        // gh #418 Phase D: validate every endpoint the flow will use, before
        // it uses any of them.
        checked_endpoint(&authorization_endpoint, "the authorization endpoint")?;
        checked_endpoint(&token_endpoint, "the token endpoint")?;
        if let Some(endpoint) = &registration_endpoint {
            checked_endpoint(endpoint, "the registration endpoint")?;
        }
        Ok(IdpMeta {
            authorization_endpoint,
            token_endpoint,
            registration_endpoint,
        })
    }
}

/// One URL split into its scheme and host, or an explanation of why the
/// endpoint is not acceptable.
///
/// gh #418 Phase D's validation, in one place both directions of discovery
/// go through: a bearer token may only travel to `https://`, or to plain
/// `http://` on loopback (RFC 8252, and how a local IdP is reached on a
/// developer machine). Everything else - a public `http://` host, a
/// `file:`/`data:` URL, a scheme we do not speak - is refused here, before
/// any request leaves.
pub(crate) fn checked_endpoint(url: &str, what: &str) -> Result<(String, String), String> {
    let (scheme, rest) = url
        .split_once("://")
        .ok_or_else(|| format!("{what} is not an absolute URL: {url:?}"))?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "https" && scheme != "http" {
        return Err(format!(
            "{what} uses the unsupported scheme {scheme:?}: {url:?} (OAuth endpoints are https, or http on loopback)"
        ));
    }
    let authority = rest.split('/').next().unwrap_or("");
    // Strip userinfo, then the port: the host is what a grant is keyed by.
    let host_port = authority.rsplit('@').next().unwrap_or(authority);
    let host = if host_port.starts_with('[') {
        // An IPv6 literal keeps its brackets: `[::1]:8080` -> `[::1]`.
        host_port
            .split_once(']')
            .map(|(inside, _)| format!("{inside}]"))
            .unwrap_or_else(|| host_port.to_string())
    } else {
        host_port.split(':').next().unwrap_or(host_port).to_string()
    };
    if host.is_empty() {
        return Err(format!("{what} names no host: {url:?}"));
    }
    if scheme == "http" && !is_loopback_host(&host) {
        return Err(format!(
            "{what} must use https (refused {url:?}: http is only allowed on loopback)"
        ));
    }
    Ok((scheme, host))
}

/// Whether a host is loopback by name or literal. `*.localhost` resolves to
/// loopback on every platform we ship.
fn is_loopback_host(host: &str) -> bool {
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    bare.eq_ignore_ascii_case("localhost")
        || bare.ends_with(".localhost")
        || bare
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// `host:port` of a URL, the shape a flow-scoped grant needs.
///
/// A `NetPattern` without a port means 443 (`matches`), so a loopback IdP on
/// an ephemeral port would be granted nothing by a bare host. Configuring
/// the port is the precise grant, and these are hosts the user's own
/// metadata named.
fn flow_grant_for(url: &str) -> Option<String> {
    let (scheme, host) = checked_endpoint(url, "an OAuth endpoint").ok()?;
    let authority = url.split_once("://")?.1.split('/').next().unwrap_or("");
    let host_port = authority.rsplit('@').next().unwrap_or(authority);
    // An IPv6 literal keeps its brackets and its own colons.
    let port = if host_port.starts_with('[') {
        host_port
            .split_once("]:")
            .and_then(|(_, port)| port.parse::<u16>().ok())
    } else {
        host_port
            .split_once(':')
            .and_then(|(_, port)| port.parse().ok())
    };
    let port = port.unwrap_or(if scheme == "https" { 443 } else { 80 });
    Some(format!("{host}:{port}"))
}

/// The origin (`scheme://host[:port]`) of a URL, userinfo stripped.
///
/// gh #467: the first version split the whole URL on `/` with `splitn(4, '/')`
/// and rejected the **second** piece. For `http://h:1/mcp` that piece is the
/// empty string *between the two slashes of `//`* - empty for every well-formed
/// URL - so the guard fired on exactly the input it was written for and
/// `origin_of` could never succeed. It survived because it is the last resort
/// in [`discover`] and nothing tested the branch that reaches it.
///
/// So this parses instead of pattern-matching: scheme, then authority, then
/// cut the authority at the path/query/fragment. `checked_endpoint` already
/// knows how to read an authority (userinfo, an IPv6 literal's brackets, the
/// port), and an origin that disagrees with it about "which host is this"
/// would be a second, quietly different answer to the same question.
///
/// The scheme is lowercased because a grant lookup is an exact string compare:
/// `HTTP://h` must not reach a host check as a different scheme.
fn origin_of(url: &str) -> Result<String, String> {
    let (scheme, rest) = url
        .split_once("://")
        .ok_or_else(|| format!("cannot take the origin of {url:?}: it has no scheme"))?;
    let scheme = scheme.to_ascii_lowercase();
    // The authority ends at the path, the query, or the fragment - whichever
    // comes first. `split('/')` alone would keep `example.com?q=1` whole.
    let authority = rest.find(['/', '?', '#']).map_or(rest, |cut| &rest[..cut]);
    // `checked_endpoint` validates the shape (a scheme we speak, plain http only
    // on loopback) and reads the host past any userinfo; `rsplit('@')` is where
    // the credential is dropped.
    let (_, host) = checked_endpoint(url, "the MCP origin")?;
    if authority.is_empty() || host.is_empty() {
        return Err(format!(
            "cannot take the origin of {url:?}: it names no host"
        ));
    }
    // Keep the port: an IdP on an ephemeral loopback port is exactly the case
    // this fallback serves, and `flow_grant_for` keys its grant by host *and*
    // port for the same reason.
    let authority_no_user = authority.rsplit('@').next().unwrap_or(authority);
    Ok(format!("{scheme}://{authority_no_user}"))
}

/// Find the authorization server: the configured metadata document
/// wins (pi's `authServerMetadataUrl`), else the challenge's
/// `resource_metadata` (RFC 9728), else the MCP origin as its own
/// issuer.
fn discover(
    caps: Arc<lca_tools::Capabilities>,
    server: &HttpServerConfig,
    challenge: &BTreeMap<String, String>,
) -> Result<IdpMeta, String> {
    if let Some(url) = server
        .oauth
        .as_ref()
        .and_then(|oauth| oauth.auth_server_metadata_url.clone())
    {
        return IdpMeta::from_document(&get_document(caps, server.timeout(), &url)?);
    }
    if let Some(metadata_url) = challenge.get("resource_metadata") {
        let document = get_document(caps.clone(), server.timeout(), metadata_url)?;
        let issuer = document
            .get("authorization_servers")
            .and_then(|servers| servers.as_array())
            .and_then(|servers| servers.first())
            .and_then(|first| first.as_str())
            .ok_or_else(|| "the resource metadata names no authorization server".to_string())?;
        let url = format!(
            "{}/.well-known/oauth-authorization-server",
            issuer.trim_end_matches('/')
        );
        return IdpMeta::from_document(&get_document(caps, server.timeout(), &url)?);
    }
    let url = format!(
        "{}/.well-known/oauth-authorization-server",
        origin_of(&server.url)?
    );
    IdpMeta::from_document(&get_document(caps, server.timeout(), &url)?)
}

/// Register this client (RFC 7591): the name the server sees, the
/// loopback redirect, and the requested scopes.
fn register_client(
    caps: Arc<lca_tools::Capabilities>,
    server: &HttpServerConfig,
    meta: &IdpMeta,
    redirect_uri: &str,
    scope: Option<&str>,
) -> Result<(String, Option<String>), String> {
    let endpoint = meta.registration_endpoint.clone().ok_or_else(|| {
        "the server supports no dynamic registration; configure a client id (phase 3)".to_string()
    })?;
    let client_name = server
        .oauth
        .as_ref()
        .and_then(|oauth| oauth.client_name.clone())
        .unwrap_or_else(|| "lca".to_string());
    let mut registration = serde_json::json!({
        "client_name": client_name,
        "redirect_uris": [redirect_uri],
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
    });
    if let Some(scope) = scope {
        registration["scope"] = scope.into();
    }
    let body = serde_json::to_vec(&registration)
        .map_err(|err| format!("cannot encode the registration: {err}"))?;
    let caps_moved = caps.clone();
    let endpoint_moved = endpoint.clone();
    let attempt = with_timeout(server.timeout(), "the client registration", move || {
        let headers = vec![("Content-Type".to_string(), "application/json".to_string())];
        match post_attempt(&caps_moved, &endpoint_moved, &headers, &body) {
            AttemptOutcome::Exchanged(attempt) => Ok(attempt),
            AttemptOutcome::Retryable(detail) | AttemptOutcome::Denied(detail) => Err(detail),
        }
    })?;
    if attempt.status != 200 && attempt.status != 201 {
        return Err(format!(
            "registration answered {}: {}",
            attempt.status,
            String::from_utf8_lossy(&attempt.body)
        ));
    }
    let answer: serde_json::Value = serde_json::from_slice(&attempt.body)
        .map_err(|err| format!("bad registration JSON: {err}"))?;
    let client_id = answer
        .get("client_id")
        .and_then(|id| id.as_str())
        .ok_or_else(|| "registration answered no client id".to_string())?
        .to_string();
    let client_secret = answer
        .get("client_secret")
        .and_then(|secret| secret.as_str())
        .map(str::to_string);
    Ok((client_id, client_secret))
}

/// The interactive sign-in (the anthropic browser flow's shape):
/// register (or reuse the configured client), open the authorize URL,
/// wait for the pasted or looped-back code, exchange, store. The
/// requested scopes are the configured ones plus any stored grant
/// (step-up merges, never replaces).
pub fn sign_in(
    caps: Arc<lca_tools::Capabilities>,
    server: &HttpServerConfig,
) -> Result<(), String> {
    let store = TokenStore::new(caps.clone(), server);
    let stored = store.load()?;
    // gh #418 Phase D: the challenge the 401 captured - including
    // `resource_metadata`, which is the only pointer to an authorization
    // server on another host. It used to be passed as an empty map, so the
    // RFC 9728 branch could never run during sign-in.
    let challenge = store.pending_challenge()?.unwrap_or_default();
    let scope = merge_scopes(&[
        server
            .oauth
            .as_ref()
            .and_then(|oauth| oauth.scope.as_deref()),
        stored.as_ref().and_then(|token| token.scope.as_deref()),
        challenge.get("scope").map(String::as_str),
    ]);
    // A server that answered 401 names what it needs; without a
    // challenge the configured (plus stored) scopes are the request.
    let (redirect_uri, flow) = caps
        .oauth_begin("/callback")
        .map_err(|err| format!("cannot start the loopback flow: {err}"))?;
    // The flow-scoped hosts are cleared on the way out, whatever happened:
    // a discovered host must not outlive the flow that needed it.
    let outcome = sign_in_flow(caps.clone(), server, challenge, scope, redirect_uri, flow);
    caps.oauth_flow_clear();
    outcome
}

/// The body of [`sign_in`] once the loopback flow is bound, split out so the
/// flow-scoped host set is always cleared exactly once.
fn sign_in_flow(
    caps: Arc<lca_tools::Capabilities>,
    server: &HttpServerConfig,
    challenge: BTreeMap<String, String>,
    scope: Option<String>,
    redirect_uri: String,
    flow: u32,
) -> Result<(), String> {
    // Discovery itself has to reach a host the static grants do not name (the
    // configured metadata document, or the one the 401 challenge pointed at),
    // so those go in first. The static grant list names the MCP server alone;
    // the hosts here are admitted for this flow only (gh #418 Phase D).
    let mut flow_hosts = Vec::new();
    let mut seeded = Vec::new();
    if let Some(url) = server
        .oauth
        .as_ref()
        .and_then(|oauth| oauth.auth_server_metadata_url.as_deref())
    {
        seeded.push(url);
    }
    if let Some(url) = challenge.get("resource_metadata") {
        seeded.push(url.as_str());
    }
    for url in seeded {
        // The URL is validated by `get_document` before the request; here we
        // only need its host, and a malformed one simply contributes none.
        if let Some(host) = flow_grant_for(url)
            && !flow_hosts.contains(&host)
        {
            flow_hosts.push(host);
        }
    }
    caps.oauth_flow_attach(flow_hosts.clone())
        .map_err(|err| format!("cannot admit the discovery hosts: {err}"))?;
    let meta = discover(caps.clone(), server, &challenge)?;
    for host in meta.hosts() {
        if !flow_hosts.contains(&host) {
            flow_hosts.push(host);
        }
    }
    caps.oauth_flow_attach(meta.hosts())
        .map_err(|err| format!("cannot admit the OAuth hosts: {err}"))?;
    // Name the discovered host(s) that are not the MCP server's own, so the
    // sign-in report says where the credential is about to go.
    let store = TokenStore::new(caps.clone(), server);
    let via = meta.via_report(server, &flow_hosts);
    if let Err(err) = store.set_discovery_note(&via) {
        // A note is informative; never fail the flow over it.
        let _ = err;
    }

    let (verifier, challenge) = pkce_pair().map_err(|err| format!("PKCE failed: {err}"))?;
    // pi sends the verifier as the state; the equality check then
    // covers both values at once.
    let state = verifier.clone();
    let (client_id, client_secret) = match server
        .oauth
        .as_ref()
        .and_then(|oauth| oauth.client_id.clone())
    {
        Some(configured) => {
            // gh #418 Phase F: a secret `lca mcp add` stored lives in the
            // credential namespace, not in `mcp.json`; the config file wins
            // when it carries one (a hand-edited file is an explicit choice).
            let from_config = server
                .oauth
                .as_ref()
                .and_then(|oauth| oauth.client_secret.clone());
            let secret = match from_config {
                Some(secret) => Some(secret),
                None => TokenStore::new(caps.clone(), server).client_secret()?,
            };
            (configured, secret)
        }
        None => register_client(caps.clone(), server, &meta, &redirect_uri, scope.as_deref())?,
    };
    let encode = percent_encode;
    let mut url = format!(
        "{}?response_type=code&client_id={}&redirect_uri={}&code_challenge={}&code_challenge_method=S256&state={}",
        meta.authorization_endpoint,
        encode(&client_id),
        encode(&redirect_uri),
        encode(&challenge),
        encode(&state),
    );
    if let Some(scope) = &scope {
        url.push_str(&format!("&scope={}", encode(scope)));
    }
    // A refusal here would wait out the whole callback window with no
    // explanation, so it is reported (gh #418 Phase D: any loopback http or
    // https is acceptable, and anything else should say why).
    if let Err(err) = caps.oauth_open(&url) {
        return Err(format!("cannot open the authorization page: {err}"));
    }
    let callback = caps.oauth_await(flow);
    let _ = caps.oauth_end(flow);
    let callback = callback.map_err(|err| err.to_string())?;
    let get = |key: &str| {
        callback
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.clone())
            .unwrap_or_default()
    };
    if get("state") != state {
        return Err("OAuth state mismatch; the callback did not come from this flow".to_string());
    }
    let code = get("code");
    if code.is_empty() {
        return Err("the authorization server returned no code".to_string());
    }
    exchange(
        caps.clone(),
        server,
        &meta,
        &store,
        &client_id,
        client_secret.as_deref(),
        &code,
        &redirect_uri,
        &verifier,
        scope.as_deref(),
    )
}

/// Exchange a code (or a refresh token) at the token endpoint; the
/// `grant` closure shapes the form body per grant type.
fn token_post(
    caps: Arc<lca_tools::Capabilities>,
    server: &HttpServerConfig,
    token_url: &str,
    pairs: &[(&str, &str)],
) -> Result<serde_json::Value, String> {
    let encoded = pairs
        .iter()
        .map(|(key, value)| format!("{}={}", percent_encode(key), percent_encode(value)))
        .collect::<Vec<_>>()
        .join("&");
    let token_url = token_url.to_string();
    let attempt = with_timeout(server.timeout(), "the token exchange", move || {
        let headers = vec![(
            "Content-Type".to_string(),
            "application/x-www-form-urlencoded".to_string(),
        )];
        match post_attempt(&caps, &token_url, &headers, encoded.as_bytes()) {
            AttemptOutcome::Exchanged(attempt) => Ok(attempt),
            AttemptOutcome::Retryable(detail) | AttemptOutcome::Denied(detail) => Err(detail),
        }
    })?;
    if attempt.status != 200 {
        return Err(format!(
            "token endpoint answered {}: {}",
            attempt.status,
            String::from_utf8_lossy(&attempt.body)
        ));
    }
    serde_json::from_slice(&attempt.body).map_err(|err| format!("bad token JSON: {err}"))
}

/// Store an issuance (code or refresh grant) against its client.
fn store_issuance(
    store: &TokenStore,
    answer: &serde_json::Value,
    client_id: &str,
    client_secret: Option<&str>,
    token_url: &str,
    fallback_scope: Option<&str>,
) -> Result<(), String> {
    let access_token = answer
        .get("access_token")
        .and_then(|token| token.as_str())
        .ok_or_else(|| "the token endpoint issued no access token".to_string())?
        .to_string();
    let lifetime = answer
        .get("expires_in")
        .and_then(|secs| secs.as_u64())
        .unwrap_or(DEFAULT_TOKEN_LIFETIME_SECS);
    store.save(&StoredToken {
        access_token,
        refresh_token: answer
            .get("refresh_token")
            .and_then(|token| token.as_str())
            .map(str::to_string),
        expires_at_ms: Some(now_ms() + lifetime * 1000),
        client_id: client_id.to_string(),
        client_secret: client_secret.map(str::to_string),
        scope: answer
            .get("scope")
            .and_then(|scope| scope.as_str())
            .map(str::to_string)
            .or_else(|| fallback_scope.map(str::to_string)),
        token_url: token_url.to_string(),
    })
}

/// The code exchange at the end of [`sign_in`].
#[allow(clippy::too_many_arguments)]
fn exchange(
    caps: Arc<lca_tools::Capabilities>,
    server: &HttpServerConfig,
    meta: &IdpMeta,
    store: &TokenStore,
    client_id: &str,
    client_secret: Option<&str>,
    code: &str,
    redirect_uri: &str,
    verifier: &str,
    scope: Option<&str>,
) -> Result<(), String> {
    let mut pairs = vec![
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", redirect_uri),
        ("client_id", client_id),
        ("code_verifier", verifier),
    ];
    let secret;
    if let Some(secret_value) = client_secret {
        secret = secret_value.to_string();
        pairs.push(("client_secret", secret.as_str()));
    }
    let answer = token_post(caps, server, &meta.token_endpoint, &pairs)?;
    store_issuance(
        store,
        &answer,
        client_id,
        client_secret,
        &meta.token_endpoint,
        scope,
    )?;
    // The grant now covers the stepped-up scopes: forget the pending
    // memory (a best-effort delete; absence is the common case).
    let _ = store.clear_pending();
    Ok(())
}

/// Use the refresh token to mint a new access token. `Ok(true)` means
/// the store holds fresh tokens; `Ok(false)` means there is nothing
/// to refresh with. An `invalid_grant` purges the dead tokens (the
/// grant died server-side; keeping it would loop).
pub fn refresh(
    caps: Arc<lca_tools::Capabilities>,
    server: &HttpServerConfig,
) -> Result<bool, String> {
    let store = TokenStore::new(caps.clone(), server);
    let Some(stored) = store.load()? else {
        return Ok(false);
    };
    let Some(refresh_token) = stored.refresh_token.clone() else {
        return Ok(false);
    };
    let mut pairs = vec![
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token.as_str()),
        ("client_id", stored.client_id.as_str()),
    ];
    let secret;
    if let Some(secret_value) = stored.client_secret.clone() {
        secret = secret_value;
        pairs.push(("client_secret", secret.as_str()));
    }
    // gh #418 Phase D: the token endpoint may live on the authorization
    // server's host, not the MCP server's. The stored token URL is where the
    // last sign-in legitimately went, so it is admitted for the duration of
    // this refresh and cleared right after - the same flow-scoped rule, and
    // no wider.
    if let Some(host) = flow_grant_for(&stored.token_url) {
        caps.oauth_flow_attach([host])
            .map_err(|err| format!("cannot admit the token endpoint's host: {err}"))?;
    }
    let outcome = token_post(caps.clone(), server, &stored.token_url, &pairs);
    caps.oauth_flow_clear();
    match outcome {
        Ok(answer) => {
            store_issuance(
                &store,
                &answer,
                &stored.client_id,
                stored.client_secret.as_deref(),
                &stored.token_url,
                stored.scope.as_deref(),
            )?;
            Ok(true)
        }
        Err(detail) if detail.contains("invalid_grant") => {
            let _ = store.clear();
            Ok(false)
        }
        Err(detail) => Err(detail),
    }
}

/// The sign-in copy for `hosts`: `via <server>: authorization host <host>`,
/// empty when every host is the MCP server's own.
///
/// Exposed so the manager (and a test) can render the same sentence the
/// sign-in screen uses without reaching into `IdpMeta` (gh #418 Phase D).
pub fn via_copy(server: &HttpServerConfig, hosts: &[String]) -> String {
    IdpMeta {
        authorization_endpoint: hosts.first().cloned().unwrap_or_default(),
        token_endpoint: String::new(),
        registration_endpoint: None,
    }
    .via_report(server, hosts)
}

#[cfg(test)]
mod origin_tests {
    //! gh #467: `origin_of` must parse a real URL.
    //!
    //! These rows are unit-level on purpose: the defect survived because
    //! nothing asserted that a *well-formed* URL succeeds. The branch-level
    //! rows (that `discover` actually falls back) live in
    //! `tests/oauth.rs::discovery_falls_back_to_the_servers_own_origin`,
    //! because that is where the fallback is reachable at all.
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code

    use super::origin_of;

    // Verifies: gh #467 - the row whose absence let a function that cannot
    // succeed survive. Every one of these used to return an error.
    #[test]
    fn an_origin_is_the_scheme_and_authority_of_a_real_url() {
        for (url, expected) in [
            ("http://127.0.0.1:49388/mcp", "http://127.0.0.1:49388"),
            ("http://127.0.0.1:49388", "http://127.0.0.1:49388"),
            ("https://example.com", "https://example.com"),
            ("https://example.com/", "https://example.com"),
            (
                "https://example.com:8443/a/b?q=1#frag",
                "https://example.com:8443",
            ),
            ("https://example.com?q=1", "https://example.com"),
            ("https://example.com#frag", "https://example.com"),
        ] {
            assert_eq!(
                origin_of(url).unwrap_or_else(|err| panic!("{url:?} has an origin: {err}")),
                expected,
                "the origin of {url:?}"
            );
        }
    }

    // Verifies: gh #467 design point 1 - userinfo is stripped. A credential
    // must never reach a string that gets compared against a host grant or
    // written into a discovery note.
    #[test]
    fn credentials_never_survive_into_the_origin() {
        for (url, expected) in [
            ("https://user:pw@example.com/mcp", "https://example.com"),
            (
                "https://token@example.com:9000/x",
                "https://example.com:9000",
            ),
            ("http://a:b@127.0.0.1:8080", "http://127.0.0.1:8080"),
        ] {
            let origin = origin_of(url).unwrap_or_else(|err| panic!("{url:?}: {err}"));
            assert_eq!(origin, expected, "the origin of {url:?}");
            assert!(
                !origin.contains('@'),
                "no userinfo survives: {origin:?} from {url:?}"
            );
            assert!(!origin.contains("pw"), "and not the value: {origin:?}");
        }
    }

    // Verifies: gh #467 design point 1 - an IPv6 literal keeps its brackets,
    // and the colons inside it are not a port. `checked_endpoint` learned this
    // the hard way; an origin string that split on ':' would mangle every v6
    // host into `http://[::1`.
    #[test]
    fn an_ipv6_literal_keeps_its_brackets_and_its_port() {
        for (url, expected) in [
            ("http://[::1]:8080/mcp", "http://[::1]:8080"),
            ("http://[::1]/mcp", "http://[::1]"),
            ("https://[2001:db8::1]:443/x", "https://[2001:db8::1]:443"),
        ] {
            assert_eq!(
                origin_of(url).unwrap_or_else(|err| panic!("{url:?}: {err}")),
                expected,
                "the origin of {url:?}"
            );
        }
    }

    // Verifies: gh #467 - a scheme-less string is refused, and the message
    // names the input. This is the *real* input error the old guard was
    // pretending to be.
    #[test]
    fn a_url_with_no_scheme_is_refused_and_named() {
        for url in ["example.com/mcp", "/mcp", "127.0.0.1:8080"] {
            let err = origin_of(url).expect_err("a scheme-less string has no origin");
            assert!(
                err.contains(url),
                "the refusal names the input {url:?}: {err}"
            );
        }
    }

    // Verifies: gh #467 - an empty authority is refused rather than becoming
    // `http://`, which would be a URL that resolves to nothing and a grant
    // that matches nothing - a silent failure dressed as success.
    #[test]
    fn an_empty_authority_is_refused() {
        for url in ["http:///mcp", "https://", "http://"] {
            let err = origin_of(url).expect_err("no authority, no origin");
            assert!(
                err.contains(url),
                "the refusal names the input {url:?}: {err}"
            );
        }
    }

    // Verifies: gh #467 - the scheme keeps its case-insensitive identity but a
    // grant lookup is exact, so a mixed-case scheme normalises rather than
    // reaching a comparison as `HTTP://`. `checked_endpoint` lowercases for the
    // same reason.
    #[test]
    fn a_mixed_case_scheme_normalises() {
        assert_eq!(
            origin_of("HTTPS://Example.com/x").expect("an origin"),
            "https://Example.com"
        );
    }
}
