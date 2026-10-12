//! The `/extensions` hub's host side (#227): row assembly with
//! provenance, background catalog fetch, and consent-gated installs.
//! The interface owns the picker; this file owns data and workers.
//! Installs never bypass consent: the modal shows the row, the worker
//! re-verifies the resolved manifest against it before writing.

use std::sync::{Arc, Mutex};

use lca_registry::{CatalogEntry, CatalogSource, MergedEntry};

/// The official first-party catalog document URL: the in-repo
/// `extensions/catalog.toml` served raw. The binary never bakes the
/// catalog in (issue #227's no-bloat rule); the TTL cache covers
/// offline opens.
pub const OFFICIAL_CATALOG_URL: &str =
    "https://raw.githubusercontent.com/misaalanshori/lca/main/extensions/catalog.toml";

/// The official source for a running app: the URL above (the in-repo
/// file is the published document, not a runtime read).
pub fn official_source() -> CatalogSource {
    CatalogSource {
        id: "official".to_string(),
        name: "Official (misaalanshori/lca)".to_string(),
        url: Some(OFFICIAL_CATALOG_URL.to_string()),
    }
}

/// Assemble hub rows: installed first (sorted), then catalog order.
/// A catalog row whose name is installed stays listed under its
/// source (the hub marks it installed rather than hiding it, so an
/// update reads as already-here, not missing).
pub fn assemble_rows(
    installed: &[(String, String)],
    merged: &[MergedEntry],
) -> Vec<lca_ui::HubRow> {
    let installed_names: std::collections::HashSet<&str> =
        installed.iter().map(|(name, _)| name.as_str()).collect();
    let mut installed_rows: Vec<lca_ui::HubRow> = installed
        .iter()
        .map(|(name, version)| lca_ui::HubRow {
            name: name.clone(),
            version: version.clone(),
            capabilities: Vec::new(),
            description: "Installed.".to_string(),
            oci_ref: String::new(),
            source_name: String::new(),
            source_url: String::new(),
            official: false,
            installed: true,
        })
        .collect();
    installed_rows.sort_by(|a, b| a.name.cmp(&b.name));
    let mut rows = installed_rows;
    rows.extend(merged.iter().map(|row| {
        let entry: &CatalogEntry = &row.entry;
        lca_ui::HubRow {
            name: entry.name.clone(),
            version: entry.version.clone(),
            capabilities: entry.capabilities.clone(),
            description: entry.description.clone(),
            oci_ref: entry.oci_ref.clone(),
            source_name: row.source_name.clone(),
            source_url: row
                .source_url
                .clone()
                .unwrap_or_else(|| OFFICIAL_CATALOG_URL.to_string()),
            official: row.official,
            installed: installed_names.contains(entry.name.as_str()),
        }
    }));
    rows
}

/// Capability names a manifest declares, for the catalog-row check:
/// the install worker refuses a manifest reaching past what the row
/// (and the consent modal) named.
pub fn manifest_capability_names(manifest: &lca_ext_host::Manifest) -> Vec<String> {
    let mut names = Vec::new();
    if !manifest.fs.is_empty() {
        names.push("fs".to_string());
    }
    if manifest.process {
        names.push("process".to_string());
    }
    if manifest.pty {
        names.push("pty".to_string());
    }
    if !manifest.net.is_empty() {
        names.push("net".to_string());
    }
    if !manifest.net_local.is_empty() {
        names.push("net-local".to_string());
    }
    if manifest.oauth.is_some() {
        names.push("oauth".to_string());
    }
    if manifest.credentials {
        names.push("credentials".to_string());
    }
    if manifest.completion {
        names.push("completion".to_string());
    }
    if manifest.tools {
        names.push("tools".to_string());
    }
    if !manifest.ui_regions.is_empty() {
        names.push("ui".to_string());
    }
    names.sort();
    names
}

/// Verify a resolved manifest against the hub row consent approved:
/// same extension, same version, and no capability past what the row
/// named. A stale catalog row aborts before anything is written.
pub fn verify_against_row(
    manifest: &lca_ext_host::Manifest,
    row_name: &str,
    row_version: &str,
    row_capabilities: &[String],
) -> Result<(), String> {
    if manifest.name != row_name {
        return Err(format!(
            "catalog entry is stale: resolved `{}` but the hub offered `{row_name}`",
            manifest.name
        ));
    }
    if manifest.version != row_version {
        return Err(format!(
            "catalog entry is stale: resolved {} but the hub offered {row_version}",
            manifest.version
        ));
    }
    let mut extra: Vec<String> = manifest_capability_names(manifest)
        .into_iter()
        .filter(|cap| !row_capabilities.contains(cap))
        .collect();
    extra.sort();
    if extra.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "catalog entry is stale: the manifest reaches past it ({})",
            extra.join(", ")
        ))
    }
}

