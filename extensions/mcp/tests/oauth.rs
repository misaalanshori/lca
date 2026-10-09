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
    server: &mcp::HttpServerConfig,
) -> Result<(), String> {
    let for_thread = caps.clone();
    let server = server.clone();
    let result: Arc<Mutex<Option<Result<(), String>>>> = Arc::new(Mutex::new(None));
    let slot = result.clone();
    std::thread::spawn(move || {
        let outcome = mcp::sign_in(for_thread, &server);
        *slot.lock().expect("slot") = Some(outcome);
    });

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    let auth_url = loop {
        if let Some(url) = caps.oauth_opened().last().cloned() {
            break url;
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

fn authed_server(mock: &common::Mock) -> mcp::HttpServerConfig {
    let mut server = remote_server(mock, "/mcp");
    server.oauth = Some(mcp::OAuthConfig {
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
    let caps = sandbox("oauth-full", mcp::manifest_grants());
    let server = authed_server(&mock);
    drive_sign_in(&caps, &server).expect("sign-in runs");

    let stored = mcp::TokenStore::new(caps.clone(), &server);
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
    let bridge = mcp::McpBridge::connect_http(caps.clone(), vec![server]).expect("connect");
    assert_eq!(bridge.tool_specs().expect("specs").len(), 1);
    assert!(caps.oauth_opened().len() == 1, "no second browser flow");
}

// Verifies: gh #53 - an expired access token refreshes before use
// (no browser involved) and the new token persists.
#[test]
fn an_expired_token_refreshes_before_use() {
    let mock = mock_server();
    let caps = sandbox("oauth-refresh", mcp::manifest_grants());
    let server = authed_server(&mock);
    let store = mcp::TokenStore::new(caps.clone(), &server);
    store
        .save(&mcp::StoredToken {
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

    let bridge = mcp::McpBridge::connect_http(caps.clone(), vec![server]).expect("connect");
    assert_eq!(bridge.tool_specs().expect("specs").len(), 1);
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
    let caps = sandbox("oauth-purge", mcp::manifest_grants());
    let server = authed_server(&mock);
    let store = mcp::TokenStore::new(caps.clone(), &server);
    store
        .save(&mcp::StoredToken {
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

    let err = match mcp::McpBridge::connect_http(caps.clone(), vec![server]) {
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
    let caps = sandbox("oauth-stepup", mcp::manifest_grants());
    let server = authed_server(&mock);
    let store = mcp::TokenStore::new(caps.clone(), &server);
    store
        .save(&mcp::StoredToken {
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

    let err = match mcp::McpBridge::connect_http(caps.clone(), vec![server.clone()]) {
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
    let bridge = mcp::McpBridge::connect_http(caps.clone(), vec![server]).expect("connect widens");
    assert_eq!(bridge.tool_specs().expect("specs").len(), 1);
}
