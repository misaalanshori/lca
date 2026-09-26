//! The `/login` flow (`api-key-login-plan.md` D1, ADR-0033): the host is
//! UI + courier + consent. It renders whatever the extension hands it,
//! ferries the typed values back, persists the opaque settings it is given,
//! and runs the ad hoc `net` grant (FR-PERM-16).
//!
//! Nothing here interprets provider-shaped data. Field ids are the neutral
//! ones ADR-0033 names (`api-key`, `base-url`, `model`); an unknown id is
//! prompted for generically and passed through untouched.
//!
//! This module holds no I/O - the flow is a state machine over values - so
//! the caller owns the credential write, the extension call, and the grant.

use std::collections::BTreeMap;

use lca_protocol::LoginOption;
use lca_tui::{CUSTOM_OPTION, LoginNext, PickerOption};

/// The host-owned universal entry's fields, in prompt order (D1: base URL +
/// key + model). A preset declares its own; this one never does.
pub const CUSTOM_FIELDS: &[&str] = &["base-url", "api-key", "model"];

/// What one field asks for: the label to show, and whether to mask it.
///
/// A URL or a model id is shown as typed. Typing those blind is worse than
/// any leak of a value that is not secret; a key is always masked.
pub fn field_prompt(field: &str) -> (String, bool) {
    match field {
        "api-key" | "api_key" | "key" | "token" => ("API key (input hidden)".to_string(), true),
        "base-url" | "base_url" => ("Base URL (e.g. https://example.com/v1)".to_string(), false),
        "model" => ("Model id".to_string(), false),
        other => (format!("{other}:"), false),
    }
}

/// The host's picker row for one option. The label and hint are display
/// strings the extension produced; the host never derives meaning from them.
pub fn picker_row(provider: &str, option: &LoginOption) -> PickerOption {
    PickerOption {
        provider: provider.to_string(),
        id: option.id.clone(),
        label: option.name.clone(),
        hint: option.host.clone(),
    }
}

/// What [`LoginFlow::push`] produced: either the next prompt, or the
/// collected answers for the caller to submit.
pub enum Step {
    /// Ask for one more value.
    Next(LoginNext),
    /// Every field is answered; submit these.
    Submit {
        /// The provider being signed in to.
        provider: String,
        /// The chosen option id ([`CUSTOM_OPTION`] for the host's entry).
        choice: String,
        /// Field id -> value, in the order collected.
        values: BTreeMap<String, String>,
    },
}

struct Pending {
    provider: String,
    choice: String,
    fields: Vec<String>,
    answers: BTreeMap<String, String>,
}

/// The `/login` state: what the picker offered, and the answers being
/// collected. One flow at a time, matching the one modal the UI shows.
#[derive(Default)]
pub struct LoginFlow {
    options: Vec<(String, LoginOption)>,
    pending: Option<Pending>,
}

impl LoginFlow {
    /// An empty flow.
    pub fn new() -> Self {
        Self::default()
    }

    /// Remember what the picker is showing so a later `pick` can find the
    /// option's declared fields, and return the modal to open.
    ///
    /// `options` is `(provider, option)` per choice. The host's universal
    /// entry is appended when it is missing, so a picker never comes up
    /// without a way out.
    pub fn offer(&mut self, mut options: Vec<(String, LoginOption)>, fallback: &str) -> LoginNext {
        if options.iter().any(|(_, option)| option.id == CUSTOM_OPTION) {
            // The caller already put one there (an override entry).
        } else {
            options.push((
                fallback.to_string(),
                LoginOption {
                    id: CUSTOM_OPTION.to_string(),
                    name: "Custom endpoint\u{2026}".to_string(),
                    kind: "custom".to_string(),
                    host: String::new(),
                    fields: CUSTOM_FIELDS.iter().map(|f| (*f).to_string()).collect(),
                    extras: BTreeMap::new(),
                },
            ));
        }
        self.pending = None;
        self.options = options;
        let rows: Vec<PickerOption> = self
            .options
            .iter()
            .map(|(provider, option)| {
                let mut row = picker_row(provider, option);
                if row.hint.is_empty() {
                    row.hint = "base URL + key + model".to_string();
                }
                row
            })
            .collect();
        LoginNext::Picker { options: rows }
    }

