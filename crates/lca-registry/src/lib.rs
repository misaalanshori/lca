//! Reference resolution and installation state for extensions
//! (ADR-0010, FR-DIST-1..9): one lockfile, one digest-verification
//! path, four source kinds - OCI over the existing HTTP client, a
//! plain HTTPS zip archive, a local path, and a git repository clone
//! pinned by commit (gh #59).
//!
//! Everything here is bytes-in, records-out: the resolver verifies the
//! content digest before anything is written (FR-DIST-3/4), the store
//! writes the tree the SRDD describes (component named by its content
//! digest, the manifest, an approved-grant hash), and the lockfile at
//! the top records digest, source, and grant hash so `ext update` can
//! prompt before a widened capability set applies (FR-DIST-6/7).
//!
//! ## The module map (gh #561)
//!
//! This file is the crate's surface and its four resolvers. Everything else has
//! a module of its own, and the rule is that a new concern gets a module rather
//! than being appended here:
//!
//! | module | owns |
//! |---|---|
//! | `lockfile` | the format version, `LockEntry`, `Lockfile::{load, save}` |
//! | `consent` | the canonical grant hash, the consent lines, the update-widens comparison, the extension-name rule |
//! | `store` | `InstallTree`: the on-disk extension tree and its only writer |
//! | `http` | the shared hyper client and the plain GET (gh #355) |
//! | `proxy` | gh #308's proxy rules and connector |
//! | `oci` | the OCI token dance, the redirect fetch, the bounded collector, the archive fetch |
//! | `archive`, `catalog`, `git`, `models` | the formats and sources they are named for |
//!
//! Modules stay private: their public names are re-exported here, so a caller
//! still writes `lca_registry::Proxies` and never `lca_registry::proxy::Proxies`.

#![forbid(unsafe_code)]

use std::path::Path;

use sha2::{Digest, Sha256};

mod archive;
mod catalog;
mod consent;
mod git;
mod http;
mod lockfile;
pub mod models;
mod oci;
mod proxy;
mod store;

pub(crate) use archive::manifest_declares_resources;
pub use archive::{pack_archive, pack_archive_with_resources, read_archive};
pub use catalog::{
    CATALOG_FORMAT, CATALOG_TTL, CatalogEntry, CatalogSource, MergedEntry, abi_supported,
    cache_fresh, cache_path, fetch_catalog, is_valid_catalog_id, merge_catalogs, official_source,
    parse_catalog, read_cache, write_cache,
};
pub use consent::{consent_lines, grant_hash, update_widens_grants, validate_extension_name};
pub use git::{parse_git_source, resolve_git};
use http::http_client;
pub use http::{
    PLAIN_GET_MAX_BYTES, PLAIN_GET_TIMEOUT, plain_get, plain_get_within, plain_get_within_capped,
};
pub use lockfile::{LOCKFILE_VERSION, LockEntry, Lockfile};
pub use oci::{
    ARCHIVE_TIMEOUT, OCI_MAX_BYTES, OCI_MAX_REDIRECTS, OCI_TIMEOUT, fetch_oci_public,
    fetch_oci_within, fetch_oci_within_capped,
};
use oci::{fetch_bytes, fetch_oci};
pub use proxy::{Proxies, ProxyEndpoint, ProxyStream, ProxyTcp, parse_proxy_for_tests};
pub use store::InstallTree;

/// One resolved artifact, verified and ready to install: the two files
/// ADR-0010 says the archive carries, plus where they came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    /// The manifest text (`extension.toml`).
    pub manifest: String,
    /// The component bytes.
    pub component: Vec<u8>,
    /// `sha256:<hex>` of `component`.
    pub digest: String,
    /// The reference the digest was resolved from.
    pub source: String,
    /// The pinned commit for git sources (gh #59): empty otherwise.
    /// The lockfile keeps the typed ref (branches advance on update)
    /// and this pin separately, mirroring OCI's ref-plus-digest split.
    pub commit: String,
    /// The package's `resources/` bag (ADR-0030): `(relative path, bytes)`.
    /// Empty when the package ships none.
    pub resources: Vec<(String, Vec<u8>)>,
}

