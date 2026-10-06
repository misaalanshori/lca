//! The Codex subscription login against the mock gateway (gh
//! #180): the paste flow stores tokens plus the JWT account, a state
//! mismatch fails before any exchange, and logout clears.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

mod common;

use common::{mock_server, parse_paste, sandbox};

fn codex_jwt(account: &str) -> String {
    fn b64(data: &str) -> String {
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let bytes = data.as_bytes();
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let b = [
                chunk[0],
                *chunk.get(1).unwrap_or(&0),
                *chunk.get(2).unwrap_or(&0),
            ];
            let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
            out.push(ALPHABET[((n >> 18) & 63) as usize] as char);
            out.push(ALPHABET[((n >> 12) & 63) as usize] as char);
            if chunk.len() > 1 {
                out.push(ALPHABET[((n >> 6) & 63) as usize] as char);
            }
            if chunk.len() > 2 {
                out.push(ALPHABET[(n & 63) as usize] as char);
            }
        }
        out
    }
    format!(
        "{}.{}.{}",
        b64(r#"{"alg":"none"}"#),
        b64(&format!(
            r#"{{"https://api.openai.com/auth":{{"chatgpt_account_id":"{account}"}}}}"#
        )),
        b64("sig")
    )
}

/// Drive the login on a thread and complete it the headless way: read
/// the printed authorization URL, paste the callback back through the
/// manual-delivery seam (the same wakeup the loopback listener gives).
fn drive_login_paste(
    cap: &Arc<lca_tools::Capabilities>,
) -> Result<lca_protocol::IdentityOutcome, String> {
    let for_thread = cap.clone();
    let result: Arc<Mutex<Option<Result<lca_protocol::IdentityOutcome, String>>>> =
        Arc::new(Mutex::new(None));
    let slot = result.clone();
    std::thread::spawn(move || {
        let outcome =
            codex::run_login(for_thread.as_ref(), for_thread.as_ref()).map_err(|err| err.0);
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
    assert!(auth_url.contains("code_challenge="), "PKCE challenge sent");
    assert!(
        auth_url.contains("codex_cli_simplified_flow=true"),
        "the codex flow flag rides: {auth_url}"
    );

    // The user opens the URL elsewhere and pastes the redirect back.
    let paste = format!("http://127.0.0.1:9/auth/callback?code=mock-code&state={state}");
    cap.oauth_deliver_manual(parse_paste(&paste))
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

fn stored(cap: &Arc<lca_tools::Capabilities>, key: &str) -> Option<String> {
    lca_protocol::ProviderCap::credentials_get(cap.as_ref(), key)
}

// Verifies: gh #180 - the paste flow completes the Codex OAuth loop:
// tokens plus the JWT account land in the namespace, the verifier
// crosses on the exchange, and everything was granted.
#[test]
fn login_paste_stores_tokens_and_the_jwt_account() {
    let mock = mock_server();
    mock.set_token(&format!(
        r#"{{"access_token":"{}","refresh_token":"mock-refresh","expires_in":1800}}"#,
        codex_jwt("acct-codex-1")
    ));
    let cap = sandbox(
        "codex",
        "login",
        &mock,
        codex::manifest_grants(),
        "test-client",
    );
    let outcome = drive_login_paste(&cap).expect("login runs");
    assert_eq!(outcome, lca_protocol::IdentityOutcome::Ok);
    assert_eq!(stored(&cap, "account_id"), Some("acct-codex-1".to_string()));
    assert!(stored(&cap, "access").is_some());
    assert!(stored(&cap, "refresh").is_some());
    assert!(
        !mock.requests_of("code_verifier").is_empty(),
        "verifier crossed"
    );
    assert!(cap.denials().is_empty(), "everything was granted");
}

// Verifies: gh #180 - a login with stored tokens re-runs the flow
// (gh #179's deadlock, ported forward): the second login publishes a
// fresh authorization URL and stores fresh tokens.
#[test]
fn login_with_stored_tokens_re_runs_the_flow() {
    let mock = mock_server();
    mock.set_token(&format!(
        r#"{{"access_token":"{}","refresh_token":"r2","expires_in":1800}}"#,
        codex_jwt("acct-codex-2")
    ));
    let cap = sandbox(
        "codex",
        "relogin",
        &mock,
        codex::manifest_grants(),
        "test-client",
    );
    cap.credentials_set("access", "stale").expect("seed");
    cap.credentials_set("refresh", "stale").expect("seed");
    cap.credentials_set("expires", "1").expect("seed");

    let outcome = drive_login_paste(&cap).expect("login runs");
    assert_eq!(outcome, lca_protocol::IdentityOutcome::Ok);
    assert_eq!(cap.oauth_opened().len(), 1, "the flow re-opened");
    assert_eq!(stored(&cap, "account_id"), Some("acct-codex-2".to_string()));
}

// Verifies: gh #180 - logout clears the namespace.
#[test]
fn logout_clears_the_namespace() {
    let mock = mock_server();
    let cap = sandbox(
        "codex",
        "logout",
        &mock,
        codex::manifest_grants(),
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
        codex::run_logout(cap.as_ref()),
        lca_protocol::IdentityOutcome::Ok
    );
    for key in ["access", "refresh", "expires", "account_id"] {
        assert_eq!(stored(&cap, key), None, "{key} cleared");
    }
}
