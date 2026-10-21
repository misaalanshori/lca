//! Provider profiles (gh #31): one credentials namespace, several
//! services, and model rows that name the service that will bill.
//!
//! Everything here is **extension-internal**: the host still never parses
//! a profile, a preset, or a model list (ADR-0031), and the namespace is
//! still this extension's own.
//!
//! The shape:
//!
//! - Credentials are per profile - `profile.<id>.api_key`,
//!   `profile.<id>.base_url`. The legacy bare `api_key`/`base_url` keep
//!   reading as the unnamed **default** profile, so an install written
//!   before profiles (or configured by the environment alone) behaves
//!   exactly as it did (backward tolerance, gh #34's discipline).
//! - The persisted `models` setting grows a profile id per entry -
//!   `id[@profile][=window]` - which is backward tolerant both ways: an
//!   entry without `@` is a default-profile model, and `=window` still
//!   parses as it always did.
//! - **Routing follows the model**: a request for a model of profile `P`
//!   uses `P`'s base URL and key. The label promises which service and
//!   key will be billed, so the request keeps that promise.
//! - The environment keeps its documented precedence (`docs/configuration.md`:
//!   environment over what login persisted) and acts on the **default**
//!   profile only. A model owned by a named profile is that profile's,
//!   whatever the environment says.

use lca_protocol::ProviderCap;

use crate::Settings;

/// A model's owning profile. `None` is the default profile: the bare
/// credentials plus the environment.
pub type Profile = Option<String>;

/// The credential key `field` lives under for `profile`.
pub fn credential_key(profile: &Profile, field: &str) -> String {
    match profile {
        Some(id) => scoped_key(id, field),
        None => field.to_string(),
    }
}

/// Every per-profile field a login writes (gh #293). ONE list, so the
/// login path and `run_logout` can never drift apart again — that drift
/// (login writing `profile.<id>.api_key` while logout deleted only the
/// bare pair) was the root cause of the logout leak.
pub const PROFILE_CREDENTIAL_FIELDS: &[&str] = &[API_KEY_FIELD, BASE_URL_FIELD];

/// The API-key field name (`profile.<id>.api_key`).
pub const API_KEY_FIELD: &str = "api_key";

/// The base-URL field name (`profile.<id>.base_url`).
pub const BASE_URL_FIELD: &str = "base_url";

/// The key one *named* profile's field lives under.
fn scoped_key(id: &str, field: &str) -> String {
    format!("profile.{id}.{field}")
}

/// One entry of the `models` setting: `id[@profile][=window]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelEntry {
    /// The raw model id, never decorated.
    pub id: String,
    /// The profile that owns it (`None`: the default profile).
    pub profile: Profile,
    /// The endpoint's reported window, when it sent one (gh #34).
    pub window: Option<u32>,
}

/// Parse the `models` setting into entries, profile tag and all.
///
/// Splitting `=` comes first, exactly as it always did, so `id=window`
/// keeps parsing; the profile is the `@` in what remains. An id that
/// carries neither reads as a default-profile model.
pub fn model_entries(value: &str) -> Vec<ModelEntry> {
    value
        .split(',')
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            let mut rest = entry;
            let mut window = None;
            if let Some((model, tokens)) = entry.rsplit_once('=')
                && !model.is_empty()
                && let Ok(parsed) = tokens.parse::<u32>()
            {
                rest = model;
                window = Some(parsed);
            }
            let (id, profile) = match rest.rsplit_once('@') {
                Some((id, profile)) if !id.is_empty() && !profile.is_empty() => {
                    (id.to_string(), Some(profile.to_string()))
                }
                _ => (rest.to_string(), None),
            };
            ModelEntry {
                id,
                profile,
                window,
            }
        })
        .collect()
}

