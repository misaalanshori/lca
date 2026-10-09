//! The Meta device login against the mock gateway (gh #186): the
//! page opens with the code, the poll mints, and the identity rides
//! as the refresh beside the minted day-key.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

mod common;

use common::{mock_server, sandbox};

fn stored(cap: &Arc<lca_tools::Capabilities>, key: &str) -> Option<String> {
    lca_protocol::ProviderCap::credentials_get(cap.as_ref(), key)
}

// Verifies: gh #186 - the device login opens the Meta page with the
// code fragment, polls past one pending, and stores the identity as
// `refresh` with the minted key as `access`.
#[test]
fn device_login_mints_and_stores_identity_beside_the_key() {
    let mock = mock_server();
    let cap = sandbox("meta", "device", &mock, meta::manifest_grants());
    let for_thread = cap.clone();
    let result: Arc<Mutex<Option<Result<lca_protocol::IdentityOutcome, String>>>> =
        Arc::new(Mutex::new(None));
    let slot = result.clone();
    std::thread::spawn(move || {
        let outcome =
            meta::run_login(for_thread.as_ref(), for_thread.as_ref()).map_err(|err| err.0);
        *slot.lock().expect("slot") = Some(outcome);
    });

    let deadline = Instant::now() + Duration::from_secs(15);
    let opened = loop {
        if let Some(url) = cap.oauth_opened().last().cloned() {
            break url;
        }
        if Instant::now() > deadline {
            panic!("the device login never published its page");
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    assert!(
        opened.ends_with("#code=123456"),
        "the page opens with the code fragment: {opened}"
    );

    let deadline = Instant::now() + Duration::from_secs(90);
    let outcome = loop {
        if let Some(outcome) = result.lock().expect("slot").take() {
            break outcome;
        }
        if Instant::now() > deadline {
            panic!("the device login never finished polling");
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    outcome.expect("device login runs");
    assert_eq!(stored(&cap, "refresh"), Some("meta-identity".to_string()));
    assert_eq!(stored(&cap, "access"), Some("meta-minted-key".to_string()));
    assert!(stored(&cap, "expires").is_some(), "a day-ish expiry lands");
    let mints = mock.requests_of("/muse-code/key");
    assert_eq!(mints.len(), 1, "one mint: {mints:?}");
    assert!(
        mints[0].contains("Bearer meta-identity"),
        "the identity authenticates the mint: {}",
        mints[0]
    );
    assert!(cap.denials().is_empty(), "everything was granted");
}

// Verifies: gh #186 - a dead session (401 on mint) purges the trio so
// the next call re-authenticates instead of looping.
#[test]
fn a_dead_session_purges_on_mint() {
    let mock = mock_server();
    mock.fail_once("/muse-code/key", 401, Some(r#"{"detail":"expired"}"#));
    let cap = sandbox("meta", "dead", &mock, meta::manifest_grants());
    lca_protocol::ProviderCap::credentials_set(&*cap, "refresh", "dead-identity").expect("seed");
    lca_protocol::ProviderCap::credentials_set(&*cap, "access", "dead-key").expect("seed");
    let request = lca_protocol::CompletionRequest {
        messages: vec![lca_protocol::ChatMessage {
            role: lca_protocol::MessageRole::User,
            content: vec![lca_protocol::ContentBlock::Text {
                text: "hi".to_string(),
            }],
            tool_calls: Vec::new(),
            tool_call_id: None,
            usage: None,
            extras: Default::default(),
        }],
        tools: Vec::new(),
        model: "llama-3.3-70b-instruct".to_string(),
        stable_prefix: 0,
        extras: Default::default(),
    };
    let mut events = Vec::new();
    let outcome = meta::run_provider_stream(cap.as_ref(), &request, &mut |event| {
        events.push(event);
        true
    });
    let Err(failure) = outcome else {
        panic!("a dead session must fail");
    };
    assert!(
        failure.message.contains("/login meta"),
        "names the way back: {}",
        failure.message
    );
    assert!(stored(&cap, "access").is_none(), "the trio purges");
}
