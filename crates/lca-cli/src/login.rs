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
use lca_ui::{LoginNext, PickerOption};

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

/// Split a device-code URL (gh #184): an `oauth_open` URL carrying a
/// `#code=...` fragment names the user code the device page shows.
/// The fragment never reaches the server, so navigation is unaffected;
/// the host shows the code beside the page link (the only freeze-safe
/// channel — no new host import). Returns the page URL without the
/// fragment, plus the code when present.
pub fn split_device_code(url: &str) -> (String, Option<String>) {
    let Some((page, fragment)) = url.split_once('#') else {
        return (url.to_string(), None);
    };
    let Some(code) = fragment.strip_prefix("code=") else {
        return (url.to_string(), None);
    };
    if code.is_empty() {
        return (url.to_string(), None);
    }
    (page.to_string(), Some(code.to_string()))
}

/// Parse an OAuth redirect callback (R4(c)'s manual fallback): the query
/// of a pasted callback URL, or a bare query string, into the `(name,
/// value)` pairs `oauth_await` would have delivered from the loopback
/// listener. Returns `None` when there is no query at all.
/// A `code#state` paste (gh #183: the shape Anthropic's copy-code page
/// shows, pi's `parseAuthorizationInput`) splits into its two pairs.
pub fn parse_callback(value: &str) -> Option<Vec<(String, String)>> {
    let trimmed = value.trim();
    if !trimmed.contains('?')
        && !trimmed.contains('=')
        && let Some((code, state)) = trimmed.split_once('#')
        && !code.is_empty()
        && !state.is_empty()
    {
        return Some(vec![
            ("code".to_string(), code.to_string()),
            ("state".to_string(), state.to_string()),
        ]);
    }
    let (query, had_question) = match trimmed.split_once('?') {
        Some((_, query)) => (query, true),
        None => (trimmed, false),
    };
    // A redirect with no query carries no code: refuse it rather than feed
    // a path (`/callback`) to the flow as a parameter.
    if !had_question && !query.contains('=') {
        return None;
    }
    let query = query.split('#').next().unwrap_or(query);
    let mut pairs = Vec::new();
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        pairs.push((percent_decode(name), percent_decode(value)));
    }
    (!pairs.is_empty()).then_some(pairs)
}

/// Decode `%XX` escapes; bytes that are not valid UTF-8 after decoding are
/// replaced lossily (the flow only cares about the ASCII `code`/`state`).
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && bytes.get(i + 1).is_some_and(u8::is_ascii_hexdigit)
            && bytes.get(i + 2).is_some_and(u8::is_ascii_hexdigit)
        {
            let hi = (bytes[i + 1] as char).to_digit(16).unwrap_or(0);
            let lo = (bytes[i + 2] as char).to_digit(16).unwrap_or(0);
            out.push((hi * 16 + lo) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
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
        /// The chosen option id.
        choice: String,
        /// The chosen option's `kind` (gh #188): a `custom` option
        /// keeps the preset-less treatment downstream.
        kind: String,
        /// Field id -> value, in the order collected.
        values: BTreeMap<String, String>,
    },
}