/// Serialize entries back into the setting's single line.
pub fn serialize_entries(entries: &[ModelEntry]) -> String {
    entries
        .iter()
        .map(|entry| {
            let mut text = entry.id.clone();
            if let Some(profile) = &entry.profile {
                text.push('@');
                text.push_str(profile);
            }
            if let Some(window) = entry.window {
                text.push('=');
                text.push_str(&window.to_string());
            }
            text
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// Which profile `model` belongs to; `None` (the default) both when the
/// id is not in the list and when its entry carries no profile tag.
pub fn profile_of_model(entries: &[ModelEntry], model: &str) -> Profile {
    entries
        .iter()
        .find(|entry| entry.id == model)
        .and_then(|entry| entry.profile.clone())
}

/// The environment, as the documented precedence states it: set and
/// non-empty wins; unset falls through to what login persisted, then to
/// the compiled-in default (`Settings::default` reads the same variable,
/// so the last step is that value).
fn env_var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

/// The effective base URL for one profile: the environment for the
/// default profile (documented precedence - environment over what login
/// persisted - acting on the default alone), the profile's own for a
/// named one, falling back to the bare value when a profile stored
/// nothing (`auth: "none"` presets store no key of their own).
pub fn base_url_for<C: ProviderCap + ?Sized>(
    cap: &C,
    settings: &Settings,
    profile: &Profile,
) -> String {
    if let Some(profile) = profile {
        if let Some(url) = cap
            .credentials_get(&scoped_key(profile, "base_url"))
            .filter(|url| !url.is_empty())
        {
            return url;
        }
        // Nothing of its own: the legacy bare value still reaches an
        // endpoint, and the environment stays out of a named profile.
        return cap
            .credentials_get("base_url")
            .filter(|url| !url.is_empty())
            .unwrap_or_else(|| settings.base_url.clone());
    }
    env_var("OPENAI_BASE_URL")
        .or_else(|| {
            cap.credentials_get("base_url")
                .filter(|url| !url.is_empty())
        })
        .unwrap_or_else(|| settings.base_url.clone())
}

/// The effective bearer token for one profile, in the documented order.
///
/// **Default profile:** `OPENAI_API_KEY`, then `OPENCODE_API_KEY`, then
/// the active preset's own declared `env` names in declaration order
/// (gh #292), then what login persisted.
///
/// **Named profile:** its preset's names, then what login persisted. The
/// two universal names are *this provider's default-profile* names and are
/// deliberately not consulted for a named profile. `OPENAI_API_KEY`
/// authenticates every profile that reached it, so a user who exported it
/// because they use OpenAI directly would have that key sent to Groq and
/// DeepSeek too the moment they logged into them - the same
/// wrong-secret-to-the-wrong-endpoint leak the cross-provider rule below
/// exists to prevent, arriving through the front door. gh #31's promise
/// ("a named profile is its own, environment notwithstanding") is exactly
/// that, and it still holds; what gh #292 adds is that a named profile now
/// also reads the variable *its own service* documents.
///
/// gh #292: `provider-presets.toml` documents a variable per service
/// (`DEEPSEEK_API_KEY`, `GROQ_API_KEY`, …) and `Preset` parsed the list,
/// but nothing ever read it - so exporting the variable the docs named
/// left a developer told to authenticate. The preset's names are the
/// promise that its documented setup just works.
///
/// Only the **active** preset's names are consulted, and a named profile
/// whose id is not in the table reads no environment at all: falling
/// through to any other variable would let a stray `GROQ_API_KEY`
/// authenticate a DeepSeek profile and send the wrong secret to the
/// wrong endpoint.
pub fn api_key_for<C: ProviderCap + ?Sized>(
    cap: &C,
    settings: &Settings,
    profile: &Profile,
) -> Option<String> {
    // The universal names are the default profile's, never a named one's.
    if profile.is_none()
        && let Some(key) = env_var("OPENAI_API_KEY").or_else(|| env_var("OPENCODE_API_KEY"))
    {
        return Some(key);
    }
    preset_env_key(cap, profile).or_else(|| {
        // What login persisted. A named profile reads its own
        // `profile.<id>.api_key` first; the **bare** `api_key` / the
        // environment-derived `settings.api_key` is the *default* profile's
        // slot, so a named profile reaches it only when it has nothing of
        // its own (`auth: "none"` presets store no key, and the bare slot
        // is then how such a setup gets to its endpoint).
        //
        // Reading the bare slot first for a named profile - what gh #31
        // did - stayed invisible while a named profile could see no
        // environment at all. gh #292 makes it matter: `settings.api_key`
        // is read from the universal variables, so an `OPENAI_API_KEY`
        // exported for direct OpenAI use would otherwise travel to
        // DeepSeek the moment a `deepseek` profile exists.
        let stored = cap
            .credentials_get(&credential_key(profile, "api_key"))
            .filter(|key| !key.is_empty());
        match stored {
            Some(key) => Some(key),
            None if profile.is_none() => cap
                .credentials_get("api_key")
                .filter(|key| !key.is_empty())
                .or_else(|| settings.api_key.clone()),
            // A named profile's last resort is a *stored* bare key, never
            // the environment-derived `settings.api_key` that sits beside
            // it.
            None => cap.credentials_get("api_key").filter(|key| !key.is_empty()),
        }
    })
}

/// The first set variable among the active preset's declared `env` names
/// (gh #292), in declaration order so a preset that lists two (`opencode-go`
/// declares `OPENCODE_API_KEY` before `OPENAI_API_KEY`) resolves
/// deterministically.
///
/// A profile with no discoverable preset declares no names, so this reads
/// nothing and the caller falls through to the stored credential. An
/// `auth = "none"` preset (Ollama, LM Studio) declares an empty list and
/// is likewise unchanged.
fn preset_env_key<C: ProviderCap + ?Sized>(cap: &C, profile: &Profile) -> Option<String> {
    // A named profile's id IS its preset id (`login_submit` picks the
    // preset by `answer.choice` and names the profile after it). The
    // default profile carries no id of its own, so the endpoint's
    // remembered preset is what names the service the user chose.
    let id = match profile {
        Some(id) => id.clone(),
        None => cap
            .credentials_get("preset")
            .filter(|name| !name.is_empty())?,
    };
    // The same resource `load_presets` reads, taken through this generic
    // `cap` rather than that function's `&dyn ProviderCap` sink. Both
    // parse the one file with `parse_presets`, so the shapes cannot
    // diverge - only the access path differs.
    let bytes = cap.resource_read(PRESETS_RESOURCE).ok()?;
    let presets = crate::parse_presets(&String::from_utf8_lossy(&bytes));
    let preset = presets.iter().find(|preset| preset.id == id)?;
    preset.env.iter().find_map(|name| env_var(name))
}

/// The extension's preset table, by the name its own `resources/` bag uses.
const PRESETS_RESOURCE: &str = "provider-presets.toml";

/// The label a row shows for one profile: the profile's own name, and
/// for the default profile whatever that endpoint actually is - a stored
/// preset, else the host, else `default`. Never the crate name (gh #31).
pub fn label_for<C: ProviderCap + ?Sized>(
    cap: &C,
    settings: &Settings,
    profile: &Profile,
) -> String {
    if let Some(profile) = profile {
        return profile.clone();
    }
    if let Some(preset) = cap
        .credentials_get("preset")
        .filter(|name| !name.is_empty())
    {
        return preset;
    }
    let base = base_url_for(cap, settings, profile);
    let host = crate::host_of(&base);
    if host.is_empty() {
        "default".to_string()
    } else {
        host
    }
}

/// One row of the picker: its raw id, the profile that owns it, the
/// label the row shows, and the window the setting reported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickerModel {
    /// The raw id - selection, calls, and metadata use this one.
    pub id: String,
    /// The owning profile (`None`: the default).
    pub profile: Profile,
    /// What the row shows after the id (gh #31).
    pub label: String,
    /// The window from the setting entry, when it carried one.
    pub window: Option<u32>,
}

/// The picker's list: the configured model first, then every entry of the
/// `models` setting, each carrying its profile id and label. Both
/// delivery modes build their list here so they cannot drift (NFR-25).
pub fn picker_models<C: ProviderCap + ?Sized>(
    cap: &C,
    settings: &Settings,
    stored: &str,
    configured: &str,
) -> Vec<PickerModel> {
    let entries = model_entries(stored);
    let mut out: Vec<PickerModel> = Vec::new();
    let build = |id: String, window: Option<u32>| {
        let profile = profile_of_model(&entries, &id);
        let label = label_for(cap, settings, &profile);
        PickerModel {
            id,
            profile,
            label,
            window,
        }
    };
    if !configured.is_empty() {
        let window = entries
            .iter()
            .find(|entry| entry.id == configured)
            .and_then(|entry| entry.window);
        out.push(build(configured.to_string(), window));
    }
    for entry in &entries {
        if out.iter().any(|model| model.id == entry.id) {
            continue;
        }
        out.push(build(entry.id.clone(), entry.window));
    }
    out
}

/// The extras the host's picker reads back: the profile id (absent for
/// the default, so routing stays explicit) and the label every row shows.
pub fn row_extras(model: &PickerModel) -> Vec<(String, String)> {
    let mut extras = vec![("label".to_string(), model.label.clone())];
    if let Some(profile) = &model.profile {
        extras.push(("profile".to_string(), profile.clone()));
    }
    extras
}