/// The hub's install-result slot, polled by the interface.
#[derive(Default)]
pub struct InstallSlot {
    /// A finished install's notice, taken once.
    pub finished: Option<String>,
}

/// Install one consented row on a worker thread (#227): resolve,
/// re-verify against the row, write through the same path the CLI
/// uses. Consent already happened in the modal; `yes` is implied.
pub fn spawn_install(
    data_dir: std::path::PathBuf,
    oci_ref: String,
    row_name: String,
    row_version: String,
    row_capabilities: Vec<String>,
    slot: Arc<Mutex<InstallSlot>>,
) {
    std::thread::spawn(move || {
        let registry = lca_registry::InstallTree::new(data_dir.join("extensions"));
        let notice = install_row(
            registry,
            &oci_ref,
            &row_name,
            &row_version,
            &row_capabilities,
        );
        *slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = InstallSlot {
            finished: Some(notice),
        };
    });
}

fn install_row(
    registry: lca_registry::InstallTree,
    oci_ref: &str,
    row_name: &str,
    row_version: &str,
    row_capabilities: &[String],
) -> String {
    let resolved = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime.block_on(lca_registry::resolve_oci(oci_ref)),
        Err(err) => {
            return format!("could not start the install: {err}");
        }
    };
    let resolved = match resolved {
        Ok(resolved) => resolved,
        Err(err) => return format!("could not fetch {oci_ref}: {err}"),
    };
    let manifest = match lca_ext_host::Manifest::parse(&resolved.manifest) {
        Ok(manifest) => manifest,
        Err(err) => return format!("the artifact failed validation: {err}"),
    };
    if let Err(stale) = verify_against_row(&manifest, row_name, row_version, row_capabilities) {
        return stale;
    }
    match registry.install(resolved) {
        Ok(entry) => format!(
            "installed {row_name} {row_version} ({})",
            entry.digest.chars().take(12).collect::<String>()
        ),
        Err(err) => format!("could not install {row_name}: {err}"),
    }
}

/// Fetch hub rows: installed plus catalogs (official + configured
/// customs), merged with provenance, ABI-filtered. Slow first
/// fetches run on the chat-spawned worker, never the interface
/// thread; failures degrade to installed-only with an offline note.
pub fn fetch_all(
    data_dir: &std::path::Path,
    customs: &[lca_config::CustomCatalog],
    installed: &[(String, String)],
) -> lca_ui::HubData {
    let mut sources = vec![official_source()];
    sources.extend(customs.iter().map(|custom| lca_registry::CatalogSource {
        id: custom.id.clone(),
        name: custom.name.clone(),
        url: Some(custom.url.clone()),
    }));
    let mut merged = Vec::new();
    let mut failed = Vec::new();
    for source in &sources {
        let cache = lca_registry::cache_path(data_dir, &source.id);
        let entries = match lca_core::drive_blocking({
            let source = source.clone();
            let cache = cache.clone();
            async move { lca_registry::fetch_catalog(&source, &cache, false).await }
        }) {
            Ok(entries) => entries,
            Err(_) => {
                failed.push(source.name.clone());
                Vec::new()
            }
        };
        merged.extend(
            entries
                .into_iter()
                .filter(lca_registry::abi_supported)
                .map(|entry| lca_registry::MergedEntry {
                    entry,
                    source_id: source.id.clone(),
                    source_name: source.name.clone(),
                    source_url: source.url.clone(),
                    official: source.url.is_none(),
                }),
        );
    }
    let rows = assemble_rows(installed, &merged);
    let notice = if merged.is_empty() && !failed.is_empty() {
        Some(format!(
            "extension catalogs are unreachable ({}) - showing installed only",
            failed.join(", ")
        ))
    } else if !failed.is_empty() {
        Some(format!("unreachable catalogs: {}", failed.join(", ")))
    } else {
        None
    };
    lca_ui::HubData {
        installed: rows.iter().filter(|row| row.installed).cloned().collect(),
        available: rows.into_iter().filter(|row| !row.installed).collect(),
        notice,
    }
}