    /// The user chose `choice`. An option with no fields submits right
    /// away (a local `auth = "none"` endpoint has no key step).
    pub fn pick(&mut self, provider: &str, choice: &str) -> Step {
        let Some((owner, option)) = self
            .options
            .iter()
            .find(|(_, option)| option.id == choice)
            .map(|(owner, option)| (owner.clone(), option.clone()))
        else {
            return Step::Next(LoginNext::Message(format!(
                "no login option named `{choice}`"
            )));
        };
        // A preset's fields win over the row's provider, so one option
        // always belongs to exactly one extension.
        let provider = if owner.is_empty() {
            provider.to_string()
        } else {
            owner
        };
        let fields: Vec<String> = if option.id == CUSTOM_OPTION {
            CUSTOM_FIELDS.iter().map(|f| (*f).to_string()).collect()
        } else {
            option.fields.clone()
        };
        self.pending = Some(Pending {
            provider: provider.clone(),
            choice: option.id.clone(),
            fields,
            answers: BTreeMap::new(),
        });
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.fields.is_empty())
        {
            let done = self.pending.take().expect("pending just set");
            return Step::Submit {
                provider: done.provider,
                choice: done.choice,
                values: done.answers,
            };
        }
        Step::Next(self.next_prompt())
    }

    /// One value arrived from the prompt. Returns the next prompt, or the
    /// completed answers for the caller to submit.
    pub fn push(&mut self, _provider: &str, value: &str) -> Step {
        let Some(pending) = self.pending.as_mut() else {
            return Step::Next(LoginNext::Message("nothing to sign in to".to_string()));
        };
        let Some(field) = pending.fields.first().cloned() else {
            return Step::Next(LoginNext::Message("nothing to sign in to".to_string()));
        };
        pending.fields.remove(0);
        pending.answers.insert(field, value.to_string());
        if pending.fields.is_empty() {
            let done = self.pending.take().expect("pending just used");
            return Step::Submit {
                provider: done.provider,
                choice: done.choice,
                values: done.answers,
            };
        }
        Step::Next(self.next_prompt())
    }

    /// Abandon the in-flight login.
    pub fn cancel(&mut self) {
        self.pending = None;
    }

    /// The prompt for the field the flow is waiting on.
    fn next_prompt(&self) -> LoginNext {
        let Some(pending) = &self.pending else {
            return LoginNext::Message("nothing to sign in to".to_string());
        };
        match pending.fields.first() {
            Some(field) => {
                let (label, masked) = field_prompt(field);
                LoginNext::Secret {
                    provider: pending.provider.clone(),
                    label,
                    masked,
                }
            }
            None => LoginNext::Message("nothing to sign in to".to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn option(id: &str, fields: &[&str]) -> LoginOption {
        LoginOption {
            id: id.to_string(),
            name: id.to_string(),
            kind: "api-key".to_string(),
            host: "example.test".to_string(),
            fields: fields.iter().map(|f| (*f).to_string()).collect(),
            extras: BTreeMap::new(),
        }
    }

    // The picker always offers the host's universal entry, even when the
    // extension supplies nothing (D1: "always Custom endpoint…").
    #[test]
    fn an_empty_option_set_still_offers_the_custom_entry() {
        let mut flow = LoginFlow::new();
        let LoginNext::Picker { options } = flow.offer(vec![], "openai-compatible") else {
            panic!("the picker opened");
        };
        assert_eq!(options.len(), 1);
        assert_eq!(options[0].id, CUSTOM_OPTION);
        assert_eq!(
            options[0].provider, "openai-compatible",
            "it belongs to a provider"
        );
    }

    #[test]
    fn a_preset_collects_its_declared_fields_then_submits() {
        let mut flow = LoginFlow::new();
        let _ = flow.offer(
            vec![(
                "openai-compatible".into(),
                option("openrouter", &["api-key"]),
            )],
            "openai-compatible",
        );
        let Step::Next(next) = flow.pick("openai-compatible", "openrouter") else {
            panic!("a preset with a key asks for it");
        };
        let LoginNext::Secret { masked, label, .. } = next else {
            panic!("a field prompt, got {next:?}");
        };
        assert!(masked, "a key is masked");
        assert!(label.contains("API key"), "{label}");
        let Step::Submit { choice, values, .. } = flow.push("openai-compatible", "sk-x") else {
            panic!("one field submits");
        };
        assert_eq!(choice, "openrouter");
        assert_eq!(values.get("api-key").map(String::as_str), Some("sk-x"));
    }

    #[test]
    fn an_option_with_no_fields_submits_without_a_prompt() {
        let mut flow = LoginFlow::new();
        let _ = flow.offer(
            vec![("openai-compatible".into(), option("ollama", &[]))],
            "openai-compatible",
        );
        // A local `auth = "none"` endpoint has no key step, so the choice
        // itself completes the login.
        let Step::Submit { choice, values, .. } = flow.pick("openai-compatible", "ollama") else {
            panic!("a field-less option submits on selection");
        };
        assert_eq!(choice, "ollama");
        assert!(values.is_empty(), "nothing was asked for");
    }

    #[test]
    fn the_custom_entry_collects_base_url_key_and_model_in_order() {
        let mut flow = LoginFlow::new();
        let _ = flow.offer(vec![], "openai-compatible");
        let Step::Next(next) = flow.pick("openai-compatible", CUSTOM_OPTION) else {
            panic!("the custom entry asks for its fields");
        };
        let LoginNext::Secret { masked, label, .. } = next else {
            panic!("base URL first, got {next:?}");
        };
        assert!(!masked, "a base URL is shown as typed");
        assert!(label.contains("Base URL"), "{label}");

        let Step::Next(next) = flow.push("", "https://x.test/v1") else {
            panic!("more to collect");
        };
        let LoginNext::Secret { masked, .. } = next else {
            panic!("the key next");
        };
        assert!(masked, "the key is masked");

        let Step::Next(next) = flow.push("", "sk-x") else {
            panic!("the model still to collect");
        };
        let LoginNext::Secret { masked, .. } = next else {
            panic!("the model last");
        };
        assert!(!masked, "a model id is shown as typed");

        let Step::Submit { choice, values, .. } = flow.push("", "gpt-4o") else {
            panic!("three fields submit");
        };
        assert_eq!(choice, CUSTOM_OPTION);
        assert_eq!(
            values.get("base-url").map(String::as_str),
            Some("https://x.test/v1")
        );
        assert_eq!(values.get("api-key").map(String::as_str), Some("sk-x"));
        assert_eq!(values.get("model").map(String::as_str), Some("gpt-4o"));
    }

    #[test]
    fn an_unknown_field_id_is_prompted_for_generically() {
        let (label, masked) = field_prompt("webhook-secret");
        assert!(label.contains("webhook-secret"), "{label}");
        assert!(!masked, "an unknown id is not assumed to be a secret");
    }

    #[test]
    fn an_unknown_choice_is_reported_not_panicked() {
        let mut flow = LoginFlow::new();
        let _ = flow.offer(vec![], "openai-compatible");
        assert!(matches!(
            flow.pick("openai-compatible", "nope"),
            Step::Next(LoginNext::Message(_))
        ));
    }
}

/// The user's own presets, `<config>/provider-presets.toml` (D1's override
/// layer): named custom endpoints, merged with the extension's own list.
///
/// These are user data, not vendor data in the core - the file lives in the
/// user's config directory and is attributed to the openai-compatible
/// provider that will speak its shape. The host reads only the neutral
/// fields ADR-0033 names and never interprets the base URL beyond taking
/// the host to show in the `net` consent.
///
/// A malformed file yields no entries rather than an error: a typo in an
/// optional convenience file must not stop the user signing in.
pub fn override_presets(text: &str, provider: &str) -> Vec<(String, LoginOption)> {
    #[derive(serde::Deserialize)]
    struct File {
        #[serde(default)]
        preset: Vec<Entry>,
    }
    #[derive(serde::Deserialize)]
    struct Entry {
        id: String,
        #[serde(default)]
        name: String,
        #[serde(default)]
        base_url: String,
        #[serde(default)]
        auth: String,
        #[serde(default)]
        models: Vec<String>,
    }

    let Ok(file) = toml::from_str::<File>(text) else {
        return Vec::new();
    };
    file.preset
        .into_iter()
        .filter(|entry| !entry.id.is_empty() && entry.id != CUSTOM_OPTION)
        .map(|entry| {
            let name = if entry.name.is_empty() {
                entry.id.clone()
            } else {
                entry.name.clone()
            };
            let host = crate::ad_hoc_host_from_authority(
                entry
                    .base_url
                    .split("://")
                    .nth(1)
                    .unwrap_or(&entry.base_url),
            )
            .unwrap_or_default();
            // `auth = "none"` has no key step; anything else asks for one.
            let fields = if entry.auth == "none" {
                Vec::new()
            } else {
                vec!["api-key".to_string()]
            };
            let mut extras = BTreeMap::new();
            if !entry.base_url.is_empty() {
                extras.insert("base_url".to_string(), entry.base_url.clone());
            }
            if !entry.models.is_empty() {
                extras.insert("models".to_string(), entry.models.join(","));
            }
            (
                provider.to_string(),
                LoginOption {
                    id: entry.id.clone(),
                    name,
                    kind: if entry.auth == "none" {
                        "custom".to_string()
                    } else {
                        "api-key".to_string()
                    },
                    host,
                    fields,
                    extras,
                },
            )
        })
        .collect()
}

#[cfg(test)]
mod override_tests {
    use super::*;

    // D1's override layer: named custom endpoints, merged with the
    // extension's own presets.
    #[test]
    fn a_user_preset_becomes_a_named_custom_endpoint() {
        let entries = override_presets(
            "[[preset]]\nid = \"my-proxy\"\nname = \"My Proxy\"\n\
             base_url = \"https://llm.example.com/v1\"\nauth = \"bearer\"\n\
             models = [\"a\", \"b\"]\n",
            "openai-compatible",
        );
        assert_eq!(entries.len(), 1);
        let (provider, option) = &entries[0];
        assert_eq!(provider, "openai-compatible");
        assert_eq!(option.id, "my-proxy");
        assert_eq!(option.name, "My Proxy");
        assert_eq!(option.host, "llm.example.com");
        assert_eq!(option.fields, vec!["api-key".to_string()]);
        assert_eq!(
            option.extras.get("base_url").map(String::as_str),
            Some("https://llm.example.com/v1")
        );
    }

    #[test]
    fn a_local_user_preset_has_no_key_step() {
        let entries = override_presets(
            "[[preset]]\nid = \"box\"\nbase_url = \"http://localhost:1234/v1\"\nauth = \"none\"\n",
            "openai-compatible",
        );
        assert_eq!(entries[0].1.fields, Vec::<String>::new());
    }

    #[test]
    fn a_malformed_override_file_yields_nothing_rather_than_an_error() {
        assert!(override_presets("not toml [[[", "openai-compatible").is_empty());
        assert!(override_presets("", "openai-compatible").is_empty());
        assert!(
            override_presets("[[preset]]\nname = \"no id\"\n", "openai-compatible").is_empty(),
            "an entry with no id is dropped"
        );
    }

    #[test]
    fn the_custom_entry_id_is_reserved() {
        assert!(
            override_presets(
                &format!("[[preset]]\nid = \"{CUSTOM_OPTION}\"\n"),
                "openai-compatible"
            )
            .is_empty(),
            "a user preset cannot shadow the host's universal entry"
        );
    }
}
