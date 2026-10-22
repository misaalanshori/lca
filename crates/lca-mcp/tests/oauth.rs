//! gh #53 phase 2: OAuth against the mock IdP - dynamic
//! registration, the browser loopback flow, the token store, refresh,
//! purge on failure, and step-up scopes.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use std::sync::{Arc, Mutex};

mod common;

use common::{mock_server, remote_server, sandbox};

/// Drive the interactive sign-in headlessly: the flow runs on a
/// thread, the published authorization URL is answered with a pasted
/// callback through the manual-delivery seam (the anthropic journeys'
/// shape).
fn drive_sign_in(
    caps: &Arc<lca_tools::Capabilities>,
    server: &lca_mcp::HttpServerConfig,
) -> Result<(), String> {
    let for_thread = caps.clone();
    let server = server.clone();
    let result: Arc<Mutex<Option<Result<(), String>>>> = Arc::new(Mutex::new(None));
    let slot = result.clone();
    std::thread::spawn(move || {
        let outcome = lca_mcp::sign_in(for_thread, &server);
        *slot.lock().expect("slot") = Some(outcome);
    });

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    let auth_url = loop {
        if let Some(url) = caps.oauth_opened().last().cloned() {
            break url;
        }
        // A sign-in that fails before publishing (a discovery error, an
        // unreachable IdP) leaves no URL to wait for. Report *its* reason
        // instead of the harness's timeout: "never published" is the symptom,
        // and the flow already knows the cause.
        if let Some(Err(err)) = result.lock().expect("slot").as_ref() {
            return Err(format!("the sign-in failed before publishing: {err}"));
        }
        if std::time::Instant::now() > deadline {
            return Err("the sign-in never published an authorization URL".to_string());
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    };
    assert!(
        auth_url.contains("code_challenge="),
        "PKCE rides: {auth_url}"
    );
    let state = auth_url
        .split('&')
        .find_map(|pair| pair.strip_prefix("state="))
        .ok_or_else(|| "the authorization URL carries no state".to_string())?
        .to_string();

    caps.oauth_deliver_manual(vec![
        ("code".to_string(), "mock-code".to_string()),
        ("state".to_string(), state),
    ])
    .map_err(|err| err.to_string())?;

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        if let Some(outcome) = result.lock().expect("slot").take() {
            return outcome;
        }
        if std::time::Instant::now() > deadline {
            return Err("the sign-in never finished after the paste".to_string());
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

fn authed_server(mock: &common::Mock) -> lca_mcp::HttpServerConfig {
    let mut server = remote_server(mock, "/mcp");
    server.oauth = Some(lca_mcp::OAuthConfig {
        client_name: Some("lca-test".to_string()),
        scope: Some("base".to_string()),
        auth_server_metadata_url: Some(format!(
            "{}/idp/.well-known/oauth-authorization-server",
            mock.base
        )),
        client_id: None,
        client_secret: None,
    });
    server
}

// Verifies: gh #53 - the full loop: registration, the browser flow,
// the code exchange, stored tokens, and a connected bridge after.
#[test]
fn browser_sign_in_registers_stores_and_connects() {
    let mock = mock_server();
    let caps = sandbox("oauth-full", lca_mcp::manifest_grants());
    let server = authed_server(&mock);
    drive_sign_in(&caps, &server).expect("sign-in runs");

    let stored = lca_mcp::TokenStore::new(caps.clone(), &server);
    let token = stored
        .load()
        .expect("store reads")
        .expect("a token is stored");
    assert_eq!(token.access_token, "mock-access");
    assert_eq!(token.refresh_token.as_deref(), Some("mock-refresh"));

    let registers = mock.requests_of("/register");
    assert_eq!(registers.len(), 1, "one registration: {registers:?}");
    assert!(
        registers[0].contains("lca-test"),
        "the client name crosses: {}",
        registers[0]
    );
    assert!(
        registers[0].contains("/callback"),
        "the loopback redirect crosses"
    );

    let exchanges = mock.requests_of("/token");
    assert_eq!(exchanges.len(), 1, "one exchange: {exchanges:?}");
    assert!(
        exchanges[0].contains("code_verifier"),
        "PKCE verifies: {}",
        exchanges[0]
    );

    // The stored token authenticates the bridge with no new flow.
    mock.guard(Some("mock-access"), None);
    let bridge = lca_mcp::McpBridge::connect_http(caps.clone(), vec![server]).expect("connect");
    assert_eq!(
        bridge.tool_specs().expect("specs").len(),
        4,
        "echo plus the resource trio"
    );
    assert!(caps.oauth_opened().len() == 1, "no second browser flow");
}

// Verifies: gh #53 - an expired access token refreshes before use
// (no browser involved) and the new token persists.
#[test]
fn an_expired_token_refreshes_before_use() {
    let mock = mock_server();
    let caps = sandbox("oauth-refresh", lca_mcp::manifest_grants());
    let server = authed_server(&mock);
    let store = lca_mcp::TokenStore::new(caps.clone(), &server);
    store
        .save(&lca_mcp::StoredToken {
            access_token: "stale-access".to_string(),
            refresh_token: Some("mock-refresh".to_string()),
            expires_at_ms: Some(1),
            client_id: "mock-client".to_string(),
            client_secret: Some("mock-secret".to_string()),
            scope: Some("base".to_string()),
            token_url: format!("{}/token", mock.base),
        })
        .expect("seed");
    mock.set_token(r#"{"access_token":"fresh-access","refresh_token":"fresh-refresh","expires_in":1800,"scope":"base"}"#);
    mock.guard(Some("fresh-access"), None);

    let bridge = lca_mcp::McpBridge::connect_http(caps.clone(), vec![server]).expect("connect");
    assert_eq!(
        bridge.tool_specs().expect("specs").len(),
        4,
        "echo plus the resource trio"
    );
    let refreshes = mock.requests_of("grant_type=refresh_token");
    assert_eq!(refreshes.len(), 1, "one refresh: {refreshes:?}");
    let token = store.load().expect("store reads").expect("still stored");
    assert_eq!(token.access_token, "fresh-access");
}

// Verifies: gh #53 - a rejected refresh purges the store and the call
// reports that sign-in is needed (not a transport error).
#[test]
fn a_bad_refresh_purges_and_needs_sign_in() {
    let mock = mock_server();
    let caps = sandbox("oauth-purge", lca_mcp::manifest_grants());
    let server = authed_server(&mock);
    let store = lca_mcp::TokenStore::new(caps.clone(), &server);
    store
        .save(&lca_mcp::StoredToken {
            access_token: "stale-access".to_string(),
            refresh_token: Some("dead-refresh".to_string()),
            expires_at_ms: Some(1),
            client_id: "mock-client".to_string(),
            client_secret: Some("mock-secret".to_string()),
            scope: Some("base".to_string()),
            token_url: format!("{}/token", mock.base),
        })
        .expect("seed");
    mock.set_token(r#"{"error":"invalid_grant"}"#);
    // Anonymous calls fail too, so the 401 path (not the open gate)
    // reports sign-in.
    mock.guard(Some("mock-access"), None);

    let err = match lca_mcp::McpBridge::connect_http(caps.clone(), vec![server]) {
        Ok(_) => panic!("no token works"),
        Err(err) => err,
    };
    assert!(err.contains("sign-in"), "the error names sign-in: {err}");
    assert!(
        store.load().expect("store reads").is_none(),
        "the dead token is purged"
    );
}

// Verifies: gh #53 - step-up: the server's `scope` challenge merges
// into the next sign-in instead of replacing the granted scopes.
#[test]
fn step_up_scopes_merge_on_re_sign_in() {
    let mock = mock_server();
    let caps = sandbox("oauth-stepup", lca_mcp::manifest_grants());
    let server = authed_server(&mock);
    let store = lca_mcp::TokenStore::new(caps.clone(), &server);
    store
        .save(&lca_mcp::StoredToken {
            access_token: "base-only".to_string(),
            refresh_token: None,
            expires_at_ms: Some(9_999_999_999_999),
            client_id: "mock-client".to_string(),
            client_secret: Some("mock-secret".to_string()),
            scope: Some("base".to_string()),
            token_url: format!("{}/token", mock.base),
        })
        .expect("seed");
    // The server wants more than the stored token carries.
    mock.guard(
        Some("never-this"),
        Some(r#"Bearer resource_metadata="http://unused.invalid/x", scope="extra""#),
    );

    let err = match lca_mcp::McpBridge::connect_http(caps.clone(), vec![server.clone()]) {
        Ok(_) => panic!("scope falls short"),
        Err(err) => err,
    };
    assert!(err.contains("sign-in"), "sign-in again: {err}");
    assert!(err.contains("extra"), "the missing scope is named: {err}");

    // The next sign-in asks for the union, and the store keeps it.
    mock.set_token(r#"{"access_token":"wide-access","expires_in":1800,"scope":"base extra"}"#);
    drive_sign_in(&caps, &server).expect("re-sign-in runs");
    let authorize = caps.oauth_opened().last().cloned().unwrap_or_default();
    assert!(
        authorize.contains("base") && authorize.contains("extra"),
        "the union rides the authorize URL: {authorize}"
    );
    let token = store.load().expect("store reads").expect("stored");
    assert_eq!(token.scope.as_deref(), Some("base extra"));
    assert_eq!(token.access_token, "wide-access");
    // ...and the widened token opens the server.
    mock.guard(Some("wide-access"), None);
    let bridge =
        lca_mcp::McpBridge::connect_http(caps.clone(), vec![server]).expect("connect widens");
    assert_eq!(
        bridge.tool_specs().expect("specs").len(),
        4,
        "echo plus the resource trio"
    );
}

/// A remote server with **no** configured metadata document, so discovery has to
/// find the authorization server some other way.
fn unconfigured_server(mock: &common::Mock) -> lca_mcp::HttpServerConfig {
    let mut server = remote_server(mock, "/mcp");
    server.oauth = Some(lca_mcp::OAuthConfig {
        client_name: Some("lca-test".to_string()),
        scope: Some("base".to_string()),
        auth_server_metadata_url: None,
        client_id: None,
        client_secret: None,
    });
    server
}

// Verifies: gh #467 design point 3 - **the branch the issue is really about**.
//
// `discover` has three steps: the configured metadata document, the challenge's
// RFC 9728 `resource_metadata` pointer, then the MCP server's own origin as its
// own issuer. Every existing row started at one of the first two, so the third
// was never exercised - which is how a function that cannot succeed survived, and
// why the same commit passed on Linux and failed on macOS: on Linux the earlier
// steps covered the row, on macOS the fallback was actually reached.
//
// Here the config names no document and the challenge carries no pointer, so the
// fallback is the only path left. The mock serves RFC 8414 metadata at
// `/.well-known/oauth-authorization-server` on its own origin, which is precisely
// what the fallback asks for.
#[test]
fn discovery_falls_back_to_the_servers_own_origin() {
    let mock = mock_server();
    let caps = sandbox("oauth-fallback", lca_mcp::manifest_grants());
    let server = unconfigured_server(&mock);

    // The 401 challenge names the IdP as a bare `Bearer` - no `resource_metadata`
    // pointer, which is the deployment the issue described: an IdP that
    // advertises no RFC 9728 document and a user who configured nothing.
    mock.guard(None, Some("Bearer"));
    drive_sign_in(&caps, &server).expect("the fallback sign-in completes");

    // It asked this server's own origin for the RFC 8414 document ...
    let fallback_fetches: Vec<String> = mock
        .requests_of("/.well-known/oauth-authorization-server")
        .into_iter()
        .collect();
    assert!(
        !fallback_fetches.is_empty(),
        "the fallback fetched the origin's own metadata: {:?}",
        mock.requests
    );

    // ... and the sign-in completed against it: a token is stored and it opens
    // the server. Before the fix this errored with
    // `cannot take the origin of "http://127.0.0.1:NNNNN/mcp"`.
    let store = lca_mcp::TokenStore::new(caps.clone(), &server);
    let token = store
        .load()
        .expect("store reads")
        .expect("the fallback stored a token");
    assert_eq!(token.access_token, "mock-access");
    assert_eq!(
        mock.requests_of("/register").len(),
        1,
        "registration happened against the discovered IdP"
    );

    mock.guard(Some("mock-access"), None);
    let bridge = lca_mcp::McpBridge::connect_http(caps.clone(), vec![server])
        .expect("the fallback's token opens the server");
    assert_eq!(bridge.tool_specs().expect("specs").len(), 4);
}
