//! The OpenRouter login against the mock gateway (gh #185): the
//! loopback flow provisions a permanent key, and the paste-redirect
//! fallback completes it without the loopback.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

mod common;

use common::{mock_server, sandbox};

fn stored(cap: &Arc<lca_tools::Capabilities>, key: &str) -> Option<String> {
    lca_protocol::ProviderCap::credentials_get(cap.as_ref(), key)
}

/// Drive the login on a thread; the completer delivers the callback
/// pairs (loopback-shaped or pasted-redirect-shaped — both arrive as
/// pairs through the same seam).
fn drive_login(
    cap: &Arc<lca_tools::Capabilities>,
    paste: Vec<(String, String)>,
) -> Result<lca_protocol::IdentityOutcome, String> {
    let for_thread = cap.clone();
    let result: Arc<Mutex<Option<Result<lca_protocol::IdentityOutcome, String>>>> =
        Arc::new(Mutex::new(None));
    let slot = result.clone();
    std::thread::spawn(move || {
        let outcome =
            openrouter::run_login(for_thread.as_ref(), for_thread.as_ref()).map_err(|err| err.0);
        *slot.lock().expect("slot") = Some(outcome);
    });

    let deadline = Instant::now() + Duration::from_secs(15);
    let auth_url = loop {
        if let Some(url) = cap.oauth_opened().last().cloned() {
            break url;
        }
        if Instant::now() > deadline {
            return Err("the login never published an authorization URL".to_string());
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    assert!(
        auth_url.contains("callback_url="),
        "OpenRouter's param, not redirect_uri: {auth_url}"
    );
    assert!(
        auth_url.contains("code_challenge="),
        "PKCE rides: {auth_url}"
    );
    assert!(
        !auth_url.contains("client_id="),
        "no client id in this flow: {auth_url}"
    );
    cap.oauth_deliver_manual(paste)
        .map_err(|err| err.to_string())?;

    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if let Some(outcome) = result.lock().expect("slot").take() {
            return outcome;
        }
        if Instant::now() > deadline {
            return Err("the login never finished after the paste".to_string());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

// Verifies: gh #185 - the loopback flow stores the provisioned key as
// the access token with no refresh and a never-expiry; the exchange
// body is pi's JSON shape (code, verifier, method — nothing else).
#[test]
fn login_provisions_a_permanent_key() {
    let mock = mock_server();
    let cap = sandbox("openrouter", "login", &mock, openrouter::manifest_grants());
    let outcome =
        drive_login(&cap, vec![("code".to_string(), "mock-code".to_string())]).expect("login runs");
    assert_eq!(outcome, lca_protocol::IdentityOutcome::Ok);
    assert_eq!(stored(&cap, "access"), Some("sk-or-mock".to_string()));
    assert_eq!(stored(&cap, "refresh"), Some(String::new()));
    assert_eq!(
        stored(&cap, "expires"),
        Some(u64::MAX.to_string()),
        "the key never expires"
    );
    let exchanges = mock.requests_of("/auth/keys");
    assert_eq!(exchanges.len(), 1, "one exchange: {exchanges:?}");
    assert!(
        exchanges[0].contains("code_verifier"),
        "the verifier crossed: {}",
        exchanges[0]
    );
    assert!(
        exchanges[0].contains("S256"),
        "the method crossed: {}",
        exchanges[0]
    );
    assert!(cap.denials().is_empty(), "everything was granted");
}

// Verifies: gh #185 - the paste-redirect fallback (a remote/headless
// session pastes the redirect URL's query): same pairs, same key.
#[test]
fn the_paste_redirect_fallback_provisions_the_same_key() {
    let mock = mock_server();
    let cap = sandbox("openrouter", "paste", &mock, openrouter::manifest_grants());
    // What the host's callback parser yields for a pasted redirect URL.
    let paste = vec![("code".to_string(), "mock-code".to_string())];
    let outcome = drive_login(&cap, paste).expect("login runs");
    assert_eq!(outcome, lca_protocol::IdentityOutcome::Ok);
    assert_eq!(stored(&cap, "access"), Some("sk-or-mock".to_string()));
}

// Verifies: gh #185 - an exchange with no key fails loudly instead of
// storing an empty credential.
#[test]
fn an_exchange_without_a_key_fails() {
    let mock = mock_server();
    mock.set_key(r#"{"oops":true}"#);
    let cap = sandbox("openrouter", "nokey", &mock, openrouter::manifest_grants());
    let outcome = drive_login(&cap, vec![("code".to_string(), "mock-code".to_string())]);
    let Err(reason) = outcome else {
        panic!("a keyless exchange must fail");
    };
    assert!(reason.contains("no key"), "names the lack: {reason}");
    assert!(
        stored(&cap, "access").is_none_or(|key| key.is_empty()),
        "nothing usable stored"
    );
}