impl super::Ui {
    /// Fetch hub rows for the chat worker: installed plus catalogs
    /// (official + configured customs), merged with provenance,
    /// ABI-filtered. Runs on the chat-spawned thread; slow first
    /// fetches never touch the interface thread.
    pub(super) fn hub_fetch(&self) -> lca_ui::HubData {
        let data = crate::data_dir();
        let installed = lca_registry::InstallTree::new(data.join("extensions"))
            .list()
            .unwrap_or_default()
            .into_iter()
            .map(|(name, entry)| (name, entry.version))
            .collect::<Vec<_>>();
        let customs = crate::lock(&self.config).extensions_catalogs().to_vec();
        fetch_all(&data, &customs, &installed)
    }

    /// Confirm an install the consent modal approved: spawn the
    /// worker now, report through the poll slot.
    pub(super) fn hub_confirm(&self, consent: lca_ui::InstallConsent) {
        let data = crate::data_dir();
        let slot = self.hub_install.clone();
        spawn_install(
            data,
            consent.artifact_ref,
            consent.name,
            consent.version,
            consent.capabilities,
            slot,
        );
    }

    /// Take a finished background install notice, once.
    pub(super) fn hub_poll(&self) -> Option<String> {
        crate::lock(&self.hub_install).finished.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn merged(name: &str, source: &str, official: bool, caps: &[&str]) -> MergedEntry {
        MergedEntry {
            entry: CatalogEntry {
                name: name.to_string(),
                version: "1.0.0".to_string(),
                abi: "0.6".to_string(),
                category: "provider".to_string(),
                description: format!("{name} does things."),
                oci_ref: format!("ghcr.io/example/{name}:abi-0.6"),
                capabilities: caps.iter().map(|cap| cap.to_string()).collect(),
            },
            source_id: source.to_string(),
            source_url: None,
            source_name: if official {
                "Official (misaalanshori/lca)".to_string()
            } else {
                source.to_string()
            },
            official,
        }
    }

    // Verifies: #227 - row assembly groups installed first (sorted),
    // then catalog order, marking installed rows wherever they also
    // list.
    #[test]
    fn assemble_rows_groups_installed_first() {
        let rows = assemble_rows(
            &[("b-tool".to_string(), "2.0.0".to_string())],
            &[
                merged("openrouter", "official", true, &["net"]),
                merged("b-tool", "official", true, &["net"]),
                merged("acme-deploy", "acme", false, &["process"]),
            ],
        );
        assert_eq!(rows.len(), 4);
        assert!(rows[0].installed);
        assert_eq!(rows[0].name, "b-tool");
        assert!(!rows[1].installed && rows[1].official);
        assert_eq!(rows[1].name, "openrouter");
        assert!(rows[2].installed, "listed-but-installed marks");
        assert!(!rows[3].official, "custom provenance rides");
        assert_eq!(rows[3].source_name, "acme");
    }

    // Verifies: #227 - the install worker refuses a stale catalog row:
    // renamed, reversioned, or reaching past the consented caps.
    #[test]
    fn verify_against_row_refuses_stale_entries() {
        let manifest = |name: &str, version: &str, caps: &[&str]| {
            let mut text = format!(
                "name = \"{name}\"\nversion = \"{version}\"\nabi = \"0.6\"\nworlds = [\"tool\"]\n"
            );
            for cap in caps {
                text.push_str(&format!("[capabilities.{cap}]\n"));
                if *cap == "net" {
                    text.push_str("hosts = [\"example.invalid\"]\n");
                }
                if *cap == "credentials" {
                    text.push_str("namespace = \"x\"\n");
                }
            }
            lca_ext_host::Manifest::parse(&text).expect("test manifest parses")
        };
        let net = manifest("probe", "1.0.0", &["net"]);
        assert!(verify_against_row(&net, "probe", "1.0.0", &["net".to_string()]).is_ok());
        assert!(verify_against_row(&net, "other", "1.0.0", &["net".to_string()]).is_err());
        assert!(verify_against_row(&net, "probe", "2.0.0", &["net".to_string()]).is_err());
        // Declared without the key is fine (consented superset); an
        // undeclared reach is not.
        assert!(verify_against_row(&net, "probe", "1.0.0", &[]).is_err());
        let plain = manifest("probe", "1.0.0", &[]);
        assert!(verify_against_row(&plain, "probe", "1.0.0", &["net".to_string()]).is_ok());
    }
}
