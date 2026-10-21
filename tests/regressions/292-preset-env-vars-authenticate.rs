//! GitHub #292: `provider-presets.toml` documents a canonical environment
//! variable per service (`DEEPSEEK_API_KEY`, `GROQ_API_KEY`,
//! `MISTRAL_API_KEY`, …) and `Preset` parsed the list, but
//! `profiles::api_key_for` consulted only `OPENAI_API_KEY` and
//! `OPENCODE_API_KEY`.
//!
//! The harm is a documented setup that silently does not work. A developer
//! who exports the variable the provider's own page names is told to
//! authenticate: nothing stores a key, the request goes out with no
//! `authorization` header, and the endpoint answers 401. The only ways out
//! are aliasing the variable to `OPENAI_API_KEY` or typing the secret into
//! `/login` — both of which defeat the point of declaring the name in the
//! table. So the `env` column was decorative data, and the promise it makes
//! was empty.
//!
//! This guard drives the real seams: the shipped preset resource, the real
//! `StreamDriver`, an in-memory `ProviderCap`. It asserts on the header the
//! endpoint would receive, which is the exact thing that was missing.
//!
//! The failure mode gh #292's fix must *not* introduce is the opposite one:
//! resolving some other service's variable, which sends a valid secret to
//! the wrong endpoint. That row is here too — it is the reason the fix
//! consults only the active preset's names rather than every name in the
//! table.
//!
//! Verifies: NFR-24 (a shipped defect's guard), GitHub issue #292,
//! gh #292 acceptance 1–3.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::collections::BTreeMap;
use std::sync::Mutex;

use lca_protocol::{CapabilityError, CompletionRequest, ProviderCap};
use openai_compatible::{Settings, StreamDriver};

const PRESETS: &[u8] =
    include_bytes!("../../extensions/openai-compatible/resources/provider-presets.toml");

/// In-memory credentials plus a record of the requests the provider built.
/// The two seams this defect lives between: what is stored, and what the
/// endpoint actually receives.
#[derive(Default)]
struct FakeCap {
    credentials: Mutex<BTreeMap<String, String>>,
    headers: Mutex<Vec<Vec<String>>>,
    urls: Mutex<Vec<String>>,
}

impl FakeCap {
    fn with_credentials(pairs: &[(&str, &str)]) -> FakeCap {
        let cap = FakeCap::default();
        for (key, value) in pairs {
            cap.credentials_set(key, value).expect("seed");
        }
        cap
    }

    fn last_header(&self, name: &str) -> Option<String> {
        let prefix = format!("{name}: ");
        self.headers
            .lock()
            .unwrap()
            .last()?
            .iter()
            .find(|line| line.starts_with(&prefix))
            .map(|line| line.trim_start_matches(&prefix).to_string())
    }

    fn last_url(&self) -> String {
        self.urls
            .lock()
            .unwrap()
            .last()
            .cloned()
            .unwrap_or_default()
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
        self.urls.lock().unwrap().push(url.to_string());
        self.headers.lock().unwrap().push(
            headers
                .iter()
                .map(|(name, value)| format!("{name}: {value}"))
                .collect(),
        );
        Ok(1)
    }

    fn net_response_status(&self, _handle: u32) -> Result<u16, CapabilityError> {
        // 200 with no body: these requests stop at the status + headers,
        // which is where the credential question is decided.
        Ok(200)
    }

