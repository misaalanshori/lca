//! The Grok subscription login against the mock gateway (gh
//! #181): the loopback flow stores tokens plus the userinfo account,
//! a state mismatch fails before any exchange, and logout revokes and
//! clears.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

mod common;

use common::{mock_server, sandbox};

/// Drive the login on a thread and complete it through the loopback
/// listener (the browser shape; the paste shape rides the same wakeup
/// and is pinned by the codex journey).
fn drive_login_loopback(
    cap: &Arc<lca_tools::Capabilities>,
) -> Result<lca_protocol::IdentityOutcome, String> {
    let for_thread = cap.clone();
    let result: Arc<Mutex<Option<Result<lca_protocol::IdentityOutcome, String>>>> =
        Arc::new(Mutex::new(None));
    let slot = result.clone();
    std::thread::spawn(move || {
        let outcome =
            grok::run_login(for_thread.as_ref(), for_thread.as_ref()).map_err(|err| err.0);
        *slot.lock().expect("slot") = Some(outcome);
    });

    let deadline = Instant::now() + Duration::from_secs(15);
    let (redirect, auth_url) = loop {
        if let (Some(redirect), Some(auth)) = (
            cap.oauth_begun().last().cloned(),
            cap.oauth_opened().last().cloned(),
        ) {
            break (redirect, auth);
        }
        if Instant::now() > deadline {
            return Err("the login never bound a redirect URL".to_string());
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    let state = auth_url
        .split('&')
        .find_map(|pair| pair.strip_prefix("state="))
        .ok_or_else(|| "the authorization URL carries no state".to_string())?
        .to_string();
    assert!(auth_url.contains("code_challenge="), "PKCE challenge sent");
    assert!(
        auth_url.contains("referrer=lca"),
        "this agent's referrer rides"
    );
    assert!(
        auth_url.contains("grok-cli%3Aaccess") || auth_url.contains("grok-cli:access"),
        "the grok scope rides: {auth_url}"
    );

    let callback = format!("{redirect}?code=mock-code&state={state}");
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut attempts = 0;
    loop {
        if let Some(outcome) = result.lock().expect("slot").take() {
            return outcome;
        }
        if attempts < 5 {
            let _ = send_callback(&callback);
            attempts += 1;
        }
        if Instant::now() > deadline {
            return Err("the login never finished after the callback was sent".to_string());
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

fn send_callback(callback: &str) -> Result<(), String> {
    let host_path = callback.trim_start_matches("http://");
    let (hostport, path) = host_path.split_once('/').ok_or("no redirect path")?;
    let mut stream = std::net::TcpStream::connect(hostport).map_err(|err| err.to_string())?;
    write!(
        stream,
        "GET /{path} HTTP/1.1\r\nHost: {hostport}\r\nConnection: close\r\n\r\n"
    )
    .map_err(|err| err.to_string())
}

fn stored(cap: &Arc<lca_tools::Capabilities>, key: &str) -> Option<String> {
    lca_protocol::ProviderCap::credentials_get(cap.as_ref(), key)
}

// Verifies: gh #181 - the loopback flow completes the Grok OAuth
// loop: tokens plus the userinfo account land in the namespace.
#[test]
fn login_loopback_stores_tokens_and_the_userinfo_account() {
    let mock = mock_server();
    let cap = sandbox(
        "grok",
        "login",
        &mock,
        grok::manifest_grants(),
        "test-client",
    );
    let outcome = drive_login_loopback(&cap).expect("login runs");
    assert_eq!(outcome, lca_protocol::IdentityOutcome::Ok);
    assert_eq!(stored(&cap, "account_id"), Some("mock-acct-1".to_string()));
    assert!(stored(&cap, "access").is_some());
    assert!(
        !mock.requests_of("code_verifier").is_empty(),
        "verifier crossed"
    );
    assert!(
        !mock.requests_of("/userinfo").is_empty(),
        "account looked up"
    );
    assert!(cap.denials().is_empty(), "everything was granted");
}

// Verifies: gh #181 - logout revokes the refresh token server-side,
// then clears the namespace.
#[test]
fn logout_revokes_and_clears_the_namespace() {
    let mock = mock_server();
    let cap = sandbox(
        "grok",
        "logout",
        &mock,
        grok::manifest_grants(),
        "test-client",
    );
    for (key, value) in [
        ("access", "a"),
        ("refresh", "r"),
        ("expires", "9"),
        ("account_id", "x"),
    ] {
        cap.credentials_set(key, value).expect("seed");
    }
    assert_eq!(
        grok::run_logout(cap.as_ref()),
        lca_protocol::IdentityOutcome::Ok
    );
    assert!(!mock.requests_of("/revoke").is_empty(), "revoke crossed");
    for key in ["access", "refresh", "expires", "account_id"] {
        assert_eq!(stored(&cap, key), None, "{key} cleared");
    }
}