/// The parts of an extension package: the manifest, the component, and
/// the optional `resources/` bag (ADR-0030, ADR-0010's allowlist).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Archive {
    /// The manifest text (`extension.toml`).
    pub manifest: String,
    /// The component bytes.
    pub component: Vec<u8>,
    /// The `resources/` bag: `(relative path, bytes)`.
    pub resources: Vec<(String, Vec<u8>)>,
}

/// Per-file size cap for one resource. Kept in step with
/// `lca_tools::RESOURCE_FILE_MAX_BYTES` (the read-side cap); the installer
/// is the authority for the package budget (ADR-0030).
pub const RESOURCE_FILE_MAX_BYTES: u64 = 1024 * 1024;
/// Per-package size cap for the whole `resources/` bag (ADR-0030's
/// suggested 32 MB). A DoS guard, shown as a manifest-declared budget.
pub const RESOURCE_PACKAGE_MAX_BYTES: u64 = 32 * 1024 * 1024;

/// Ceiling on one archive member's **uncompressed** size (gh #350).
///
/// A component is code, not memory: the host caps a guest's linear memory
/// separately (manifest `memory_mb`, clamped at 512 MiB), so this number
/// bounds bytes on disk, not bytes the guest may allocate at run time. The
/// largest first-party component in the tree is ~1.8 MB, so 64 MiB is
/// generous by more than an order of magnitude while still turning a
/// decompression bomb into a named refusal instead of an OOM.
pub const MAX_COMPONENT_BYTES: u64 = 64 * 1024 * 1024;

/// Ceiling on an `extension.toml` from an archive (gh #350). A manifest is
/// a page of keys; 1 MiB is generous. Without a bound, the manifest read
/// was the one member with no ceiling anywhere.
pub const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;

/// Ceiling on an archive's **compressed** size, i.e. what one download may
/// retain (gh #350). Zip's compression ratio caps how far a bomb can
/// expand, so an archive smaller than this cannot exceed the member
/// ceilings by much; the per-member `take` bounds still do the real work.
/// 64 MiB is the same order as one component's ceiling.
pub const MAX_ARCHIVE_BYTES: u64 = 64 * 1024 * 1024;

/// Refuse an archive carrying more members than this (gh #350). A package
/// is `extension.toml`, one component, and a `resources/` bag; a few
/// hundred files is already an outlier. This bounds the name-vector
/// allocation before any entry is opened, which is the part a bomb made of
/// many tiny entries would otherwise multiply.
pub const MAX_ARCHIVE_ENTRIES: usize = 1024;

impl Resolved {
    /// Content-digest a component (FR-DIST-3's value).
    pub fn digest_of(component: &[u8]) -> String {
        format!("sha256:{:x}", Sha256::digest(component))
    }

    /// Content-digest a package whose payload is its manifest and
    /// `resources/` bag rather than a component (a data-only package,
    /// ADR-0030). Deterministic: the manifest bytes, then each resource's
    /// path and bytes in sorted order, length-prefixed so a path cannot be
    /// confused with the bytes that follow it. Without this the digest was
    /// the empty-component digest for every data-only package, so
    /// `ext update` could never see a new version.
    pub fn digest_of_package(manifest: &str, resources: &[(String, Vec<u8>)]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(b"lca-package\0");
        hasher.update((manifest.len() as u64).to_le_bytes());
        hasher.update(manifest.as_bytes());
        let mut entries: Vec<&(String, Vec<u8>)> = resources.iter().collect();
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        for (path, bytes) in entries {
            hasher.update((path.len() as u64).to_le_bytes());
            hasher.update(path.as_bytes());
            hasher.update((bytes.len() as u64).to_le_bytes());
            hasher.update(bytes);
        }
        format!("sha256:{:x}", hasher.finalize())
    }
}

/// Everything that can go wrong resolving or installing.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Transport, HTTP status, or malformed registry response.
    #[error("fetch failed: {0}")]
    Fetch(String),
    /// The digest did not match the bytes (FR-DIST-4's caller deletes).
    #[error("digest mismatch: expected {expected}, got {actual}")]
    DigestMismatch {
        /// What the source claimed.
        expected: String,
        /// What the bytes hash to.
        actual: String,
    },
    /// The source or archive did not carry both required files.
    #[error("invalid artifact: {0}")]
    Invalid(String),
    /// The lockfile or tree is unreadable/writable.
    #[error("{0}")]
    Io(String),
    /// The lockfile parses but does not make sense.
    #[error("corrupt: {0}")]
    Corrupt(String),
}

impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Self {
        Error::Io(err.to_string())
    }
}

/// Check a claimed digest against the bytes (FR-DIST-3/4).
pub fn verify_digest(claimed: &str, bytes: &[u8]) -> Result<(), Error> {
    let actual = Resolved::digest_of(bytes);
    if actual != claimed {
        return Err(Error::DigestMismatch {
            expected: claimed.to_string(),
            actual,
        });
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Resolvers
// ---------------------------------------------------------------------------

/// Resolve any source kind into the verified pair (FR-DIST-1/5/9).
/// Dispatch: an existing path is a local install; an `http(s)://` URL
/// is an archive; anything else is an OCI reference.
pub async fn resolve(source: &str, local_manifest: Option<&Path>) -> Result<Resolved, Error> {
    if source.starts_with("http://") || source.starts_with("https://") {
        return resolve_archive(source).await;
    }
    if source.starts_with("git+") {
        return resolve_git(source);
    }
    let path = Path::new(source);
    if path.exists() {
        return resolve_local(path, local_manifest);
    }
    resolve_oci(source).await
}

/// FR-DIST-5: read both files from a local path (the manifest defaults
/// to `extension.toml` beside the component).
pub fn resolve_local(component: &Path, manifest_path: Option<&Path>) -> Result<Resolved, Error> {
    // A directory is a data-only package (ADR-0030): no component, just a
    // manifest and a `resources/` bag.
    if component.is_dir() {
        let manifest_path = manifest_path
            .map(|path| path.to_path_buf())
            .unwrap_or_else(|| component.join("extension.toml"));
        let manifest = std::fs::read_to_string(&manifest_path)?;
        let resources = read_resource_dir(&component.join("resources"))?;
        let digest = Resolved::digest_of_package(&manifest, &resources);
        // Gh #258: the resolution (digest prefix, never content).
        lca_debug::emit(
            lca_debug::Domain::Registry,
            "resolve.local",
            &format!(
                "digest={} path={}",
                digest.chars().take(12).collect::<String>(),
                component.display()
            ),
        );
        return Ok(Resolved {
            digest,
            source: component.display().to_string(),
            commit: String::new(),
            manifest,
            component: Vec::new(),
            resources,
        });
    }
    let manifest_path = match manifest_path {
        Some(path) => path.to_path_buf(),
        None => component
            .parent()
            .map(|dir| dir.join("extension.toml"))
            .ok_or_else(|| Error::Invalid("no manifest path".to_string()))?,
    };
    let manifest = std::fs::read_to_string(&manifest_path)?;
    let component_bytes = std::fs::read(component)?;
    let resources = match component.parent() {
        Some(dir) => read_resource_dir(&dir.join("resources"))?,
        None => Vec::new(),
    };
    Ok(Resolved {
        digest: Resolved::digest_of(&component_bytes),
        commit: String::new(),
        source: component.display().to_string(),
        manifest,
        component: component_bytes,
        resources,
    })
}

/// Whether an archive entry's path is a safe relative resource path:
/// no absolute root, no `..`, no backslash, no NUL, not empty. Shared by
/// the archive reader (gh #350's member ceiling runs on top of this) and the
/// resource-directory reader that installs a local package.
pub(crate) fn valid_archive_path(path: &str) -> bool {
    !path.is_empty()
        && !path.contains('\0')
        && !path.contains('\\')
        && !path.starts_with('/')
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

/// Read a package's `resources/` directory into `(relative path, bytes)`.
/// A missing directory is an empty bag, not an error.
pub(crate) fn read_resource_dir(dir: &Path) -> Result<Vec<(String, Vec<u8>)>, Error> {
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut files = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    let mut bag_bytes = 0u64;
    while let Some(current) = stack.pop() {
        for entry in std::fs::read_dir(&current)? {
            let entry = entry?;
            let path = entry.path();
            let meta = entry.metadata()?;
            if meta.is_dir() {
                stack.push(path);
                continue;
            }
            if !meta.is_file() {
                continue;
            }
            let rel = path
                .strip_prefix(dir)
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            if !valid_archive_path(&rel) {
                return Err(Error::Invalid(format!(
                    "resource path `{rel}` is not a safe relative path"
                )));
            }
            if meta.len() > RESOURCE_FILE_MAX_BYTES {
                return Err(Error::Invalid(format!(
                    "resource `{rel}` is {} bytes, over the {RESOURCE_FILE_MAX_BYTES} byte per-file cap",
                    meta.len()
                )));
            }
            bag_bytes += meta.len();
            if bag_bytes > RESOURCE_PACKAGE_MAX_BYTES {
                return Err(Error::Invalid(format!(
                    "the resource bag exceeds the {RESOURCE_PACKAGE_MAX_BYTES} byte package cap"
                )));
            }
            files.push((rel, std::fs::read(&path)?));
        }
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(files)
}

/// FR-DIST-9: fetch the zip, unpack the two files; the digest is
/// computed here (an archive URL carries no external claim - the
/// lockfile records what this fetch produced, and `ext update` compares
/// against it next time).
pub async fn resolve_archive(url: &str) -> Result<Resolved, Error> {
    let client = http_client()?;
    let (bytes, _) = fetch_bytes(&client, url, "application/zip, application/octet-stream").await?;
    let archive = read_archive(&bytes)?;
    let digest = if archive.component.is_empty() {
        Resolved::digest_of_package(&archive.manifest, &archive.resources)
    } else {
        Resolved::digest_of(&archive.component)
    };
    Ok(Resolved {
        digest,
        source: url.to_string(),
        commit: String::new(),

        manifest: archive.manifest,
        component: archive.component,
        resources: archive.resources,
    })
}

/// The official first-party repository: bare install names land here.
const OFFICIAL_REGISTRY_HOST: &str = "ghcr.io";
/// The official organization prefix bare names expand under.
const OFFICIAL_REGISTRY_ORG: &str = "misaalanshori/lca";

/// Expand an official shorthand (gh #252, Docker-style): `name` and
/// `name:tag` (no slash) become `ghcr.io/misaalanshori/lca/name[:tag]`,
/// the tag defaulting to the current ABI line. Anything with a slash
/// (explicit OCI refs, archive URLs, git sources, local paths) — and
/// digest pins (`@`), which never had shorthand — passes through
/// untouched.
pub fn expand_shorthand(source: &str) -> String {
    if source.contains('/') || source.contains('\\') || source.contains('@') {
        return source.to_string();
    }
    let (name, tag) = match source.split_once(':') {
        Some((name, tag)) => (name, tag),
        None => (source, ""),
    };
    // No image name (`:tag`): leave it for the reference parser's
    // own error rather than inventing a path.
    if name.is_empty() {
        return source.to_string();
    }
    let tag = if tag.is_empty() {
        format!("abi-{}", lca_ext_abi::ABI_VERSION)
    } else {
        tag.to_string()
    };
    format!("{OFFICIAL_REGISTRY_HOST}/{OFFICIAL_REGISTRY_ORG}/{name}:{tag}")
}

/// Split `host/repo/name:tag` (the tag comes after the last colon that
/// sits past the last slash, so `host:5000/repo/name` parses right).
/// Gh #252: a reference with no slash is an official first-party
/// shorthand — it expands here first (see [`expand_shorthand`]), so
/// this still only ever sees full references.
fn split_oci_reference(reference: &str) -> Result<(&str, &str, &str), Error> {
    let slash = reference.find('/').ok_or_else(|| {
        Error::Invalid(format!(
            "`{reference}` is not an OCI reference (no registry host)"
        ))
    })?;
    let (host, rest) = reference.split_at(slash);
    let rest = &rest[1..];
    let tag = match rest.rfind(':') {
        Some(colon) if colon > rest.rfind('/').unwrap_or(0) => &rest[colon + 1..],
        _ => "",
    };
    let name = if tag.is_empty() {
        rest
    } else {
        &rest[..rest.len() - tag.len() - 1]
    };
    if name.is_empty() {
        return Err(Error::Invalid(format!("`{reference}` has no image name")));
    }
    Ok((host, name, tag))
}

/// The media types the two blobs ride under (our convention: the
/// manifest's config blob carries `extension.toml`, layer0 carries the
/// component - documented in the authoring guide's publishing section).
const MANIFEST_MEDIA_TYPES: &str = "application/vnd.oci.image.manifest.v1+json,      application/vnd.docker.distribution.manifest.v2+json";
const WASM_LAYER: &str = "application/wasm";

/// FR-DIST-1: the OCI distribution protocol, anonymous pulls only
/// (the risk table's decision: authenticated registry support is the
/// part that may slip, the HTTPS archive covers distribution meanwhile).
pub async fn resolve_oci(reference: &str) -> Result<Resolved, Error> {
    // Gh #252: bare names expand to the official repository first.
    let expanded = expand_shorthand(reference);
    let (host, name, tag) = split_oci_reference(&expanded)?;
    let tag = if tag.is_empty() { "latest" } else { tag };
    let client = http_client()?;
    let scheme = if host.starts_with("localhost") || host.starts_with("127.0.0.1") {
        "http"
    } else {
        "https"
    };
    let base = format!("{scheme}://{host}/v2/{name}");
    let image = name;
    let mut token: Option<String> = None;
    let (manifest_bytes, _headers) = fetch_oci(
        &client,
        &format!("{base}/manifests/{tag}"),
        MANIFEST_MEDIA_TYPES,
        image,
        &mut token,
    )
    .await?;
    let manifest_json: serde_json::Value = serde_json::from_slice(&manifest_bytes)
        .map_err(|err| Error::Fetch(format!("registry manifest is not JSON: {err}")))?;

    let blob_digest = |descriptor: &serde_json::Value| -> Result<String, Error> {
        descriptor
            .get("digest")
            .and_then(|d| d.as_str())
            .map(str::to_string)
            .ok_or_else(|| Error::Invalid("registry manifest has an undigested blob".to_string()))
    };

    // Config blob = extension.toml (our publishing convention), with
    // layer1 as the fallback a two-layer artifact would use.
    let config_digest = blob_digest(
        manifest_json
            .get("config")
            .unwrap_or(&serde_json::Value::Null),
    )?;
    let (extension_toml, _) = match fetch_oci(
        &client,
        &format!("{base}/blobs/{config_digest}"),
        "*",
        image,
        &mut token,
    )
    .await
    {
        Ok(found) => found,
        Err(config_err) => {
            let layer = manifest_json
                .get("layers")
                .and_then(|l| l.as_array())
                .and_then(|layers| layers.get(1))
                .ok_or(config_err)?;
            let digest = blob_digest(layer)?;
            let found = fetch_oci(
                &client,
                &format!("{base}/blobs/{digest}"),
                "*/*",
                image,
                &mut token,
            )
            .await?;
            verify_digest(&digest, &found.0)?;
            found
        }
    };
    verify_digest(&config_digest, &extension_toml)?;
    let extension_toml = String::from_utf8(extension_toml)
        .map_err(|_| Error::Invalid("the manifest blob is not UTF-8".to_string()))?;

    let layer = manifest_json
        .get("layers")
        .and_then(|l| l.as_array())
        .and_then(|layers| layers.first())
        .ok_or_else(|| Error::Invalid("registry manifest has no layers".to_string()))?;
    if layer.get("mediaType").and_then(|m| m.as_str()) != Some(WASM_LAYER)
        && !layer
            .get("mediaType")
            .and_then(|m| m.as_str())
            .is_some_and(|m| m.contains("wasm"))
    {
        return Err(Error::Invalid(format!(
            "the first layer is {}, not a WebAssembly component",
            layer
                .get("mediaType")
                .and_then(|m| m.as_str())
                .unwrap_or("missing")
        )));
    }
    let layer_digest = blob_digest(layer)?;
    let (component, _) = fetch_oci(
        &client,
        &format!("{base}/blobs/{layer_digest}"),
        "*/*",
        image,
        &mut token,
    )
    .await?;
    verify_digest(&layer_digest, &component)?;

    Ok(Resolved {
        digest: Resolved::digest_of(&component),
        commit: String::new(),
        source: reference.to_string(),
        manifest: extension_toml,
        component,
        // ponytail: OCI carries no `resources` layer, so an OCI install
        // resolves an empty bag. This is deliberate, not silent: the
        // first-party resource consumer (openai-compatible) serves the same
        // bag from the compiled-in table (ADR-0032), and the HTTPS archive
        // and local-path installs carry resources fully. A *third-party*
        // OCI package that declares a bag gets none - the ceiling. Refusing
        // here instead was tried and reverted: every published artifact
        // declares `resources`, so a hard error broke the shipped install
        // path. Add the third layer (and the publish-script change) when a
        // third-party OCI package needs a bag.
        resources: Vec::new(),
    })
}
