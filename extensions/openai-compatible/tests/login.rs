//! The login presets (ADR-0031/0033): the extension loads its own resource,
//! maps presets to picker options, and consumes an answer.
//!
//! Verifies: ADR-0031, ADR-0033.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use std::collections::BTreeMap;
use std::sync::Mutex;

use lca_protocol::{CapabilityError, LoginAnswer, ProviderCap};
use openai_compatible::{
    Settings, login_options, login_submit, parse_presets, profiles, run_login, run_logout,
};

const PRESETS: &[u8] = include_bytes!("../resources/provider-presets.toml");

#[derive(Default)]
struct FakeCap {
    resources: BTreeMap<String, Vec<u8>>,
    creds: Mutex<BTreeMap<String, String>>,
    /// What `GET /models` answers: `None` is "nothing listens".
    models_body: Option<(u16, Vec<u8>)>,
    /// The URLs the cap was asked for, so a test can assert discovery ran.
    seen: Mutex<Vec<String>>,
    /// Whether the body has already been handed out (a response reads once).
    sent: Mutex<bool>,
}

impl ProviderCap for FakeCap {
    fn net_request(
        &self,
        _method: &str,
        url: &str,
        _headers: &[(&str, &str)],
        _body: Option<&[u8]>,
    ) -> Result<u32, CapabilityError> {
        self.seen.lock().expect("seen").push(url.to_string());
        if self.models_body.is_some() {
            Ok(1)
        } else {
            Err(CapabilityError::Io("nothing listens in this test".into()))
        }
    }
    fn net_response_status(&self, _handle: u32) -> Result<u16, CapabilityError> {
        self.models_body
            .as_ref()
            .map(|(status, _)| *status)
            .ok_or_else(|| CapabilityError::Io("nothing listens in this test".into()))
    }
    fn net_read_body(&self, _handle: u32, _max: usize) -> Result<Option<Vec<u8>>, CapabilityError> {
        // One chunk then end, like the real reader.
        if *self.sent.lock().expect("sent") {
            return Ok(None);
        }
        *self.sent.lock().expect("sent") = true;
        self.models_body
            .as_ref()
            .map(|(_, body)| Some(body.clone()))
            .ok_or_else(|| CapabilityError::Io("nothing listens in this test".into()))
    }
    fn net_close_response(&self, _handle: u32) -> Result<(), CapabilityError> {
        Ok(())
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

fn settings_for(base_url: &str, key: Option<&str>) -> Settings {
    Settings {
        base_url: base_url.to_string(),
        api_key: key.map(str::to_string),
        model: "test-model".to_string(),
        context_window: 0,
        prompt_cache_key: true,
        supports_reasoning: true,
        model_overrides: String::new(),
    }
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

    // Verifies: gh #182 - Vercel AI Gateway resolves as a preset, not
    // an extension: a bearer key step against the gateway's OpenAI
    // shape, with no curated models (live discovery wins).
    let vercel = options
        .iter()
        .find(|option| option.id == "vercel")
        .expect("vercel present");
    assert_eq!(vercel.host, "ai-gateway.vercel.sh");
    assert_eq!(vercel.kind, "api-key");
    assert!(vercel.fields.contains(&"api-key".to_string()));

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
        cap.credentials_get("profile.openrouter.api_key").as_deref(),
        Some("sk-secret"),
        "the key went to this login's profile in the extension's own namespace (gh #31)"
    );
    assert!(
        settings.iter().any(|(key, value)| {
            key == "profile.openrouter.base_url" && value == "https://openrouter.ai/api/v1"
        }),
        "the host is handed this profile's base URL to persist: {settings:?}"
    );

    let unknown = LoginAnswer {
        choice: "nope".to_string(),
        values: BTreeMap::new(),
    };
    assert!(login_submit(&cap, &unknown).is_err());
}

// Verifies: gh #188 - the custom endpoint is the extension's own
// preset, not host synthesis: kind `custom`, the three declared
// fields in prompt order, and no endpoint host of its own.
#[test]
fn the_custom_preset_is_an_extension_declared_option() {
    let options = login_options(&cap());
    let custom = options
        .iter()
        .find(|option| option.id == "custom")
        .expect("the custom preset is declared");
    assert_eq!(custom.kind, "custom");
    assert_eq!(
        custom.fields,
        vec![
            "base-url".to_string(),
            "api-key".to_string(),
            "model".to_string()
        ],
        "base URL, key, model - the old universal entry's order"
    );
    assert!(custom.host.is_empty(), "no endpoint of its own");
    assert_eq!(
        options.last().map(|option| option.id.as_str()),
        Some("custom"),
        "still last in the picker"
    );
}

// Verifies: gh #188 - a custom submit stores bare default-profile
// keys (like a direct setup) and hands the host bare pairs to
// persist, so `complete` reads them with no profile involved.
#[test]
fn a_custom_submit_stores_bare_default_profile_keys() {
    let cap = cap();
    let answer = LoginAnswer {
        choice: "custom".to_string(),
        values: [
            (
                "base-url".to_string(),
                "https://llm.example.com/v1".to_string(),
            ),
            ("api-key".to_string(), "sk-x".to_string()),
            ("model".to_string(), "m1".to_string()),
        ]
        .into_iter()
        .collect(),
    };
    let settings = login_submit(&cap, &answer).expect("submit");
    assert_eq!(
        cap.credentials_get("api_key").as_deref(),
        Some("sk-x"),
        "the key went to the bare default-profile key"
    );
    assert!(
        settings
            .iter()
            .any(|(key, value)| { key == "base_url" && value == "https://llm.example.com/v1" }),
        "the host is handed the bare base URL to persist: {settings:?}"
    );
    assert!(
        settings
            .iter()
            .any(|(key, value)| key == "model" && value == "m1"),
        "and the bare model: {settings:?}"
    );
    assert!(
        settings.iter().all(|(key, _)| !key.starts_with("profile.")),
        "nothing profile-scoped: {settings:?}"
    );
}

// Verifies: parse_presets tolerates junk (a hostile or truncated resource
// yields no options rather than a panic).
#[test]
fn parse_presets_tolerates_junk() {
    assert!(parse_presets("not toml [[[").is_empty());
    assert!(parse_presets("").is_empty());
    assert!(parse_presets("[[preset]]\nname = \"missing id\"").is_empty());
}

#[cfg(test)]
mod discovery {
    use super::*;

