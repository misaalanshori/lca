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
    /// How many `resource_read` probes have been answered.
    probes: Mutex<usize>,
    /// After this many probes, answer like a host that aborted the call
    /// (gh #566's cancellation model: the engine's flag, seen at a host call).
    abort_after_probes: Option<usize>,
}

impl FakeCap {
    fn new(responses: Vec<(u16, Vec<u8>)>) -> Self {
        FakeCap {
            requests: Mutex::new(Vec::new()),
            responses: Mutex::new(responses.into()),
            credentials: Mutex::new(HashMap::new()),
            probes: Mutex::new(0),
            abort_after_probes: None,
        }
    }

    /// Answer the Nth resource probe onward like a cancelled host call.
    fn abort_probes_after(mut self, n: usize) -> Self {
        self.abort_after_probes = Some(n);
        self
    }

    /// Seed the credential drawer, so a row can drive `access_token`'s refresh path.
    fn seed(&self, pairs: &[(&str, &str)]) {
        let mut credentials = self.credentials.lock().unwrap();
        for (key, value) in pairs {
            credentials.insert((*key).to_string(), (*value).to_string());
        }
    }

    /// The headers of the first recorded request, in order.
    fn first_request_headers(&self) -> Vec<(String, String)> {
        self.requests
            .lock()
            .unwrap()
            .first()
            .map(|(_, _, headers, _)| headers.clone())
            .unwrap_or_default()
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

    /// The wait probe (gh #566): `poll_device_code`'s sliced sleep calls this
    /// between slices precisely so a host call happens, which is the boundary
    /// a WASM epoch trap can fire at and the point a native engine's cancelled
    /// flag is observed. The answer for a path the extension does not define
    /// is `NotFound`; after `abort_after_probes`, it is the engine's abort.
    fn resource_read(&self, _path: &str) -> Result<Vec<u8>, CapabilityError> {
        let mut probes = self.probes.lock().unwrap();
        *probes += 1;
        match self.abort_after_probes {
            Some(n) if *probes >= n => Err(CapabilityError::Io(
                "request cancelled by the user".to_string(),
            )),
            _ => Err(CapabilityError::NotFound("no resource".to_string())),
        }
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

// Verifies: gh #184/#186/#187 - the shared device-flow primitives:
// field validation, the complete-URI preference, the untrusted-URI
// refusal, and the poll mapping (pending/slow_down/complete/fail).
#[test]
fn the_device_flow_primitives_hold() {
    use lca_subscription::{
        DevicePollOutcome, device_code_url, poll_device_code, poll_device_once, request_device_code,
    };

    // A full authorization answers every field; the complete URI wins.
    let cap = FakeCap::new(vec![(
        200,
        br#"{"device_code":"dev-1","user_code":"ABCD-1234","verification_uri":"https://x.test/device","verification_uri_complete":"https://x.test/device?c=ABCD-1234","interval":1,"expires_in":30}"#.to_vec(),
    )]);
    let auth = request_device_code(&cap, "https://x.test/auth", "client-1", Some("read:user"))
        .expect("auth");
    assert_eq!(auth.device_code, "dev-1");
    assert_eq!(auth.user_code, "ABCD-1234");
    assert_eq!(auth.verification_uri, "https://x.test/device?c=ABCD-1234");
    assert_eq!((auth.interval_secs, auth.expires_in_secs), (1, 30));
    assert_eq!(
        device_code_url(&auth),
        "https://x.test/device?c=ABCD-1234#code=ABCD-1234"
    );
    let bodies = cap.request_bodies();
    assert!(
        bodies[0].contains("client_id=client-1"),
        "the client crossed"
    );
    assert!(
        bodies[0].contains("scope=read%3Auser"),
        "the scope crossed: {}",
        bodies[0]
    );

    // A non-http(s) verification URI is refused, not opened.
    let cap = FakeCap::new(vec![(
        200,
        br#"{"device_code":"d","user_code":"u","verification_uri":"ftp://x.test/y"}"#.to_vec(),
    )]);
    assert!(
        request_device_code(&cap, "https://x.test/auth", "c", None).is_err(),
        "untrusted URIs stay out"
    );

    // One poll each way.
    for (body, expect) in [
        (r#"{"access_token":"tok","expires_in":60}"#, "complete"),
        (r#"{"error":"authorization_pending"}"#, "pending"),
        (r#"{"error":"slow_down","interval":10}"#, "slowdown"),
        (r#"{"error":"access_denied"}"#, "failed"),
    ] {
        let cap = FakeCap::new(vec![(200, body.as_bytes().to_vec())]);
        match poll_device_once(&cap, "https://x.test/token", &[("a", "b")]).expect("poll") {
            DevicePollOutcome::Complete(json) => assert_eq!(expect, "complete", "{json}"),
            DevicePollOutcome::Pending => assert_eq!(expect, "pending"),
            DevicePollOutcome::SlowDown(interval) => {
                assert_eq!(expect, "slowdown");
                assert_eq!(interval, Some(10));
            }
            DevicePollOutcome::Failed(message) => {
                assert_eq!(expect, "failed", "{message}");
            }
        }
    }

    // The loop waits out a pending then returns the token.
    let cap = FakeCap::new(vec![
        (200, br#"{"error":"authorization_pending"}"#.to_vec()),
        (200, br#"{"access_token":"tok-2"}"#.to_vec()),
    ]);
    let auth = lca_subscription::DeviceAuthorization::new(
        "d".to_string(),
        "u".to_string(),
        "https://x.test/device".to_string(),
        1,
        30,
    );
    let json = poll_device_code(&cap, &auth, "https://x.test/token", &[("a", "b")]).expect("token");
    assert_eq!(
        json.get("access_token").and_then(|v| v.as_str()),
        Some("tok-2")
    );
}

/// The cancellation budget for the row below, per platform: the macOS figure and
/// the reason for it are in that row's comment (gh #588).
fn budget() -> std::time::Duration {
    if cfg!(target_os = "macos") {
        std::time::Duration::from_millis(500)
    } else {
        std::time::Duration::from_millis(150)
    }
}

// Verifies: gh #566 - a cancel during a device-code wait returns inside the
// cancellation budget, not after the RFC 8628 interval.
//
// The defect: `poll_device_code` slept the whole interval in one
// `std::thread::sleep`, so `/login` with `interval: 5` could not notice an Esc
// for five seconds (ten, fifteen, twenty after each `slow_down`). In WASM mode
// that is strictly worse than a slow cancel: a Wasmtime epoch trap fires only at
// a **guest code point**, so a blocked guest sleep has no boundary to trap at,
// and the login thread sat out the interval regardless (NFR-29's 50 ms budget).
//
// The fix is the wait's *shape*: 25 ms slices with one real host call between
// them. That call is what makes a guest's loop reach the boundary the epoch
// deadline is checked at, and what reaches the native engine the host's
// `interrupt()` flagged.
//
// This row is about elapsed time because that **is** the claim, so the number is
// measured and the bound is written beside it. Measured on this box, three runs:
// **50.7, 50.7, 54.0 ms** - the fake aborts after two 25 ms slices, so the wait
// returns on the second probe plus scheduling. On the pre-fix tree the same row
// took **15.0 s** (three 5 s intervals, since the abort is only observed at a
// network call). The bound is 150 ms: ~3x the measurement, so a busy scheduler
// does not turn this into a flaky row, while still an order of magnitude under
// the interval the defect cost. A bound at the measurement itself would fail on
// any loaded machine and teach nothing.
//
// **On macOS the bound is 500 ms, and it is derived from the defect rather than from
// an observation** (gh #588; `docs/platform-notes.md` records the family). Hosted
// macOS readings were **280.3 ms** (CI 42701900918) against the 150 ms bound - not a
// slow cancel but the platform's own quantum: the row slices 25 ms and probes twice,
// so 50 ms of work plus one 60-80 ms quantum of scheduling delay is the whole
// number, and a bound finer than a quantum is not a bound on this platform. What
// the row is *for* is the 5 s interval (15 s on the pre-fix tree), so 500 ms proves
// the same property ten to thirty times over on any scheduler; 400 would too, and
// 500 is chosen because it also leaves room for a loaded runner to insert two or
// three quanta without the number drifting toward the interval it rules out. Linux
// keeps 150 ms: it is measured (50.7 / 50.7 / 54.0 ms locally) and it holds.
#[test]
fn a_cancelled_device_flow_returns_inside_the_cancellation_budget() {
    use lca_subscription::{DeviceAuthorization, poll_device_code};

    // The server keeps saying "pending", so the loop would honour every one of
    // its 5 s intervals; the fake host starts aborting after two wait probes,
    // which lands during the *first* interval.
    let cap = FakeCap::new(vec![
        (200, br#"{"error":"authorization_pending"}"#.to_vec()),
        (200, br#"{"error":"authorization_pending"}"#.to_vec()),
    ])
    .abort_probes_after(2);
    let auth = DeviceAuthorization::new(
        "d".to_string(),
        "u".to_string(),
        "https://x.test/device".to_string(),
        5,
        60,
    );
    let started = std::time::Instant::now();
    let outcome = poll_device_code(&cap, &auth, "https://x.test/token", &[("a", "b")]);
    let elapsed = started.elapsed();
    let err = outcome.expect_err("a cancelled wait is an error, not a token");
    let budget = budget();
    assert!(
        elapsed < budget,
        "a cancel must land inside the budget, not after the {}-second interval: took {elapsed:?} against {budget:?} ({})",
        auth.interval_secs,
        err.0
    );
    // And the error says what happened. "device flow timed out" would be a lie
    // about a wait the user ended.
    assert!(
        err.0.contains("cancelled"),
        "the error names the cancellation rather than blaming a timeout: {}",
        err.0
    );
    assert!(
        !err.0.contains("timed out after one or more slow_down") && !err.0.ends_with("timed out"),
        "a cancel is never reported as a timeout: {}",
        err.0
    );
}

// Verifies: gh #566 - the sliced wait must not change what a successful flow
// does: a pending answer still polls again after one interval and returns the
// token. (The deadline/slow_down arithmetic is pinned by
// `the_device_flow_primitives_hold`; this pins that slicing preserved it.)
#[test]
fn slicing_the_wait_preserves_the_poll_arithmetic() {
    use lca_subscription::{DeviceAuthorization, poll_device_code};

    let cap = FakeCap::new(vec![
        (200, br#"{"error":"authorization_pending"}"#.to_vec()),
        (200, br#"{"access_token":"tok"}"#.to_vec()),
    ]);
    let auth = DeviceAuthorization::new(
        "d".to_string(),
        "u".to_string(),
        "https://x.test/device".to_string(),
        1,
        30,
    );
    let json = poll_device_code(&cap, &auth, "https://x.test/token", &[("a", "b")])
        .expect("a pending answer is waited out, then a token");
    assert_eq!(
        json.get("access_token").and_then(|v| v.as_str()),
        Some("tok")
    );
    assert_eq!(
        cap.request_bodies().len(),
        2,
        "one poll for the pending answer, one that got the token"
    );
}

// Verifies: gh #339 finding 2 - `post_form` was a line-for-line copy of
// `post_form_accept` minus the `Accept` header. The delegation is the fix, so the guard
// is on the **wire**: the shared transport must send the form content type either way,
// add `accept` only for the callers that ask for it, and keep the one bounded read and
// close. Reading the recorded request rather than the source is what makes a future
// third caller fail here instead of inheriting a header it never asked for.
#[test]
fn the_form_post_sends_accept_only_when_asked() {
    let pairs = [("client_id", "c1")];

    let accepting = FakeCap::new(vec![(200, b"{}".to_vec())]);
    lca_subscription::post_form_accept(&accepting, "https://x.test/t", &pairs, "application/json")
        .expect("accepting post");
    let sent = accepting.first_request_headers();
    assert!(
        sent.iter()
            .any(|(name, value)| name == "accept" && value == "application/json"),
        "an accepting caller sends its header: {sent:?}"
    );
    assert!(
        sent.iter()
            .any(|(name, value)| name == "content-type"
                && value == "application/x-www-form-urlencoded"),
        "and the form content type: {sent:?}"
    );
    assert_eq!(
        accepting.request_bodies().len(),
        1,
        "one request, not one per header shape"
    );

    // The plain `post_form` is reached through the refresh path: an expired access
    // token with a refresh token present.
    let refreshing = FakeCap::new(vec![(
        200,
        br#"{"access_token":"new","expires_in":3600}"#.to_vec(),
    )]);
    refreshing.seed(&[("access", "stale"), ("refresh", "r1"), ("expires", "0")]);
    lca_subscription::access_token(&refreshing, &codex_spec()).expect("refresh");
    let sent = refreshing.first_request_headers();
    assert!(
        !sent.iter().any(|(name, _)| name == "accept"),
        "the plain form POST asks for nothing extra: {sent:?}"
    );
    assert!(
        sent.iter()
            .any(|(name, value)| name == "content-type"
                && value == "application/x-www-form-urlencoded"),
        "and it is still a form POST: {sent:?}"
    );
    let bodies = refreshing.request_bodies();
    assert!(
        bodies
            .first()
            .is_some_and(|body| body.contains("grant_type=refresh_token")),
        "with the pairs the caller passed: {bodies:?}"
    );
}
