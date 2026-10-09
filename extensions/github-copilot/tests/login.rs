//! The Copilot device login against the mock gateway (gh #184):
//! the code page opens with the fragment, the poll waits out one
//! pending, and the mint stores the token pair.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

mod common;

use common::{mock_server, sandbox};

fn stored(cap: &Arc<lca_tools::Capabilities>, key: &str) -> Option<String> {
    lca_protocol::ProviderCap::credentials_get(cap.as_ref(), key)
}

// Verifies: gh #184 - the device flow opens the verification page
// with the user code in the fragment (the host's display seam),
// polls past one pending, and stores the GitHub token beside the
// minted Copilot token.
#[test]
fn device_login_mints_and_stores_the_token_pair() {
    let mock = mock_server();
    let cap = sandbox(
        "github-copilot",
        "device",
        &mock,
        github_copilot::manifest_grants(),
    );
    let for_thread = cap.clone();
    type Outcome = Result<Vec<(String, String)>, String>;
    let result: Arc<Mutex<Option<Outcome>>> = Arc::new(Mutex::new(None));
    let slot = result.clone();
    std::thread::spawn(move || {
        let answer = lca_protocol::LoginAnswer {
            choice: github_copilot::CHOICE_DEVICE.to_string(),
            values: Default::default(),
        };
        let outcome =
            github_copilot::login_submit(for_thread.as_ref(), for_thread.as_ref(), &answer);
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
    assert_eq!(
        opened, "https://github.com/login/device#code=ABCD-1234",
        "the page opens with the code fragment"
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
    assert_eq!(
        stored(&cap, "github_token"),
        Some("gh-device-token".to_string())
    );
    assert!(
        stored(&cap, "access").is_some_and(|token| token.contains("proxy-ep=")),
        "the minted token lands"
    );
    assert!(stored(&cap, "expires").is_some(), "an expiry lands");
    assert!(cap.denials().is_empty(), "everything was granted");
}

// Verifies: gh #184 - the token's `proxy-ep` routes inference: a
// minted token names the individual API host.
#[test]
fn the_token_proxy_ep_routes_inference() {
    let mock = mock_server();
    let cap = sandbox(
        "github-copilot",
        "proxyep",
        &mock,
        github_copilot::manifest_grants(),
    );
    lca_protocol::ProviderCap::credentials_set(
        &*cap,
        "access",
        "tid=x;proxy-ep=proxy.individual.githubcopilot.com;",
    )
    .expect("seed");
    lca_protocol::ProviderCap::credentials_set(&*cap, "expires", &u64::MAX.to_string())
        .expect("seed");
    // No `api_base` override here: the point is the token routing.
    lca_protocol::ProviderCap::credentials_delete(&*cap, "api_base").expect("unseed");
    let request = lca_protocol::CompletionRequest {
        messages: Vec::new(),
        tools: Vec::new(),
        model: "gpt-4o".to_string(),
        stable_prefix: 0,
        extras: Default::default(),
    };
    // Pure build, no network: the URL is decided before any call.
    let parts = github_copilot::build_request(&*cap, &request).expect("builds without network");
    assert!(
        parts
            .0
            .starts_with("https://api.individual.githubcopilot.com/"),
        "the proxy host routes: {}",
        parts.0
    );
}
