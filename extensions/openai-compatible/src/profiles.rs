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

/// The effective bearer token for one profile: the same precedence, with
/// the default profile's environment check taking both of this provider's
/// key names (`OPENAI_API_KEY`, `OPENCODE_API_KEY`).
pub fn api_key_for<C: ProviderCap + ?Sized>(
    cap: &C,
    settings: &Settings,
    profile: &Profile,
) -> Option<String> {
    if let Some(profile) = profile {
        if let Some(key) = cap
            .credentials_get(&scoped_key(profile, "api_key"))
            .filter(|key| !key.is_empty())
        {
            return Some(key);
        }
        return cap
            .credentials_get("api_key")
            .filter(|key| !key.is_empty())
            .or_else(|| settings.api_key.clone());
    }
    env_var("OPENAI_API_KEY")
        .or_else(|| env_var("OPENCODE_API_KEY"))
        .or_else(|| {
            cap.credentials_get("api_key")
                .filter(|key| !key.is_empty())
                .or_else(|| settings.api_key.clone())
        })
}

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
