//! GitHub issue #31: the openai-compatible provider had one `api_key`
//! and one `base_url`, so a second login overwrote the first, and every
//! `/model` row wore the extension's crate name instead of the service
//! that would bill the call.
//!
//! Profiles fix both, extension-internal (the host still never parses
//! them, ADR-0031): credentials live under `profile.<id>.*`, the legacy
//! bare pair reads as the unnamed default profile, the `models` setting
//! carries each model's profile (`id[@profile][=window]`), and routing
//! follows the model. This file drives the extension's own seams -
//! `login_submit` writes the profile, `StreamDriver::open` builds the
//! request - through an in-memory `ProviderCap`, so every assertion is
//! about the credential and request shapes, with no endpoint involved.
//!
//! The row-render half lives beside the renderer
//! (`crates/lca-cli/src/tui/display.rs`), and the list/label half beside
//! the picker rows in the tmux drives (this cycle's receipts).
//!
//! Verifies: NFR-24 (a released finding's guard), GitHub issue #31.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::collections::BTreeMap;
use std::sync::Mutex;

use lca_protocol::{CapabilityError, CompletionRequest, LoginAnswer, ProviderCap};
use openai_compatible::{Settings, StreamDriver, login_submit, profiles};

/// An in-memory credentials namespace plus a record of every request the
/// provider built - the two seams this issue is about.
#[derive(Default)]
struct FakeCap {
    credentials: Mutex<BTreeMap<String, String>>,
    requests: Mutex<Vec<String>>,
    /// Header lines per request, as `name: value`.
    headers: Mutex<Vec<Vec<String>>>,
}

impl FakeCap {
    fn with_credentials(pairs: &[(&str, &str)]) -> FakeCap {
        let cap = FakeCap::default();
        for (key, value) in pairs {
            cap.credentials_set(key, value).expect("seed");
        }
        cap
    }

    fn credential(&self, key: &str) -> Option<String> {
        self.credentials.lock().unwrap().get(key).cloned()
    }

    /// The URL the last request was built for.
    fn last_url(&self) -> String {
        self.requests
            .lock()
            .unwrap()
            .last()
            .cloned()
            .unwrap_or_default()
    }

    /// The header with `name` on the last request.
    fn last_header(&self, name: &str) -> Option<String> {
        self.headers
            .lock()
            .unwrap()
            .last()?
            .iter()
            .find(|line| line.starts_with(&format!("{name}: ")))
            .map(|line| line.trim_start_matches(&format!("{name}: ")).to_string())
    }
}

impl ProviderCap for FakeCap {
    fn net_request(
        &self,
        _method: &str,
        url: &str,
        headers: &[(&str, &str)],
        _body: Option<&[u8]>,
    ) -> Result<u32, CapabilityError> {
        self.requests.lock().unwrap().push(url.to_string());
        self.headers.lock().unwrap().push(
            headers
                .iter()
                .map(|(name, value)| format!("{name}: {value}"))
                .collect(),
        );
        Ok(1)
    }

    fn net_response_status(&self, _handle: u32) -> Result<u16, CapabilityError> {
        // 200 with no body: the completion requests under test stop at
        // the status, and an empty `GET /models` reads as "no discovery"
        // so `login_submit` falls back the way it does live.
        Ok(200)
    }

    fn net_read_body(&self, _handle: u32, _max: usize) -> Result<Option<Vec<u8>>, CapabilityError> {
        Ok(None)
    }

    fn net_close_response(&self, _handle: u32) -> Result<(), CapabilityError> {
        Ok(())
    }

