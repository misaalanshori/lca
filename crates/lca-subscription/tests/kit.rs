//! The subscription kit's contract (gh #63): the OAuth PKCE journey
//! and the Responses core, against fakes — no sockets, no network.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use lca_protocol::{CapabilityError, OauthCap, ProviderCap};
use lca_subscription::{AccountStrategy, IdentityOutcome, OAuthSpec};

/// One recorded request: method, URL, headers, body.
type RecordedRequest = (String, String, Vec<(String, String)>, Vec<u8>);

/// A capability fake with scripted HTTP and a credential drawer.
struct FakeCap {
    requests: Mutex<Vec<RecordedRequest>>,
    responses: Mutex<VecDeque<(u16, Vec<u8>)>>,
    credentials: Mutex<HashMap<String, String>>,
}

impl FakeCap {
    fn new(responses: Vec<(u16, Vec<u8>)>) -> Self {
        FakeCap {
            requests: Mutex::new(Vec::new()),
            responses: Mutex::new(responses.into()),
            credentials: Mutex::new(HashMap::new()),
        }
    }

    fn request_bodies(&self) -> Vec<String> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .map(|(_, _, _, body)| String::from_utf8_lossy(body).into_owned())
            .collect()
    }
}

impl ProviderCap for FakeCap {
    fn net_request(
        &self,
        method: &str,
        url: &str,
        headers: &[(&str, &str)],
        body: Option<&[u8]>,
    ) -> Result<u32, CapabilityError> {
        self.requests.lock().unwrap().push((
            method.to_string(),
            url.to_string(),
            headers
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect(),
            body.unwrap_or_default().to_vec(),
        ));
        Ok(1)
    }

    fn net_response_status(&self, _handle: u32) -> Result<u16, CapabilityError> {
        Ok(self
            .responses
            .lock()
            .unwrap()
            .front()
            .map(|(status, _)| *status)
            .unwrap_or(200))
    }

    fn net_read_body(&self, _handle: u32, _max: usize) -> Result<Option<Vec<u8>>, CapabilityError> {
        let mut responses = self.responses.lock().unwrap();
        match responses.front_mut() {
            Some((_, body)) if !body.is_empty() => {
                let chunk = std::mem::take(body);
                Ok(Some(chunk))
            }
            Some(_) => {
                responses.pop_front();
                Ok(None)
            }
            None => Ok(None),
        }
    }

    fn net_close_response(&self, _handle: u32) -> Result<(), CapabilityError> {
        Ok(())
    }

    fn credentials_get(&self, key: &str) -> Option<String> {
        self.credentials.lock().unwrap().get(key).cloned()
    }

    fn credentials_set(&self, key: &str, value: &str) -> Result<(), CapabilityError> {
        self.credentials
            .lock()
            .unwrap()
            .insert(key.to_string(), value.to_string());
        Ok(())
    }

    fn credentials_delete(&self, key: &str) -> Result<(), CapabilityError> {
        self.credentials.lock().unwrap().remove(key);
        Ok(())
    }
}

/// An OAuth fake: the loopback listener is a channel the test feeds.
struct FakeOAuth {
    opened: Mutex<Vec<String>>,
    delivered: Mutex<Vec<Vec<(String, String)>>>,
}

impl FakeOAuth {
    fn new() -> Self {
        FakeOAuth {
            opened: Mutex::new(Vec::new()),
            delivered: Mutex::new(Vec::new()),
        }
    }
}

impl OauthCap for FakeOAuth {
    fn oauth_begin(&self, _redirect_path: &str) -> Result<(String, u32), CapabilityError> {
        Ok(("http://127.0.0.1:1/callback".to_string(), 7))
    }

    fn oauth_open(&self, url: &str) -> Result<(), CapabilityError> {
        self.opened.lock().unwrap().push(url.to_string());
        Ok(())
    }