    fn net_read_body(&self, _handle: u32, _max: usize) -> Result<Option<Vec<u8>>, CapabilityError> {
        Ok(None)
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

    fn resource_read(&self, path: &str) -> Result<Vec<u8>, CapabilityError> {
        if path == "provider-presets.toml" {
            return Ok(PRESETS.to_vec());
        }
        Err(CapabilityError::NotFound(path.to_string()))
    }
}

/// `body` with exactly `vars` set, every other name this guard touches
/// cleared. Serialized through the house `env_lock` so a parallel nextest
/// run cannot interleave another row's variables into this one.
fn with_env<F: FnOnce()>(vars: &[(&str, &str)], body: F) {
    const NAMES: &[&str] = &[
        "OPENAI_API_KEY",
        "OPENCODE_API_KEY",
        "DEEPSEEK_API_KEY",
        "GROQ_API_KEY",
        "MISTRAL_API_KEY",
        "OPENAI_BASE_URL",
    ];
    let _guard = lca_testkit::fixture::env_lock();
    let saved: Vec<(&str, Option<String>)> = NAMES
        .iter()
        .map(|name| (*name, std::env::var(name).ok()))
        .collect();
    for name in NAMES {
        unsafe { std::env::remove_var(name) };
    }
    for (name, value) in vars {
        unsafe { std::env::set_var(name, value) };
    }
    body();
    for (name, value) in saved {
        match value {
            Some(value) => unsafe { std::env::set_var(name, value) },
            None => unsafe { std::env::remove_var(name) },
        }
    }
}

fn request(model: &str) -> CompletionRequest {
    CompletionRequest {
        model: model.to_string(),
        ..CompletionRequest::default()
    }
}

/// The shipped defect: a `deepseek` profile, the documented variable
/// exported, no stored key. The request must carry the key.
///
/// Before the fix the header was simply absent — the request reached
/// DeepSeek unauthenticated and came back 401, which is exactly what the
/// issue reports.
#[test]
fn the_documented_variable_for_a_profile_reaches_its_endpoint_as_a_bearer() {
    with_env(&[("DEEPSEEK_API_KEY", "FAKESECRET-292-DEEP")], || {
        let cap = FakeCap::with_credentials(&[
            ("profile.deepseek.base_url", "https://api.deepseek.com/v1"),
            // No `profile.deepseek.api_key`: the environment is the only
            // source, which is the scenario the issue describes.
            ("models", "deepseek-chat@deepseek"),
        ]);
        StreamDriver::open(&cap, &Settings::default(), &request("deepseek-chat"))
            .expect("the driver builds the request");
        assert_eq!(
            cap.last_header("authorization").as_deref(),
            Some("Bearer FAKESECRET-292-DEEP"),
            "the preset's documented variable is the credential"
        );
        assert!(
            cap.last_url().starts_with("https://api.deepseek.com/v1/"),
            "and it goes to that preset's endpoint: {}",
            cap.last_url()
        );
    });
}

/// The regression the fix must not introduce: another service's key must
/// never be sent. A `GROQ_API_KEY` in the environment leaves the DeepSeek
/// request unauthenticated — a loud 401 is correct, a valid-looking Groq
/// secret handed to DeepSeek is a credential leak.
#[test]
fn another_services_variable_never_reaches_this_endpoint() {
    with_env(&[("GROQ_API_KEY", "FAKESECRET-292-GROQ")], || {
        let cap = FakeCap::with_credentials(&[
            ("profile.deepseek.base_url", "https://api.deepseek.com/v1"),
            ("models", "deepseek-chat@deepseek"),
        ]);
        StreamDriver::open(&cap, &Settings::default(), &request("deepseek-chat"))
            .expect("the driver builds the request");
        assert_eq!(
            cap.last_header("authorization"),
            None,
            "GROQ_API_KEY is not deepseek's credential, so no header is sent"
        );
    });
}

/// Acceptance 2: the stored credential is still what authenticates when no
/// environment variable is set. A fix for #292 must not shadow it.
#[test]
fn the_stored_key_still_authenticates_without_the_environment() {
    with_env(&[], || {
        let cap = FakeCap::with_credentials(&[
            ("profile.deepseek.api_key", "FAKESECRET-292-STORED"),
            ("profile.deepseek.base_url", "https://api.deepseek.com/v1"),
            ("models", "deepseek-chat@deepseek"),
        ]);
        StreamDriver::open(&cap, &Settings::default(), &request("deepseek-chat"))
            .expect("the driver builds the request");
        assert_eq!(
            cap.last_header("authorization").as_deref(),
            Some("Bearer FAKESECRET-292-STORED")
        );
    });
}

/// Every preset that declares a variable must actually resolve it. This is
/// the row that keeps the fix honest across the whole table rather than
/// just the two presets named in the issue: a preset added later with an
/// `env` entry gets authentication for free, and one whose list is
/// misspelled fails here instead of at a user's 401.
#[test]
fn every_preset_that_declares_a_variable_resolves_it() {
    let presets = openai_compatible::parse_presets(&String::from_utf8_lossy(PRESETS));
    assert!(presets.len() >= 15, "the shipped table's presets parse");
    let declaring: Vec<_> = presets
        .iter()
        .filter(|preset| !preset.env.is_empty())
        .collect();
    assert!(
        declaring.len() >= 18,
        "the table declares variables for most presets: {}",
        declaring.len()
    );
    let _guard = lca_testkit::fixture::env_lock();
    for preset in declaring {
        // Every one of the preset's own names, one at a time, so a row
        // that only happened to work because of a universal name would
        // still be exercised on its distinctive one.
        for name in &preset.env {
            let saved = std::env::var(name).ok();
            unsafe { std::env::set_var(name, "FAKESECRET-292-SWEEP") };
            let cap = FakeCap::default();
            let settings = Settings::default();
            let resolved =
                openai_compatible::profiles::api_key_for(&cap, &settings, &Some(preset.id.clone()));
            match saved {
                Some(value) => unsafe { std::env::set_var(name, value) },
                None => unsafe { std::env::remove_var(name) },
            }
            assert_eq!(
                resolved.as_deref(),
                Some("FAKESECRET-292-SWEEP"),
                "preset `{}` declares `{name}` but does not resolve it",
                preset.id
            );
        }
    }
}