    fn credentials_get(&self, key: &str) -> Option<String> {
        self.credential(key)
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

/// One login answer: the choice *is* the profile id, the values carry
/// what the picker collected.
fn answer(choice: &str, key: &str, base_url: &str) -> LoginAnswer {
    LoginAnswer {
        choice: choice.to_string(),
        values: [
            ("api-key".to_string(), key.to_string()),
            ("base-url".to_string(), base_url.to_string()),
        ]
        .into_iter()
        .collect(),
    }
}

/// The host's half of a login (ADR-0033): persist the opaque pairs the
/// extension returned - what `tui/login.rs` does through
/// `store_provider_secret`.
fn apply(cap: &FakeCap, settings: &[(String, String)]) {
    for (key, value) in settings {
        cap.credentials_set(key, value).expect("persist setting");
    }
}

/// One completion request for `model`.
fn request(model: &str) -> CompletionRequest {
    CompletionRequest {
        model: model.to_string(),
        ..CompletionRequest::default()
    }
}

// Verifies: gh #31's concurrency half - a second login adds a profile
// instead of overwriting the first, and a third login for the first
// profile updates that profile only. The bare `api_key`/`base_url` pair
// stays untouched: that is the default profile, and it is the legacy
// install's, not the last login's.
#[test]
fn two_logins_keep_both_profiles_credentials() {
    // The label and endpoint resolvers read the environment; the lock
    // keeps this test from seeing another's variables (testing-plan §5).
    let _env_lock = lca_testkit::fixture::env_lock();
    let cap = FakeCap::default();

    let first = login_submit(&cap, &answer("service-a", "key-a", "http://a.example/v1"))
        .expect("first login");
    apply(&cap, &first);
    let second = login_submit(&cap, &answer("service-b", "key-b", "http://b.example/v1"))
        .expect("second login");
    apply(&cap, &second);

    assert_eq!(
        cap.credential("profile.service-a.api_key").as_deref(),
        Some("key-a"),
        "the first profile's key survived the second login"
    );
    assert_eq!(
        cap.credential("profile.service-b.api_key").as_deref(),
        Some("key-b"),
        "the second profile's key is stored beside it"
    );
    assert!(
        first.iter().any(|(key, value)| {
            key == "profile.service-a.base_url" && value == "http://a.example/v1"
        }),
        "the first login returns its profile's base URL: {first:?}"
    );
    assert!(
        second.iter().any(|(key, value)| {
            key == "profile.service-b.base_url" && value == "http://b.example/v1"
        }),
        "the second login returns its profile's base URL: {second:?}"
    );
    assert_eq!(
        cap.credential("api_key"),
        None,
        "the legacy bare key is the default profile's, not a login's dump"
    );
    assert_eq!(
        cap.credential("base_url"),
        None,
        "the legacy bare base URL is the default profile's, not a login's dump"
    );

    // A third login for profile A updates A and leaves B alone.
    let again = login_submit(
        &cap,
        &answer("service-a", "key-a-2", "http://a2.example/v1"),
    )
    .expect("re-login");
    apply(&cap, &again);
    assert_eq!(
        cap.credential("profile.service-a.api_key").as_deref(),
        Some("key-a-2"),
        "re-login updates that profile"
    );
    assert_eq!(
        cap.credential("profile.service-b.api_key").as_deref(),
        Some("key-b"),
        "the other profile is untouched"
    );
    assert_eq!(
        cap.credential("profile.service-b.base_url").as_deref(),
        Some("http://b.example/v1"),
        "the other profile's endpoint is untouched"
    );
}

// Verifies: gh #31's routing half - a request for a model of profile P
// is built from P's base URL and P's key. The label promises which
// service and key will be billed; this is the request keeping that
// promise, asserted at the build seam both delivery modes share.
#[test]
fn a_request_for_a_models_profile_uses_that_profiles_credentials() {
    // The label and endpoint resolvers read the environment; the lock
    // keeps this test from seeing another's variables (testing-plan §5).
    let _env_lock = lca_testkit::fixture::env_lock();
    let cap = FakeCap::with_credentials(&[
        ("models", "model-a@service-a,model-b@service-b"),
        ("profile.service-a.base_url", "http://a.example/v1"),
        ("profile.service-a.api_key", "key-a"),
        ("profile.service-b.base_url", "http://b.example/v1"),
        ("profile.service-b.api_key", "key-b"),
    ]);

    StreamDriver::open(&cap, &Settings::default(), &request("model-a")).expect("open a");
    assert!(
        cap.last_url()
            .starts_with("http://a.example/v1/chat/completions"),
        "profile A's endpoint: {}",
        cap.last_url()
    );
    assert_eq!(
        cap.last_header("authorization").as_deref(),
        Some("Bearer key-a"),
        "profile A's key"
    );

    StreamDriver::open(&cap, &Settings::default(), &request("model-b")).expect("open b");
    assert!(
        cap.last_url()
            .starts_with("http://b.example/v1/chat/completions"),
        "profile B's endpoint: {}",
        cap.last_url()
    );
    assert_eq!(
        cap.last_header("authorization").as_deref(),
        Some("Bearer key-b"),
        "profile B's key"
    );
}

// Verifies: gh #31's backward tolerance - a bare `api_key`/`base_url`
// (and an untagged `models` entry) reads as the default profile exactly
// as it always did, so an install written before profiles behaves the
// same.
#[test]
fn legacy_bare_credentials_read_as_the_default_profile() {
    // The label and endpoint resolvers read the environment; the lock
    // keeps this test from seeing another's variables (testing-plan §5).
    let _env_lock = lca_testkit::fixture::env_lock();
    let cap = FakeCap::with_credentials(&[
        ("api_key", "legacy-key"),
        ("base_url", "http://legacy.example/v1"),
        ("models", "plain-model"),
    ]);

    StreamDriver::open(&cap, &Settings::default(), &request("plain-model")).expect("open");
    assert!(
        cap.last_url()
            .starts_with("http://legacy.example/v1/chat/completions"),
        "the bare base URL still serves: {}",
        cap.last_url()
    );
    assert_eq!(
        cap.last_header("authorization").as_deref(),
        Some("Bearer legacy-key"),
        "the bare key still authenticates"
    );
}

/// Sets environment variables for one test and restores them, holding
/// `env_lock` for the whole time (the documented discipline,
/// `lca-testkit::fixture`): the lock is what makes the mutation safe,
/// and it drops with this guard.
struct Env {
    _lock: std::sync::MutexGuard<'static, ()>,
    saved: Vec<(&'static str, Option<std::ffi::OsString>)>,
}

impl Env {
    fn set(pairs: &[(&'static str, &str)]) -> Env {
        let lock = lca_testkit::fixture::env_lock();
        let mut saved = Vec::new();
        for (key, value) in pairs {
            saved.push((*key, std::env::var_os(key)));
            // SAFETY: this test holds `env_lock` for its whole lifetime,
            // and every test that reads or writes the environment takes
            // the same lock.
            unsafe { std::env::set_var(key, value) };
        }
        Env { _lock: lock, saved }
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        for (key, value) in self.saved.drain(..) {
            // SAFETY: same lock discipline as `set` - `_lock` is dropped
            // after this runs.
            unsafe {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
    }
}

// Verifies: gh #31's env row - the environment keeps its documented
// precedence (environment over what login persisted,
// `docs/configuration.md`) and acts on the **default** profile only. A
// model owned by a named profile is that profile's, whatever the
// environment says.
#[test]
fn the_environment_wins_for_the_default_profile_only() {
    // `Env::set` takes the environment lock itself (and holds it for the
    // whole test), so this one does not take it a second time - the mutex
    // is not reentrant.
    let cap = FakeCap::with_credentials(&[
        ("api_key", "stored-key"),
        ("base_url", "http://stored.example/v1"),
        ("models", "plain-model,model-a@service-a"),
        ("profile.service-a.base_url", "http://a.example/v1"),
        ("profile.service-a.api_key", "key-a"),
    ]);
    let _env = Env::set(&[
        ("OPENAI_BASE_URL", "http://env.example/v1"),
        ("OPENAI_API_KEY", "env-key"),
    ]);

    StreamDriver::open(&cap, &Settings::default(), &request("plain-model")).expect("default");
    assert!(
        cap.last_url()
            .starts_with("http://env.example/v1/chat/completions"),
        "the environment beats the stored value for the default profile: {}",
        cap.last_url()
    );
    assert_eq!(
        cap.last_header("authorization").as_deref(),
        Some("Bearer env-key"),
        "the environment's key for the default profile"
    );

    StreamDriver::open(&cap, &Settings::default(), &request("model-a")).expect("profiled");
    assert!(
        cap.last_url()
            .starts_with("http://a.example/v1/chat/completions"),
        "a named profile is its own, environment notwithstanding: {}",
        cap.last_url()
    );
    assert_eq!(
        cap.last_header("authorization").as_deref(),
        Some("Bearer key-a"),
        "the named profile's key wins over the environment"
    );
}

// Verifies: gh #31's label half at its source - every model carries the
// profile it belongs to and the label its row shows, and no label is the
// extension's crate name (the picker row's rendering is asserted beside
// the renderer).
#[test]
fn every_model_carries_its_profile_and_a_service_label() {
    // The label and endpoint resolvers read the environment; the lock
    // keeps this test from seeing another's variables (testing-plan §5).
    let _env_lock = lca_testkit::fixture::env_lock();
    let cap = FakeCap::with_credentials(&[
        ("models", "plain-model,model-a@service-a,model-b@service-b"),
        ("base_url", "http://legacy.example/v1"),
    ]);
    let listed = profiles::picker_models(
        &cap,
        &Settings::default(),
        &cap.credential("models").unwrap_or_default(),
        "plain-model",
    );

    let labels: Vec<(&str, &str)> = listed
        .iter()
        .map(|model| (model.id.as_str(), model.label.as_str()))
        .collect();
    assert_eq!(
        labels,
        vec![
            ("plain-model", "legacy.example"),
            ("model-a", "service-a"),
            ("model-b", "service-b"),
        ],
        "the configured model leads; each row names its service: {labels:?}"
    );
    assert!(
        listed
            .iter()
            .all(|model| model.label != "openai-compatible"),
        "the crate name never labels a row: {listed:?}"
    );
    assert_eq!(
        listed[1].profile.as_deref(),
        Some("service-a"),
        "per-model provenance rides with the label"
    );
    assert_eq!(listed[0].profile, None, "the untagged model is default");
}
