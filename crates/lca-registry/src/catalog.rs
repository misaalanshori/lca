//! Extension catalogs (#227): the in-repo first-party catalog plus
//! user-configured custom catalogs, merged with provenance and
//! filtered by the host ABI window. Split from `lib.rs` for the
//! workspace file ceiling (gate 11). Fetch honors a TTL cache on
//! disk; offline falls back to cache, never crashes the hub.

use super::{Error, http};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// The catalog format this host reads (`[version] format`).
pub const CATALOG_FORMAT: u32 = 1;

/// Catalog cache time-to-live (#227: user-initiated fetch, no
/// background telemetry; an hour-old copy is fresh enough).
pub const CATALOG_TTL: Duration = Duration::from_secs(3600);

/// One catalog entry: what the hub lists and installs from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogEntry {
    /// Extension name (matches the manifest's).
    pub name: String,
    /// Release version.
    pub version: String,
    /// The ABI line.
    pub abi: String,
    /// `provider`, `compaction`, `bridge`, `context-transform`, ...
    pub category: String,
    /// One-line description.
    pub description: String,
    /// Where the component comes from (`ghcr.io/...` or `https://`).
    pub oci_ref: String,
    /// Capability names (`net`, `credentials`, ...).
    pub capabilities: Vec<String>,
}

/// Where an entry was discovered: the official catalog or a named
/// custom one. Provenance rides every merged row to the hub.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogSource {
    /// Stable id (`official`, or the user's configured id).
    pub id: String,
    /// Display name (`Official (misaalanshori/lca)`, ...).
    pub name: String,
    /// The catalog URL. `None` is the in-repo official catalog,
    /// which the binary reads from its own tree.
    pub url: Option<String>,
}

/// The official first-party source: `extensions/catalog.toml`.
pub fn official_source() -> CatalogSource {
    CatalogSource {
        id: "official".to_string(),
        name: "Official (misaalanshori/lca)".to_string(),
        url: None,
    }
}

/// Parse one catalog document: format check first, then entries.
/// Unknown keys ride along ignored; a missing or mistyped field
/// refuses the file, never half-loads it.
pub fn parse_catalog(text: &str) -> Result<Vec<CatalogEntry>, Error> {
    let value: toml::Value = text
        .parse()
        .map_err(|err| Error::Invalid(format!("catalog does not parse: {err}")))?;
    let format = value
        .get("version")
        .and_then(|version| version.get("format"))
        .and_then(|format| format.as_integer())
        .ok_or_else(|| Error::Invalid("catalog lacks [version] format".to_string()))?;
    if format as u32 != CATALOG_FORMAT {
        return Err(Error::Invalid(format!(
            "catalog format {format} is not {CATALOG_FORMAT}"
        )));
    }
    let empty = Vec::new();
    let raw = value
        .get("extension")
        .and_then(|entries| entries.as_array())
        .unwrap_or(&empty);
    let mut entries = Vec::new();
    for entry in raw {
        entries.push(CatalogEntry {
            name: required_str(entry, "name")?,
            version: required_str(entry, "version")?,
            abi: required_str(entry, "abi")?,
            category: required_str(entry, "category")?,
            description: required_str(entry, "description")?,
            oci_ref: required_str(entry, "oci_ref")?,
            capabilities: entry
                .get("capabilities")
                .and_then(|list| list.as_array())
                .map(|list| {
                    list.iter()
                        .filter_map(|item| item.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default(),
        });
    }
    Ok(entries)
}

/// Merge catalogs preserving source order: official entries first,
/// then each custom catalog's, every row carrying its provenance. A
/// name present in several catalogs lists once per source, so the
/// hub can show both and the user picks knowingly.
pub fn merge_catalogs(sources: &[(CatalogSource, Vec<CatalogEntry>)]) -> Vec<MergedEntry> {
    let mut merged = Vec::new();
    for (source, entries) in sources {
        for entry in entries {
            merged.push(MergedEntry {
                entry: entry.clone(),
                source_id: source.id.clone(),
                source_name: source.name.clone(),
                official: source.url.is_none(),
            });
        }
    }
    merged
}

/// One merged row: the entry plus where it was discovered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergedEntry {
    /// The catalog entry.
    pub entry: CatalogEntry,
    /// Its catalog's stable id.
    pub source_id: String,
    /// Its catalog's display name.
    pub source_name: String,
    /// True for the in-repo official catalog.
    pub official: bool,
}

/// Keep entries the host can load: the ABI window rule both the
/// manifest check and this filter answer through
/// (`lca_ext_abi::abi_in_window`).
pub fn abi_supported(entry: &CatalogEntry) -> bool {
    lca_ext_abi::abi_in_window(&entry.abi)
}

/// Read a cached catalog copy: `Some((entries, fresh))`, `None` when
/// absent or unparseable. Freshness is file age against
/// [`CATALOG_TTL`]; a stale copy still parses (the fetch path
/// refreshes it), so callers distinguish rather than fail.
pub fn read_cache(path: &Path) -> Option<(Vec<CatalogEntry>, bool)> {
    let text = std::fs::read_to_string(path).ok()?;
    let entries = parse_catalog(&text).ok()?;
    let age = std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok());
    Some((entries, age.is_some_and(cache_fresh)))
}

/// Whether a cache age is still fresh: under [`CATALOG_TTL`].
/// The comparison the hub trusts; `read_cache` feeds it file age.
/// Split out so tests pin the boundary without touching the clock.
pub fn cache_fresh(age: Duration) -> bool {
    age < CATALOG_TTL
}

/// Write a fetched catalog copy for the TTL cache.
pub fn write_cache(path: &Path, text: &str) -> Result<(), Error> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, text)?;
    Ok(())
}

/// Fetch one catalog document: the TTL cache first (unless forced),
/// else HTTPS, caching what lands. Offline with a stale copy serves
/// the copy; offline with nothing is an error the hub reports as
/// offline guidance, never a crash.
pub async fn fetch_catalog(
    source: &CatalogSource,
    cache_path: &Path,
    force_refresh: bool,
) -> Result<Vec<CatalogEntry>, Error> {
    if !force_refresh && let Some((entries, true)) = read_cache(cache_path) {
        return Ok(entries);
    }
    let Some(url) = source.url.as_deref() else {
        // The official catalog ships in-repo; without it there is
        // nothing to fetch (a source build reads the tree directly).
        return read_cache(cache_path)
            .map(|(entries, _)| entries)
            .ok_or_else(|| Error::Invalid("official catalog is unavailable".to_string()));
    };
    match http::plain_get(url).await {
        Ok(bytes) => {
            let text = String::from_utf8(bytes)
                .map_err(|err| Error::Invalid(format!("catalog is not UTF-8: {err}")))?;
            let entries = parse_catalog(&text)?;
            let _ = write_cache(cache_path, &text);
            Ok(entries)
        }
        Err(err) => read_cache(cache_path)
            .map(|(entries, _)| entries)
            .ok_or(err),
    }
}

/// The cache file for one catalog under a data directory
/// (`~/.lca/catalogs/<id>.toml`).
pub fn cache_path(data_dir: &Path, id: &str) -> PathBuf {
    data_dir.join("catalogs").join(format!("{id}.toml"))
}

fn required_str(entry: &toml::Value, key: &str) -> Result<String, Error> {
    entry
        .get(key)
        .and_then(|value| value.as_str())
        .map(str::to_string)
        .ok_or_else(|| Error::Invalid(format!("catalog entry lacks `{key}`")))
}
