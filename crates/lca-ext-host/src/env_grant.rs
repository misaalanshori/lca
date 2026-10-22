//! The `env` capability (gh #170): declared exact names, forwarded at
//! instantiation.
//!
//! A WASM component is instantiated with an empty `WasiCtx`, so
//! `std::env::var` inside it always answers `NotPresent`, while a native-linked
//! extension reads the host environment directly. The same provider therefore
//! worked compiled-in and broke as a `.wasm`, which is why `lca-cli` grew the
//! hardcoded `openai_ad_hoc_host()` workaround (gh #157).
//!
//! **The model.** The manifest lists variable *names*; the host copies the
//! values that are present into the context when it builds the store. That is
//! enforced by construction rather than by a check a guest could reach around,
//! and it needs no new WIT import - `wasi:cli/environment` already exists, and
//! already answers `NotPresent` for anything the context does not hold.
//!
//! **Why exact names.** The names are the whole consent gate: "Read `LCA_PROBE`
//! and `BAR_KEY` from your environment" is a reviewable sentence, while "read
//! any variable" says nothing. Wildcards are refused at parse for the same
//! reason `net`'s bare `*` is.
//!
//! **Absence is not emptiness.** A declared variable that is unset on the host
//! is *absent* from the context, never `Some("")` - an empty API key that looks
//! present is how a provider sends a blank credential instead of failing
//! honestly.
//!
//! **Never the ambient environment.** Not even `PATH`. A forwarded `PATH` is an
//! information leak and a mode difference between native and WASM that nobody
//! would expect; the grant is a list of names, not a view of the host.

use std::collections::BTreeMap;

/// The variables a manifest declares, resolved against the host environment
/// (gh #170).
///
/// Returns only the declared names that are **present** on the host. A name
/// that is declared but unset is omitted (absent, not empty), and a name the
/// host has but the manifest never declared is never included - that
/// asymmetry is the capability.
///
/// `Err` only when the manifest does not parse; a manifest without the
/// capability resolves to an empty map, which is exactly the pre-#170
/// behaviour.
pub fn declared_env(
    manifest_text: &str,
    host: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, crate::LoadError> {
    let manifest = crate::Manifest::parse(manifest_text)?;
    Ok(resolve(&manifest.env, host))
}

/// [`declared_env`] intersected with the set the user **approved** (gh #170):
/// the manifest declares, the user grants, and a declared name outside the
/// approved set is absent.
///
/// Until an install flow collects per-key approval this is the shipped
/// behaviour's seam: the declared set is the approved set, exactly as
/// `CapabilityGrants` documents for every other capability ("Phase5's install
/// flow intersects this with user approval; until then the declared set is the
/// granted set").
pub fn declared_env_approved(
    manifest_text: &str,
    approved: &[String],
    host: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, crate::LoadError> {
    let manifest = crate::Manifest::parse(manifest_text)?;
    let granted: Vec<String> = manifest
        .env
        .iter()
        .filter(|name| approved.iter().any(|approved| approved == *name))
        .cloned()
        .collect();
    Ok(resolve(&granted, host))
}

/// The intersection itself: names that exist on the host, by value.
fn resolve(names: &[String], host: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    names
        .iter()
        .filter_map(|name| host.get(name).map(|value| (name.clone(), value.clone())))
        .collect()
}

/// [`declared_env`] against the process environment.
///
/// The one function the host calls when it builds a store. Reading the real
/// environment here (rather than at manifest parse) is what makes a changed
/// value visible to the next instantiation without a restart.
pub fn declared_env_from_process(
    manifest_text: &str,
) -> Result<BTreeMap<String, String>, crate::LoadError> {
    let manifest = crate::Manifest::parse(manifest_text)?;
    let mut resolved = BTreeMap::new();
    for name in &manifest.env {
        if let Ok(value) = std::env::var(name) {
            resolved.insert(name.clone(), value);
        }
    }
    Ok(resolved)
}