struct Pending {
    provider: String,
    choice: String,
    kind: String,
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
    /// `options` is `(provider, option)` per choice. The host renders
    /// whatever the extensions declare and synthesizes nothing (gh
    /// #188): an empty set answers with the way out instead of a
    /// phantom entry.
    pub fn offer(&mut self, options: Vec<(String, LoginOption)>) -> LoginNext {
        self.pending = None;
        self.options = options;
        if self.options.is_empty() {
            return LoginNext::Message(
                "no login options are available: no provider extensions are installed or enabled. \
                 Install one (`lca ext install <provider>`) or re-enable one \
                 (`lca ext enable <provider>`)."
                    .to_string(),
            );
        }
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
        // The option's declared fields, whatever they are (gh #188):
        // the host prompts for them sequentially and never substitutes
        // its own list.
        self.pending = Some(Pending {
            provider: provider.clone(),
            choice: option.id.clone(),
            kind: option.kind.clone(),
            fields: option.fields.clone(),
            answers: BTreeMap::new(),
        });
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.fields.is_empty())
            && let Some(done) = self.pending.take()
        {
            // Every field was pre-filled: the flow is already done.
            return Step::Submit {
                provider: done.provider,
                choice: done.choice,
                kind: done.kind,
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
            let Some(done) = self.pending.take() else {
                return Step::Next(LoginNext::Message("nothing to sign in to".to_string()));
            };
            return Step::Submit {
                provider: done.provider,
                choice: done.choice,
                kind: done.kind,
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

    // Gh #188: the host synthesizes nothing. An empty option set
    // answers with the way out instead of a phantom custom entry.
    #[test]
    fn an_empty_option_set_names_the_way_out() {
        let mut flow = LoginFlow::new();
        let LoginNext::Message(text) = flow.offer(vec![]) else {
            panic!("no picker without options");
        };
        assert!(text.contains("lca ext install"), "{text}");
        assert!(text.contains("lca ext enable"), "{text}");
    }

    // Gh #188: an extension-declared custom option flows like any
    // preset - the host prompts for its declared fields in order and
    // hands the kind through for the preset-less treatment downstream.
    #[test]
    fn an_extension_declared_custom_option_collects_its_fields_in_order() {
        let mut custom = option("custom", &["base-url", "api-key", "model"]);
        custom.kind = "custom".to_string();
        custom.host = String::new();
        let mut flow = LoginFlow::new();
        let _ = flow.offer(vec![("openai-compatible".into(), custom)]);
        let Step::Next(next) = flow.pick("openai-compatible", "custom") else {
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

        let Step::Submit {
            choice,
            kind,
            values,
            ..
        } = flow.push("", "gpt-4o")
        else {
            panic!("three fields submit");
        };
        assert_eq!(choice, "custom");
        assert_eq!(kind, "custom", "the kind rides along");
        assert_eq!(
            values.get("base-url").map(String::as_str),
            Some("https://x.test/v1")
        );
        assert_eq!(values.get("api-key").map(String::as_str), Some("sk-x"));
        assert_eq!(values.get("model").map(String::as_str), Some("gpt-4o"));
    }

    #[test]
    fn a_preset_collects_its_declared_fields_then_submits() {
        let mut flow = LoginFlow::new();
        let _ = flow.offer(vec![(
            "openai-compatible".into(),
            option("openrouter", &["api-key"]),
        )]);
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
        let _ = flow.offer(vec![("openai-compatible".into(), option("ollama", &[]))]);
        // A local `auth = "none"` endpoint has no key step, so the choice
        // itself completes the login.
        let Step::Submit { choice, values, .. } = flow.pick("openai-compatible", "ollama") else {
            panic!("a field-less option submits on selection");
        };
        assert_eq!(choice, "ollama");
        assert!(values.is_empty(), "nothing was asked for");
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
        let _ = flow.offer(vec![(
            "openai-compatible".into(),
            option("openrouter", &["api-key"]),
        )]);
        assert!(matches!(
            flow.pick("openai-compatible", "nope"),
            Step::Next(LoginNext::Message(_))
        ));
    }
}

/// The user's own presets, `~/.lca/provider-presets.toml` (D1's override
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
pub(crate) fn override_presets(
    text: &str,
    provider: &str,
    needs: Option<&crate::provider_needs::ProviderNeeds>,
) -> Vec<(String, LoginOption)> {
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
        .filter(|entry| !entry.id.is_empty())
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
                needs,
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
mod callback_tests {
    use super::parse_callback;

    // Verifies: R4(c) - a pasted redirect URL yields exactly the query the
    // loopback listener would have delivered.
    #[test]
    fn a_redirect_url_yields_its_query_pairs() {
        let pairs =
            parse_callback("http://127.0.0.1:54123/callback?code=4%2F0Ac&state=xyz&scope=a")
                .expect("a callback URL parses");
        assert_eq!(
            pairs,
            vec![
                ("code".to_string(), "4/0Ac".to_string()),
                ("state".to_string(), "xyz".to_string()),
                ("scope".to_string(), "a".to_string()),
            ]
        );
    }

    // Verifies: a bare query string (what the browser's address bar shows
    // after a failed navigation) parses the same way.
    #[test]
    fn a_bare_query_parses_too() {
        let pairs = parse_callback("code=abc&state=s").expect("bare query");
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0].1, "abc");
    }

    // Verifies: gh #183 - a `code#state` paste (Anthropic's
    // copy-code page, pi's `parseAuthorizationInput`) delivers as
    // pairs, while a bare code with no state stays out.
    #[test]
    fn a_copy_code_paste_splits_into_code_and_state() {
        assert_eq!(
            parse_callback("spl-abc123#verifier-state"),
            Some(vec![
                ("code".to_string(), "spl-abc123".to_string()),
                ("state".to_string(), "verifier-state".to_string()),
            ])
        );
        assert_eq!(parse_callback("just-a-code"), None);
    }

    // Verifies: a redirect with no query carries no code, so it is refused
    // rather than fed to the flow as a path parameter.
    #[test]
    fn a_redirect_without_a_query_is_refused() {
        assert!(parse_callback("http://127.0.0.1:1/callback").is_none());
        assert!(parse_callback("http://example.com/").is_none());
        assert!(parse_callback("").is_none());
    }

    // Verifies: a fragment is dropped and non-UTF-8 escapes survive as
    // replacement characters rather than a panic.
    #[test]
    fn fragments_and_bad_escapes_do_not_panic() {
        let pairs = parse_callback("http://x/y?code=1#frag").expect("query");
        assert_eq!(pairs, vec![("code".into(), "1".into())]);
        let pairs = parse_callback("code=%FF%ZZ").expect("query");
        assert_eq!(pairs[0].0, "code");
    }
}

#[cfg(test)]
mod override_tests {
    use super::*;

    /// Manifest-driven needs for the override tests (gh #157): the
    /// openai-compatible declaration, as TOML text, so the old literal
    /// cases run as manifest rows.
    fn test_needs() -> crate::provider_needs::ProviderNeeds {
        let manifest = lca_ext_host::Manifest::parse(
            "name = \"acme\"\nversion = \"1.0.0\"\nabi = \"0.5\"\n\
             worlds = [\"provider\"]\n[capabilities.net]\nhosts = [\"api.acme.test\"]\n\
             [capabilities.credentials]\nnamespace = \"acme\"\n\
             [login]\nenv_base_url = \"ACME_BASE_URL\"\n",
        )
        .expect("the test manifest parses");
        crate::provider_needs::ProviderNeeds {
            defaults: manifest.net,
            env_base_url: manifest.login_env_base_url,
        }
    }

    // D1's override layer: named custom endpoints, merged with the
    // extension's own presets.
    #[test]
    fn a_user_preset_becomes_a_named_custom_endpoint() {
        let entries = override_presets(
            "[[preset]]\nid = \"my-proxy\"\nname = \"My Proxy\"\n\
             base_url = \"https://llm.example.com/v1\"\nauth = \"bearer\"\n\
             models = [\"a\", \"b\"]\n",
            "openai-compatible",
            Some(&test_needs()),
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
            Some(&test_needs()),
        );
        assert_eq!(entries[0].1.fields, Vec::<String>::new());
    }

    #[test]
    fn a_malformed_override_file_yields_nothing_rather_than_an_error() {
        assert!(override_presets("not toml [[[", "openai-compatible", None).is_empty());
        assert!(override_presets("", "openai-compatible", None).is_empty());
        assert!(
            override_presets("[[preset]]\nname = \"no id\"\n", "openai-compatible", None)
                .is_empty(),
            "an entry with no id is dropped"
        );
    }
}

#[cfg(test)]
mod device_code_tests {
    use super::split_device_code;

    // Verifies: gh #184 - a `#code=` fragment splits into the page and
    // the user code; anything else passes through untouched.
    #[test]
    fn a_device_code_url_splits_into_page_and_code() {
        assert_eq!(
            split_device_code("https://github.com/login/device#code=ABCD-1234"),
            (
                "https://github.com/login/device".to_string(),
                Some("ABCD-1234".to_string())
            )
        );
        assert_eq!(
            split_device_code("https://example.test/auth"),
            ("https://example.test/auth".to_string(), None)
        );
        assert_eq!(
            split_device_code("https://example.test/cb#other=1"),
            ("https://example.test/cb#other=1".to_string(), None)
        );
        assert_eq!(
            split_device_code("https://example.test/cb#code="),
            ("https://example.test/cb#code=".to_string(), None)
        );
    }
}
