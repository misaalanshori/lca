//! The Anthropic logins against the mock gateway (gh #183): the
//! browser loopback flow, the copy-code paste, the API key, and the
//! state-mismatch refusal before any exchange.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

mod common;

use common::{mock_server, sandbox};

fn stored(cap: &Arc<lca_tools::Capabilities>, key: &str) -> Option<String> {
    lca_protocol::ProviderCap::credentials_get(cap.as_ref(), key)
}

/// Drive the browser login on a thread and complete it the headless
/// way: read the published authorization URL, paste the callback back
/// through the manual-delivery seam (the same wakeup the loopback
/// listener gives).
fn drive_browser_login(
    cap: &Arc<lca_tools::Capabilities>,
) -> Result<lca_protocol::IdentityOutcome, String> {
    let for_thread = cap.clone();
    let result: Arc<Mutex<Option<Result<lca_protocol::IdentityOutcome, String>>>> =
        Arc::new(Mutex::new(None));
    let slot = result.clone();
    std::thread::spawn(move || {
        let outcome =
            anthropic::run_login(for_thread.as_ref(), for_thread.as_ref()).map_err(|err| err.0);
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
    let state = auth_url
        .split('&')
        .find_map(|pair| pair.strip_prefix("state="))
        .ok_or_else(|| "the authorization URL carries no state".to_string())?
        .to_string();
    assert!(
        auth_url.contains("code=true"),
        "pi's flag rides: {auth_url}"
    );
    assert!(auth_url.contains("code_challenge="), "PKCE challenge sent");

    // The user opens the URL elsewhere and pastes the redirect back.
    cap.oauth_deliver_manual(vec![
        ("code".to_string(), "mock-code".to_string()),
        ("state".to_string(), state),
    ])
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

// Verifies: gh #183 - the browser flow stores the token triple and
// exchanges JSON (the form endpoint would 400): the verifier and the
// loopback redirect cross in a JSON body.
#[test]
fn browser_login_stores_the_token_triple_over_json() {
    let mock = mock_server();
    let cap = sandbox("anthropic", "browser", &mock, anthropic::manifest_grants());
    let outcome = drive_browser_login(&cap).expect("login runs");
    assert_eq!(outcome, lca_protocol::IdentityOutcome::Ok);
    assert_eq!(stored(&cap, "access"), Some("mock-access".to_string()));
    assert_eq!(stored(&cap, "refresh"), Some("mock-refresh".to_string()));
    assert!(stored(&cap, "expires").is_some());
    let exchanges = mock.requests_of("/token");
    assert_eq!(exchanges.len(), 1, "one exchange: {exchanges:?}");
    assert!(
        exchanges[0].contains("code_verifier"),
        "the verifier crossed: {}",
        exchanges[0]
    );
    assert!(
        exchanges[0].contains("127.0.0.1"),
        "the loopback redirect crossed: {}",
        exchanges[0]
    );
    assert!(cap.denials().is_empty(), "everything was granted");
}

// Verifies: gh #183 - the copy-code choice opens the platform page
// (not the loopback) and the pasted `code#state` pairs exchange
// against the copy-code redirect.
#[test]
fn copy_code_login_exchanges_against_the_platform_redirect() {
    let mock = mock_server();
    let cap = sandbox("anthropic", "copycode", &mock, anthropic::manifest_grants());
    let for_thread = cap.clone();
    type Outcome = Result<Vec<(String, String)>, String>;
    let result: Arc<Mutex<Option<Outcome>>> = Arc::new(Mutex::new(None));
    let slot = result.clone();
    std::thread::spawn(move || {
        let answer = lca_protocol::LoginAnswer {
            choice: anthropic::CHOICE_COPY_CODE.to_string(),
            values: Default::default(),
        };
        let outcome = anthropic::login_submit(for_thread.as_ref(), for_thread.as_ref(), &answer);
        *slot.lock().expect("slot") = Some(outcome);
    });

    let deadline = Instant::now() + Duration::from_secs(15);
    let auth_url = loop {
        if let Some(url) = cap.oauth_opened().last().cloned() {
            break url;
        }
        if Instant::now() > deadline {
            panic!("the copy-code login never published its URL");
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    let state = auth_url
        .split('&')
        .find_map(|pair| pair.strip_prefix("state="))
        .expect("state rides")
        .to_string();
    assert!(
        auth_url.contains("platform.claude.com%2Foauth%2Fcode%2Fcallback")
            || auth_url.contains("platform.claude.com/oauth/code/callback"),
        "the platform page opens, not the loopback: {auth_url}"
    );
    // What the host's `code#state` parse delivers (pinned in the CLI).
    cap.oauth_deliver_manual(vec![
        ("code".to_string(), "spl-pasted".to_string()),
        ("state".to_string(), state),
    ])
    .expect("deliver");

    let deadline = Instant::now() + Duration::from_secs(60);
    let outcome = loop {
        if let Some(outcome) = result.lock().expect("slot").take() {
            break outcome;
        }
        if Instant::now() > deadline {
            panic!("the copy-code login never finished after the paste");
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    outcome.expect("copy-code login runs");
    assert_eq!(stored(&cap, "access"), Some("mock-access".to_string()));
    let exchanges = mock.requests_of("/token");
    assert_eq!(exchanges.len(), 1, "one exchange: {exchanges:?}");
    assert!(
        exchanges[0].contains("spl-pasted"),
        "the pasted code crossed: {}",
        exchanges[0]
    );
}

// Verifies: gh #183 - a state mismatch fails before any exchange: the
// pasted callback did not come from this flow.
#[test]
fn a_state_mismatch_fails_before_any_exchange() {
    let mock = mock_server();
    let cap = sandbox("anthropic", "mismatch", &mock, anthropic::manifest_grants());
    let for_thread = cap.clone();
    let result: Arc<Mutex<Option<Result<lca_protocol::IdentityOutcome, String>>>> =
        Arc::new(Mutex::new(None));
    let slot = result.clone();
    std::thread::spawn(move || {
        let outcome =
            anthropic::run_login(for_thread.as_ref(), for_thread.as_ref()).map_err(|err| err.0);
        *slot.lock().expect("slot") = Some(outcome);
    });

    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if cap.oauth_opened().last().is_some() {
            break;
        }
        if Instant::now() > deadline {
            panic!("the login never published an authorization URL");
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    cap.oauth_deliver_manual(vec![
        ("code".to_string(), "mock-code".to_string()),
        ("state".to_string(), "someone-elses-state".to_string()),
    ])
    .expect("deliver");

    let deadline = Instant::now() + Duration::from_secs(60);
    let outcome = loop {
        if let Some(outcome) = result.lock().expect("slot").take() {
            break outcome;
        }
        if Instant::now() > deadline {
            panic!("the login never finished after the paste");
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let Err(reason) = outcome else {
        panic!("a mismatched state must fail");
    };
    assert!(reason.contains("state"), "names the mismatch: {reason}");
    assert!(mock.requests_of("/token").is_empty(), "no exchange crossed");
}

// Verifies: gh #183 - the API-key choice stores the key and nothing
// else; an empty key is refused.
#[test]
fn the_api_key_choice_stores_the_key() {
    let mock = mock_server();
    let cap = sandbox("anthropic", "apikey", &mock, anthropic::manifest_grants());
    let answer = lca_protocol::LoginAnswer {
        choice: anthropic::CHOICE_API_KEY.to_string(),
        values: [("api-key".to_string(), "sk-ant-test".to_string())]
            .into_iter()
            .collect(),
    };
    anthropic::login_submit(cap.as_ref(), cap.as_ref(), &answer).expect("stores");
    assert_eq!(stored(&cap, "api_key"), Some("sk-ant-test".to_string()));
    let empty = lca_protocol::LoginAnswer {
        choice: anthropic::CHOICE_API_KEY.to_string(),
        values: Default::default(),
    };
    assert!(
        anthropic::login_submit(cap.as_ref(), cap.as_ref(), &empty).is_err(),
        "an empty key is refused"
    );
}
