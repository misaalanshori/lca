//! Manifest-declared provider needs (gh #157): the host stops knowing
//! providers. Default endpoint hosts, the credential namespace, and the
//! environment base-URL override all come from the provider's own
//! `extension.toml` - read off its loaded handle when present, else off
//! its installed package - never from host literals.
//!
//! The credential namespace is the manifest name itself (FR-PERM-6), so
//! only the default hosts and the env override need carrying.

use std::path::Path;

use lca_core::ExtensionRegistry;

/// What one provider declared in its manifest: everything the host's
/// ad hoc `net` grant path needs to tell a default endpoint (covered,
/// no grant) from a configured one (consent, FR-PERM-16).
pub(crate) struct ProviderNeeds {
    /// Manifest `[capabilities.net]` hosts.
    pub defaults: Vec<lca_permissions::NetPattern>,
    /// Manifest `[login] env_base_url`, when the provider declares one.
    pub env_base_url: Option<String>,
}

/// Read a provider's needs off its loaded handle, else off its
/// installed package manifest. `None` means no declaration found: the
/// host then treats every configured host as non-default (consent is
/// the safe direction).
pub(crate) fn provider_needs(
    registry: &ExtensionRegistry,
    data: &Path,
    provider: &str,
) -> Option<ProviderNeeds> {
    let text = registry
        .provider(provider)
        .and_then(|handle| handle.manifest_text())
        .or_else(|| {
            std::fs::read_to_string(
                data.join("extensions")
                    .join(provider)
                    .join("extension.toml"),
            )
            .ok()
        })?;
    let manifest = lca_ext_host::Manifest::parse(&text).ok()?;
    Some(ProviderNeeds {
        defaults: manifest.net,
        env_base_url: manifest.login_env_base_url,
    })
}

/// Resolve one provider's needs with whatever the caller has (gh
/// #157): a loaded handle's manifest wins, the installed package
/// manifest is the fallback, and `None` means no declaration found.
pub(crate) fn resolve_needs(
    registry: Option<&ExtensionRegistry>,
    data: &Path,
    provider: &str,
) -> Option<ProviderNeeds> {
    if let Some(needs) = registry.and_then(|registry| provider_needs(registry, data, provider)) {
        return Some(needs);
    }
    installed_needs(data, provider)
}

/// The same lookup without a registry (startup paths): the installed
/// package manifest only. Loaded handles are strictly richer, so
/// callers with a registry pass it to [`resolve_needs`].
pub(crate) fn installed_needs(data: &Path, provider: &str) -> Option<ProviderNeeds> {
    let text = std::fs::read_to_string(
        data.join("extensions")
            .join(provider)
            .join("extension.toml"),
    )
    .ok()?;
    let manifest = lca_ext_host::Manifest::parse(&text).ok()?;
    Some(ProviderNeeds {
        defaults: manifest.net,
        env_base_url: manifest.login_env_base_url,
    })
}

/// Whether `host` is one of the provider's declared default endpoints.
/// Unknown needs (`None`) never count as default.
pub(crate) fn is_default_host(needs: Option<&ProviderNeeds>, host: &str) -> bool {
    needs.is_some_and(|needs| {
        needs
            .defaults
            .iter()
            .any(|pattern| pattern.matches_host(host))
    })
}

/// The endpoint host a login would need an ad hoc `net` grant for:
/// the environment override's host when one is declared and set,
/// else the stored base URL's host, whenever either is not a declared
/// default (FR-PERM-16, ADR-0022). `None` when the default endpoint is
/// in use or nothing is configured.
pub(crate) fn provider_ad_hoc_host(
    data: &Path,
    provider: &str,
    needs: Option<&ProviderNeeds>,
) -> Option<String> {
    let base = needs
        .as_ref()
        .and_then(|needs| needs.env_base_url.as_deref())
        .and_then(|var| std::env::var(var).ok())
        .filter(|value| !value.is_empty())
        .or_else(|| stored_base_url(data, provider))?;
    let rest = base.split("://").nth(1).unwrap_or(&base);
    ad_hoc_host_from_authority(rest, needs)
}

/// The stored base URL: the bare default-profile value a direct setup
/// wrote (what `/login` wrote before profiles, and what a
/// `default_profile` preset writes now).
fn stored_base_url(data: &Path, provider: &str) -> Option<String> {
    let path = data.join("credentials").join(format!("{provider}.json"));
    let text = std::fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    value
        .get("base_url")
        .and_then(|url| url.as_str())
        .map(str::to_string)
}

/// Manifest-driven needs for tests (gh #157): the openai-compatible
/// declaration, as TOML text, so the old literal cases run as manifest
/// rows with no extension-crate dependency in either feature build.
#[cfg(test)]
pub(crate) fn test_needs() -> ProviderNeeds {
    let manifest = lca_ext_host::Manifest::parse(
        "name = \"acme\"\nversion = \"1.0.0\"\nabi = \"0.5\"\n\
         worlds = [\"provider\"]\n[capabilities.net]\nhosts = [\"api.acme.test\"]\n\
         [capabilities.credentials]\nnamespace = \"acme\"\n\
         [login]\nenv_base_url = \"ACME_BASE_URL\"\n",
    )
    .expect("the test manifest parses");
    ProviderNeeds {
        defaults: manifest.net,
        env_base_url: manifest.login_env_base_url,
    }
}

/// The host in a URL authority, or `None` when it is a declared
/// default endpoint.
pub(crate) fn ad_hoc_host_from_authority(
    rest: &str,
    needs: Option<&ProviderNeeds>,
) -> Option<String> {
    let authority = rest.split('/').next().unwrap_or("");
    let host = authority
        .rsplit('@')
        .next()
        .unwrap_or(authority)
        .split(':')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    (!host.is_empty() && !is_default_host(needs, &host)).then_some(host)
}
