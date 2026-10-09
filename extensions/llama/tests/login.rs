//! The llama login + probe against the mock server (gh #62): the
//! submit stores the endpoint and probes, and a dead port fails here.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use std::sync::Arc;

mod common;

use common::{mock_server, sandbox};

fn stored(cap: &Arc<lca_tools::Capabilities>, key: &str) -> Option<String> {
    lca_protocol::ProviderCap::credentials_get(cap.as_ref(), key)
}

// Verifies: gh #62 - the submit stores a normalized endpoint (the
// `/v1` tail goes) and the probe passes on a live router.
#[test]
fn submit_stores_the_endpoint_and_probes_it() {
    let mock = mock_server();
    let cap = sandbox("llama", "submit", &mock, llama::manifest_grants());
    lca_protocol::ProviderCap::credentials_delete(&*cap, "base_url").expect("unseed");
    let answer = lca_protocol::LoginAnswer {
        choice: llama::CHOICE_LOCAL.to_string(),
        values: [("base-url".to_string(), format!("{}/v1/", mock.base))]
            .into_iter()
            .collect(),
    };
    llama::login_submit(cap.as_ref(), &answer).expect("stores");
    assert_eq!(stored(&cap, "base_url"), Some(mock.base.clone()));
    let outcome = llama::run_login(cap.as_ref()).expect("probe passes");
    assert_eq!(outcome, lca_protocol::IdentityOutcome::Ok);
}

// Verifies: gh #62 - a dead port fails the login with the way back,
// not mid-turn.
#[test]
fn a_dead_port_fails_login_with_the_way_back() {
    let mock = mock_server();
    let cap = sandbox("llama", "dead", &mock, llama::manifest_grants());
    let answer = lca_protocol::LoginAnswer {
        choice: llama::CHOICE_LOCAL.to_string(),
        values: [("base-url".to_string(), "http://127.0.0.1:1".to_string())]
            .into_iter()
            .collect(),
    };
    let Err(reason) = llama::login_submit(cap.as_ref(), &answer) else {
        panic!("a dead port must fail");
    };
    assert!(
        reason.contains("llama-server"),
        "names the way back: {reason}"
    );
}