    fn cap_with(body: Option<(u16, Vec<u8>)>) -> FakeCap {
        let mut cap = FakeCap::default();
        cap.resources
            .insert("provider-presets.toml".to_string(), PRESETS.to_vec());
        cap.models_body = body;
        cap
    }

    fn answer() -> LoginAnswer {
        LoginAnswer {
            choice: "openrouter".to_string(),
            values: [("api-key".to_string(), "sk-x".to_string())]
                .into_iter()
                .collect(),
        }
    }

    fn setting<'a>(settings: &'a [(String, String)], key: &str) -> Option<&'a str> {
        settings
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    // D2: the endpoint's own list wins when it answers.
    #[test]
    fn login_submit_discovers_the_endpoints_models_when_it_answers() {
        let body = br#"{"data":[{"id":"zeta"},{"id":"alpha"},{"id":"alpha"}]}"#.to_vec();
        let cap = cap_with(Some((200, body)));
        let settings = login_submit(&cap, &answer()).expect("submit");
        assert_eq!(
            setting(&settings, "models"),
            Some("alpha@openrouter,zeta@openrouter"),
            "sorted and deduplicated, each entry tagged with the profile it belongs to (gh #31)"
        );
        assert!(
            cap.seen.lock().expect("seen")[0].ends_with("/models"),
            "discovery hit the endpoint"
        );
    }

    // D2's fallback: an unreachable endpoint means the curated short list,
    // not an empty picker and not an error.
    #[test]
    fn login_submit_falls_back_to_the_curated_list_when_the_endpoint_is_unreachable() {
        let cap = cap_with(None);
        let settings = login_submit(&cap, &answer()).expect("submit");
        let models = setting(&settings, "models").expect("a list either way");
        assert!(
            models.contains("openai/gpt-4o"),
            "the preset's curated ids: {models}"
        );
    }

