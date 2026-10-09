//! The Kimi device login against the mock gateway (gh #187): the
//! complete page opens with the code, the poll stores the triple,
//! and the refresh renews it.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

mod common;

use common::{mock_server, sandbox};

fn stored(cap: &Arc<lca_tools::Capabilities>, key: &str) -> Option<String> {
    lca_protocol::ProviderCap::credentials_get(cap.as_ref(), key)
}

fn drive_login(
    cap: &Arc<lca_tools::Capabilities>,
) -> Result<lca_protocol::IdentityOutcome, String> {
    let for_thread = cap.clone();
    let result: Arc<Mutex<Option<Result<lca_protocol::IdentityOutcome, String>>>> =
        Arc::new(Mutex::new(None));
    let slot = result.clone();
    std::thread::spawn(move || {
        let outcome =
            kimi_coding::run_login(for_thread.as_ref(), for_thread.as_ref()).map_err(|err| err.0);
        *slot.lock().expect("slot") = Some(outcome);
    });

    let deadline = Instant::now() + Duration::from_secs(15);
    let opened = loop {
        if let Some(url) = cap.oauth_opened().last().cloned() {
            break url;
        }
        if Instant::now() > deadline {
            return Err("the device login never published its page".to_string());
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    // The displayed page is always the real verification URI (the
    // override only moves the API calls): the complete URI here,
    // which already embeds the code, plus the fragment.
    assert_eq!(
        opened, "https://auth.kimi.com/device?code=KIMI-42#code=KIMI-42",
        "the complete page opens with the code fragment"
    );

    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        if let Some(outcome) = result.lock().expect("slot").take() {
            return outcome;
        }
        if Instant::now() > deadline {
            return Err("the device login never finished polling".to_string());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

// Verifies: gh #187 - the device login stores the token triple.
#[test]
fn device_login_stores_the_token_triple() {
    let mock = mock_server();
    let cap = sandbox(
        "kimi-coding",
        "device",
        &mock,
        kimi_coding::manifest_grants(),
    );
    let outcome = drive_login(&cap).expect("login runs");
    assert_eq!(outcome, lca_protocol::IdentityOutcome::Ok);
    assert_eq!(stored(&cap, "access"), Some("kimi-access".to_string()));
    assert_eq!(stored(&cap, "refresh"), Some("kimi-refresh".to_string()));
    assert!(stored(&cap, "expires").is_some(), "an expiry lands");
    assert!(cap.denials().is_empty(), "everything was granted");
}

// Verifies: gh #187 - the login options stay empty (the identity flow
// runs directly, like codex).
#[test]
fn login_runs_without_picker_presets() {
    assert!(kimi_coding::login_options().is_empty());
}