    fn oauth_await(&self, _handle: u32) -> Result<Vec<(String, String)>, CapabilityError> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if let Some(params) = self.delivered.lock().unwrap().pop() {
                return Ok(params);
            }
            if std::time::Instant::now() > deadline {
                return Err(CapabilityError::Timeout("no callback".to_string()));
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    fn oauth_end(&self, _handle: u32) -> Result<(), CapabilityError> {
        Ok(())
    }
}

fn codex_spec() -> OAuthSpec {
    OAuthSpec {
        name: "codex",
        auth_endpoint: "https://auth.openai.com/oauth/authorize",
        token_endpoint: "https://auth.openai.com/oauth/token",
        revoke_endpoint: None,
        userinfo_endpoint: None,
        api_base: "https://chatgpt.com/backend-api",
        client_id: "test-client",
        scope: "openid profile email offline_access",
        extra_auth_params: &[("originator", "lca")],
        account: AccountStrategy::JwtClaim {
            claim: "https://api.openai.com/auth",
            field: "chatgpt_account_id",
        },
    }
}

fn jwt(account: &str) -> String {
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

// Verifies: gh #63 - the kit login runs the PKCE flow and stores the
// account: the auth URL carries the challenge and state, the exchange
// carries the verifier, and the JWT yields the account id.
#[test]
fn the_kit_login_exchanges_and_stores() {
    let spec = codex_spec();
    let token_body = format!(
        r#"{{"access_token":"{}","refresh_token":"ref-1","expires_in":3600}}"#,
        jwt("acct-1")
    );
    let cap = Arc::new(FakeCap::new(vec![
        (200, token_body.into_bytes()),
        (200, Vec::new()),
    ]));
    let oauth = Arc::new(FakeOAuth::new());
    let outcome = std::thread::spawn({
        let (cap, oauth) = (cap.clone(), oauth.clone());
        move || lca_subscription::run_login(cap.as_ref(), oauth.as_ref(), &spec)
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let url = loop {
        if let Some(url) = oauth.opened.lock().unwrap().last().cloned() {
            break url;
        }
        assert!(std::time::Instant::now() < deadline, "no auth URL");
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    assert!(url.contains("code_challenge="), "PKCE challenge sent");
    assert!(url.contains("originator=lca"), "extra params ride");
    let state = url
        .split('&')
        .find_map(|pair| pair.strip_prefix("state="))
        .expect("state")
        .to_string();
    oauth.delivered.lock().unwrap().push(vec![
        ("code".to_string(), "code-1".to_string()),
        ("state".to_string(), state),
    ]);
    assert_eq!(outcome.join().expect("thread"), Ok(IdentityOutcome::Ok));
    let bodies = cap.request_bodies();
    assert!(
        bodies.iter().any(|body| body.contains("code_verifier=")),
        "the verifier crossed: {bodies:?}"
    );
    assert_eq!(
        cap.credentials_get("account_id"),
        Some("acct-1".to_string())
    );
    assert!(cap.credentials_get("access").is_some());
}

// Verifies: gh #63 - a state mismatch fails the login before any
// exchange crosses.
#[test]
fn a_state_mismatch_fails_before_the_exchange() {
    let spec = codex_spec();
    let cap = FakeCap::new(Vec::new());
    let oauth = FakeOAuth::new();
    oauth.delivered.lock().unwrap().push(vec![
        ("code".to_string(), "code-1".to_string()),
        ("state".to_string(), "wrong".to_string()),
    ]);
    let outcome = lca_subscription::run_login(&cap, &oauth, &spec);
    assert!(outcome.is_err());
    assert!(cap.request_bodies().is_empty(), "no exchange attempted");
}

// Verifies: gh #183/#185 - the shared JSON + PKCE primitives the
// non-standard gateways build on: one S256 pair, URL-encoded values,
// and a JSON POST with the bounded read.
#[test]
fn the_json_and_pkce_primitives_hold() {
    let (verifier, challenge) = lca_subscription::pkce_pair().expect("pair");
    assert_eq!(verifier.len(), 43, "32 random bytes base64url'd");
    assert_eq!(challenge.len(), 43, "SHA-256 base64url'd");
    assert!(verifier != challenge);
    assert_eq!(
        lca_subscription::percent_encode("a b+c"),
        "a%20b%2Bc",
        "uppercase hex like URLSearchParams"
    );
    let cap = FakeCap::new(vec![(200, b"{}".to_vec())]);
    let (status, _) = lca_subscription::post_json(
        &cap,
        "https://example.test/token",
        &serde_json::json!({"grant_type": "authorization_code"}),
    )
    .expect("post");
    assert_eq!(status, 200);
    let bodies = cap.request_bodies();
    assert_eq!(bodies.len(), 1);
    assert!(
        bodies[0].contains("authorization_code"),
        "JSON body crossed"
    );
    let content_type = cap
        .requests
        .lock()
        .unwrap()
        .iter()
        .flat_map(|(_, _, headers, _)| headers.clone())
        .find(|(name, _)| name == "content-type")
        .map(|(_, value)| value);
    assert_eq!(content_type.as_deref(), Some("application/json"));
}