    // A live but wrong answer (401, or a body that is not the expected
    // shape) is a fall-back, never a panic and never a bogus list.
    #[test]
    fn login_submit_treats_an_unexpected_answer_as_no_discovery() {
        for body in [
            (401u16, br#"{"error":{"message":"bad key"}}"#.to_vec()),
            (200, br#"{"not":"a model list"}"#.to_vec()),
            (200, br#"{"data":[]}"#.to_vec()),
        ] {
            let cap = cap_with(Some(body));
            let settings = login_submit(&cap, &answer()).expect("submit");
            assert_eq!(
                setting(&settings, "models"),
                Some(
                    "openai/gpt-4o@openrouter,anthropic/claude-3.5-sonnet@openrouter,\
                 google/gemini-2.0-flash@openrouter",
                ),
                "the curated list is the fallback"
            );
        }
    }

    // Verifies: gh #34 - a `GET /models` that carries a limit writes it
    // into the setting the host persists as `id=window`, and an entry
    // that carries none stays a bare id: the shape every list written
    // before gh #34 already has, so the change is readable both ways.
    // (`context_length` is the field an OpenAI-shaped endpoint actually
    // reports - OpenRouter's live answer, checked 2026-10-03.)
    #[test]
    fn login_submit_keeps_a_reported_context_limit_in_the_models_setting() {
        let body = br#"{
            "data": [
                {"id": "mimo-v2.6-flash", "context_length": 262144},
                {"id": "no-limit-model"},
                {"id": "mimo-v2.6-flash", "context_length": 262144}
            ]
        }"#
        .to_vec();
        let cap = cap_with(Some((200, body)));
        let settings = login_submit(&cap, &answer()).expect("submit");
        assert_eq!(
            setting(&settings, "models"),
            Some("mimo-v2.6-flash@openrouter=262144,no-limit-model@openrouter"),
            "sorted, deduplicated, and the endpoint's own limit rides along"
        );
    }
}

// Verifies: gh #211 - a credential-free endpoint (`auth = "none"`)
// signs in with no key anywhere: neither the namespace nor the
// settings carry one, and the attempt still succeeds so the catalog
// probe (and `auth check`) reports ready.
#[test]
fn a_credential_free_endpoint_signs_in_without_a_key() {
    use lca_protocol::IdentityOutcome;
    let cap = cap();
    let settings = settings_for("http://localhost:11434/v1", None);
    assert!(
        matches!(run_login(&cap, &settings), IdentityOutcome::Ok),
        "the ollama preset needs no key"
    );
    assert!(
        cap.credentials_get("api_key").is_none(),
        "no key is invented where none is needed"
    );
}

// Verifies: gh #293 - logout is the inverse of login. The login path
// writes `profile.<id>.api_key`, `profile.<id>.base_url`, plus `models`,
// `model`, and `preset`; the old `run_logout` deleted only bare
// `api_key`/`base_url`, leaving every profile secret on disk.
#[test]
fn logout_purges_every_key_login_wrote() {
    use lca_protocol::IdentityOutcome;
    let cap = cap();
    // A login into a named preset, as the host persists it (the extension's
    // `login_submit` writes the profile key; the host persists the rest).
    cap.credentials_set("profile.deepseek.api_key", "FAKESECRET-293-DEEP")
        .expect("seed");
    cap.credentials_set("profile.deepseek.base_url", "https://api.deepseek.com/v1")
        .expect("seed");
    cap.credentials_set("profile.groq.api_key", "FAKESECRET-293-GROQ")
        .expect("seed");
    cap.credentials_set("models", "m1@deepseek,m2@groq")
        .expect("seed");
    cap.credentials_set("model", "m1").expect("seed");
    cap.credentials_set("preset", "deepseek").expect("seed");
    cap.credentials_set("api_key", "FAKESECRET-293-BARE")
        .expect("seed");
    cap.credentials_set("base_url", "https://api.deepseek.com/v1")
        .expect("seed");

    assert!(matches!(run_logout(&cap), IdentityOutcome::Ok));

    let remaining = cap.creds.lock().expect("creds").clone();
    assert!(
        remaining.is_empty(),
        "every key class is gone, not just the bare pair: {remaining:?}"
    );
}

// Verifies: gh #293 - logout is harmless when nothing is stored (a
// keyless `auth = "none"` preset, e.g. a local endpoint).
#[test]
fn logout_of_an_empty_store_is_ok() {
    use lca_protocol::IdentityOutcome;
    let cap = cap();
    assert!(matches!(run_logout(&cap), IdentityOutcome::Ok));
}

// Verifies: gh #293 - a leftover `profile.<id>.api_key` whose preset id
// has since vanished from the merged table cannot be enumerated and is the
// documented ceiling; a preset that IS in the table is always purged.
#[test]
fn logout_purges_a_preset_that_is_still_in_the_table() {
    use lca_protocol::IdentityOutcome;
    let cap = cap();
    // `openrouter` ships in the resource, so logout must reach its keys.
    cap.credentials_set("profile.openrouter.api_key", "FAKESECRET-293-OR")
        .expect("seed");
    assert!(matches!(run_logout(&cap), IdentityOutcome::Ok));
    assert!(
        cap.credentials_get("profile.openrouter.api_key").is_none(),
        "a shipped preset's profile key is purged"
    );
}

// Verifies: gh #293 AC2 - after logout the provider is unconfigured: a
// bearer endpoint with no environment key is not ready, because no stored
// key survives (the old logout left `profile.<id>.api_key` and the
// provider stayed ready).
#[test]
fn the_provider_is_unconfigured_after_logout() {
    use lca_protocol::IdentityOutcome;
    let cap = cap();
    cap.credentials_set("profile.deepseek.api_key", "FAKESECRET-293-RDY")
        .expect("seed");
    cap.credentials_set("models", "m@deepseek").expect("seed");
    assert!(matches!(run_logout(&cap), IdentityOutcome::Ok));

    // The readiness path resolves a key from the store; with nothing left
    // and no environment key, a bearer endpoint is not ready.
    let settings = settings_for("https://api.deepseek.com/v1", None);
    assert!(
        matches!(run_login(&cap, &settings), IdentityOutcome::Failed(_)),
        "after logout the provider reports unconfigured"
    );
}

// Verifies: gh #211 - the keyless path never weakens the keyed one: a
// bearer endpoint with no key anywhere still fails, so it stays out
// of the catalog.
#[test]
fn a_bearer_endpoint_without_a_key_still_fails() {
    use lca_protocol::IdentityOutcome;
    let cap = cap();
    let settings = settings_for("https://api.openai.com/v1", None);
    assert!(
        matches!(run_login(&cap, &settings), IdentityOutcome::Failed(_)),
        "a bearer endpoint without a key is not ready"
    );
}

// ---------------------------------------------------------------------------
// gh #292: preset-declared environment variables must authenticate
// ---------------------------------------------------------------------------

/// Run `body` with exactly `vars` set, every other name cleared.
///
/// The house pattern for a test that touches the process environment
/// (`lca_testkit::fixture::env_lock`, as `tests/models.rs` uses) serializes
/// it against the rest of this crate's env-touching rows; the save/restore
/// keeps a panic from leaking a key into a later test.
fn with_env<F: FnOnce()>(vars: &[(&str, &str)], body: F) {
    const NAMES: &[&str] = &[
        "OPENAI_API_KEY",
        "OPENCODE_API_KEY",
        "DEEPSEEK_API_KEY",
        "GROQ_API_KEY",
        "MISTRAL_API_KEY",
        "LCA_TEST_292_FIRST",
        "LCA_TEST_292_SECOND",
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

// Verifies: gh #292 acceptance 1 and 3 - the documented variable for a
// preset authenticates that preset's profile.
#[test]
fn a_preset_declared_environment_variable_authenticates_its_profile() {
    with_env(&[("DEEPSEEK_API_KEY", "FAKESECRET-292-DEEP")], || {
        let cap = cap();
        let settings = settings_for("https://api.deepseek.com/v1", None);
        assert_eq!(
            profiles::api_key_for(&cap, &settings, &Some("deepseek".to_string())).as_deref(),
            Some("FAKESECRET-292-DEEP"),
            "DEEPSEEK_API_KEY is the preset's own documented variable"
        );
    });
}

// Verifies: gh #292 - the cross-provider negative. Only the *active*
// preset's names are consulted, so a key exported for another service
// never authenticates this one. Without this row the fix degenerates into
// a multi-key grab bag that sends the wrong secret to the wrong endpoint.
#[test]
fn another_providers_environment_variable_never_authenticates_this_profile() {
    with_env(&[("GROQ_API_KEY", "FAKESECRET-292-GROQ")], || {
        let cap = cap();
        let settings = settings_for("https://api.deepseek.com/v1", None);
        assert_eq!(
            profiles::api_key_for(&cap, &settings, &Some("deepseek".to_string())),
            None,
            "GROQ_API_KEY belongs to groq, not deepseek"
        );
    });
}

// Verifies: gh #292 acceptance 2 - a stored credential still works when no
// environment variable is set.
#[test]
fn a_stored_profile_credential_still_authenticates_without_the_environment() {
    with_env(&[], || {
        let cap = cap();
        cap.credentials_set("profile.deepseek.api_key", "FAKESECRET-292-STORED")
            .expect("seed");
        let settings = settings_for("https://api.deepseek.com/v1", None);
        assert_eq!(
            profiles::api_key_for(&cap, &settings, &Some("deepseek".to_string())).as_deref(),
            Some("FAKESECRET-292-STORED")
        );
    });
}

// Verifies: gh #292 - the environment beats what login persisted, keeping
// the documented ladder one shape for every profile.
#[test]
fn a_preset_declared_variable_beats_the_stored_credential() {
    with_env(&[("DEEPSEEK_API_KEY", "FAKESECRET-292-ENV")], || {
        let cap = cap();
        cap.credentials_set("profile.deepseek.api_key", "FAKESECRET-292-STORED")
            .expect("seed");
        let settings = settings_for("https://api.deepseek.com/v1", None);
        assert_eq!(
            profiles::api_key_for(&cap, &settings, &Some("deepseek".to_string())).as_deref(),
            Some("FAKESECRET-292-ENV")
        );
    });
}

// Verifies: gh #292 - a preset's own names resolve in declaration order.
//
// The shipped table cannot show this: `opencode-go` is its only two-name
// preset and both names are the *universal* ones, which the documented
// ladder resolves at step 1 whatever a preset says. So this row injects a
// two-name preset through the capability's own resource read - the same
// seam the real table arrives through - which is the only way to observe
// order at all. Both names are chosen so neither is universal.
#[test]
fn a_presets_own_names_resolve_in_declaration_order() {
    const TWO_NAMES: &str = r#"
[[preset]]
id = "twonames"
name = "Two Names"
base_url = "https://two.example/v1"
env = ["LCA_TEST_292_FIRST", "LCA_TEST_292_SECOND"]
"#;
    with_env(
        &[
            ("LCA_TEST_292_FIRST", "FAKESECRET-292-FIRST"),
            ("LCA_TEST_292_SECOND", "FAKESECRET-292-SECOND"),
        ],
        || {
            let mut cap = cap();
            cap.resources.insert(
                "provider-presets.toml".to_string(),
                TWO_NAMES.as_bytes().to_vec(),
            );
            let settings = settings_for("https://two.example/v1", None);
            assert_eq!(
                profiles::api_key_for(&cap, &settings, &Some("twonames".to_string())).as_deref(),
                Some("FAKESECRET-292-FIRST"),
                "the name declared first wins"
            );
        },
    );
}

// Verifies: gh #292 - a profile with no discoverable preset reads no
// environment at all. Guessing from the base URL could authenticate a
// profile with a key the user never associated with it.
#[test]
fn a_profile_without_a_matching_preset_reads_no_environment() {
    with_env(
        &[
            ("DEEPSEEK_API_KEY", "FAKESECRET-292-DEEP"),
            ("GROQ_API_KEY", "FAKESECRET-292-GROQ"),
        ],
        || {
            let cap = cap();
            cap.credentials_set("profile.unknown.api_key", "FAKESECRET-292-OWN")
                .expect("seed");
            let settings = settings_for("https://example.invalid/v1", None);
            assert_eq!(
                profiles::api_key_for(&cap, &settings, &Some("unknown".to_string())).as_deref(),
                Some("FAKESECRET-292-OWN"),
                "no matching preset: the stored credential is the only source"
            );
        },
    );
}

// Verifies: gh #292 - the default profile keeps resolving the two universal
// names it always did, so an install configured by the environment alone is
// unchanged.
#[test]
fn the_default_profile_still_reads_the_universal_names() {
    with_env(&[("OPENAI_API_KEY", "FAKESECRET-292-UNIVERSAL")], || {
        let cap = cap();
        let settings = settings_for("https://api.openai.com/v1", None);
        assert_eq!(
            profiles::api_key_for(&cap, &settings, &None).as_deref(),
            Some("FAKESECRET-292-UNIVERSAL")
        );
    });
}

// Verifies: gh #292 does not change gh #31's fallback - a named profile
// with nothing of its own still reaches the bare stored key, which is how
// an `auth = "none"` preset setup gets to its endpoint. Inserting the
// preset lookup must not quietly drop that step.
#[test]
fn a_named_profile_still_falls_back_to_the_bare_stored_key() {
    with_env(&[], || {
        let cap = cap();
        cap.credentials_set("api_key", "FAKESECRET-292-BARE")
            .expect("seed");
        let settings = settings_for("https://api.deepseek.com/v1", None);
        assert_eq!(
            profiles::api_key_for(&cap, &settings, &Some("deepseek".to_string())).as_deref(),
            Some("FAKESECRET-292-BARE")
        );
    });
}

// Verifies: gh #292 - `OPENAI_API_KEY` authenticates the *default* profile
// only, and never a named one. The universal name is this provider's
// default-profile name; resolving it for every profile would send an
// OpenAI key to Groq and DeepSeek the moment a user logged into them, which
// is the same wrong-secret-to-the-wrong-endpoint leak the cross-provider
// rule prevents - arriving through the front door instead.
#[test]
fn the_universal_names_never_authenticate_a_named_profile() {
    with_env(
        &[
            ("OPENAI_API_KEY", "FAKESECRET-292-UNIVERSAL"),
            ("OPENCODE_API_KEY", "FAKESECRET-292-UNIVERSAL-2"),
        ],
        || {
            let cap = cap();
            cap.credentials_set("profile.deepseek.api_key", "FAKESECRET-292-OWN")
                .expect("seed");
            let settings = settings_for("https://api.deepseek.com/v1", None);
            assert_eq!(
                profiles::api_key_for(&cap, &settings, &Some("deepseek".to_string())).as_deref(),
                Some("FAKESECRET-292-OWN"),
                "a named profile keeps its own key; the universal name is the \
                 default profile's"
            );
            // And with nothing stored, the named profile still reads no
            // universal name: no key is the honest answer.
            let bare = crate::cap();
            assert_eq!(
                profiles::api_key_for(&bare, &settings, &Some("deepseek".to_string())),
                None,
                "no stored key and no preset variable: nothing to send"
            );
        },
    );
}

// Verifies: gh #292 - the environment-derived `settings.api_key` never
// reaches a named profile. `settings.api_key` is read from the universal
// variables, so a named profile that fell back to it would carry the
// *default* profile's secret to another service. This is the row that pins
// the fix's last step: a stored bare key is still reachable (`auth:
// "none"` setups need it), but the environment copy beside it is not.
#[test]
fn the_environment_derived_settings_key_never_reaches_a_named_profile() {
    with_env(&[], || {
        let cap = cap();
        // The environment's key, as `Settings::default` would have read it.
        let settings = settings_for(
            "https://api.deepseek.com/v1",
            Some("FAKESECRET-292-FROM-ENV"),
        );
        assert_eq!(
            profiles::api_key_for(&cap, &settings, &Some("deepseek".to_string())),
            None,
            "a deepseek profile must not receive OPENAI_API_KEY's value"
        );
        // The default profile still uses it: that is its own slot.
        assert_eq!(
            profiles::api_key_for(&cap, &settings, &None).as_deref(),
            Some("FAKESECRET-292-FROM-ENV")
        );
    });
}
