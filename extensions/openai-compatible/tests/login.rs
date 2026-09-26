//! The login presets (ADR-0031/0033): the extension loads its own resource,
//! maps presets to picker options, and consumes an answer.
//!
//! Verifies: ADR-0031, ADR-0033.

use std::collections::BTreeMap;
use std::sync::Mutex;

use lca_protocol::{CapabilityError, LoginAnswer, ProviderCap};
use openai_compatible::{login_options, login_submit, parse_presets};

const PRESETS: &[u8] = include_bytes!("../resources/provider-presets.toml");

#[derive(Default)]
struct FakeCap {
    resources: BTreeMap<String, Vec<u8>>,
    creds: Mutex<BTreeMap<String, String>>,
}

impl ProviderCap for FakeCap {
    fn net_request(
        &self,
        _method: &str,
        _url: &str,
        _headers: &[(&str, &str)],
        _body: Option<&[u8]>,
    ) -> Result<u32, CapabilityError> {
        Err(CapabilityError::Io("no net in this test".into()))
    }
    fn net_response_status(&self, _handle: u32) -> Result<u16, CapabilityError> {
        Err(CapabilityError::Io("no net in this test".into()))
    }
    fn net_read_body(&self, _handle: u32, _max: usize) -> Result<Option<Vec<u8>>, CapabilityError> {
        Err(CapabilityError::Io("no net in this test".into()))
    }
    fn net_close_response(&self, _handle: u32) -> Result<(), CapabilityError> {
        Err(CapabilityError::Io("no net in this test".into()))
    }
    fn credentials_get(&self, key: &str) -> Option<String> {
        self.creds.lock().expect("creds").get(key).cloned()
    }
    fn credentials_set(&self, key: &str, value: &str) -> Result<(), CapabilityError> {
        self.creds
            .lock()
            .expect("creds")
            .insert(key.to_string(), value.to_string());
        Ok(())
    }
    fn credentials_delete(&self, key: &str) -> Result<(), CapabilityError> {
        self.creds.lock().expect("creds").remove(key);
        Ok(())
    }
    fn resource_read(&self, path: &str) -> Result<Vec<u8>, CapabilityError> {
        self.resources
            .get(path)
            .cloned()
            .ok_or_else(|| CapabilityError::NotFound(path.to_string()))
    }
}

fn cap() -> FakeCap {
    let mut cap = FakeCap::default();
    cap.resources
        .insert("provider-presets.toml".to_string(), PRESETS.to_vec());
    cap
}

// Verifies: ADR-0031 (presets are the extension's own resource data, mapped
// to the host's picker options).
#[test]
fn the_preset_resource_parses_into_the_picker_options() {
    let options = login_options(&cap());
    assert!(options.len() >= 15, "the registry carries the endpoints");
    let openrouter = options
        .iter()
        .find(|option| option.id == "openrouter")
        .expect("openrouter present");
    assert_eq!(openrouter.host, "openrouter.ai");
    assert_eq!(openrouter.kind, "api-key");
    assert!(openrouter.fields.contains(&"api-key".to_string()));
    assert_eq!(
        openrouter.extras.get("base_url").map(String::as_str),
        Some("https://openrouter.ai/api/v1")
    );

    let ollama = options
        .iter()
        .find(|option| option.id == "ollama")
        .expect("ollama present");
    assert!(ollama.fields.is_empty(), "a local preset has no key step");
    assert_eq!(ollama.host, "localhost");
}

// Verifies: ADR-0033 (login-submit stores the secret in the extension's own
// namespace and returns opaque settings for the host to persist).
#[test]
fn login_submit_stores_the_key_and_returns_opaque_settings() {
    let cap = cap();
    let answer = LoginAnswer {
        choice: "openrouter".to_string(),
        values: [("api-key".to_string(), "sk-secret".to_string())]
            .into_iter()
            .collect(),
    };
    let settings = login_submit(&cap, &answer).expect("submit");
    assert_eq!(
        cap.credentials_get("api_key").as_deref(),
        Some("sk-secret"),
        "the key went to the extension's own credentials namespace"
    );
    assert!(
        settings
            .iter()
            .any(|(key, value)| key == "base_url" && value == "https://openrouter.ai/api/v1"),
        "the host is handed the base URL to persist: {settings:?}"
    );

    let unknown = LoginAnswer {
        choice: "nope".to_string(),
        values: BTreeMap::new(),
    };
    assert!(login_submit(&cap, &unknown).is_err());
}

// Verifies: parse_presets tolerates junk (a hostile or truncated resource
// yields no options rather than a panic).
#[test]
fn parse_presets_tolerates_junk() {
    assert!(parse_presets("not toml [[[").is_empty());
    assert!(parse_presets("").is_empty());
    assert!(parse_presets("[[preset]]\nname = \"missing id\"").is_empty());
}
