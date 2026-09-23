//! Reference resolution and installation state for extensions
//! (ADR-0010, FR-DIST-1..9): one lockfile, one digest-verification
//! path, three source kinds - OCI over the existing HTTP client, a
//! plain HTTPS zip archive, and a local path.
//!
//! Everything here is bytes-in, records-out: the resolver verifies the
//! content digest before anything is written (FR-DIST-3/4), the store
//! writes the tree the SRDD describes (component named by its content
//! digest, the manifest, an approved-grant hash), and the lockfile at
//! the top records digest, source, and grant hash so `ext update` can
//! prompt before a widened capability set applies (FR-DIST-6/7).

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// The lockfile format version.
pub const LOCKFILE_VERSION: u32 = 1;

/// One installed extension's lockfile record (SRDD's extension-tree
/// paragraph: digest, source reference, approved-capability hash).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LockEntry {
    /// `sha256:<hex>` of the component bytes; every later load uses it
    /// and never re-resolves a moving tag (FR-DIST-8).
    pub digest: String,
    /// What it was resolved from: an OCI reference, an HTTPS URL, or a
    /// local path (ADR-0010: same machinery, different string).
    pub source: String,
    /// Hash of the manifest-declared grant set the user approved
    /// (FR-DIST-7's comparison basis; ad hoc grants live in the user
    /// grant store, not here).
    pub grant_hash: String,
    /// The extension's manifest `version`, for `ext list`/`ext info`.
    pub version: String,
    /// The manifest's ABI line, for the same display.
    pub abi: String,
}

/// The extension lockfile at the top of the install tree.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Lockfile {
    /// Format version.
    #[serde(default = "lock_version")]
    pub version: u32,
    /// Extension name -> record.
    #[serde(default)]
    pub extensions: BTreeMap<String, LockEntry>,
}

fn lock_version() -> u32 {
    LOCKFILE_VERSION
}

impl Lockfile {
    /// Read a lockfile, absent meaning none installed yet.
    pub fn load(path: &Path) -> Result<Lockfile, Error> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Lockfile::default());
            }
            Err(err) => return Err(Error::Io(err.to_string())),
        };
        let lock: Lockfile = serde_json::from_str(&text)
            .map_err(|err| Error::Corrupt(format!("{}: {err}", path.display())))?;
        if lock.version > LOCKFILE_VERSION {
            return Err(Error::Corrupt(format!(
                "{}: lockfile version {} is newer than this host understands ({LOCKFILE_VERSION})",
                path.display(),
                lock.version
            )));
        }
        Ok(lock)
    }

    /// Write atomically: temp file, then rename (session-log format's
    /// durability rule, applied to this store too).
    pub fn save(&self, path: &Path) -> Result<(), Error> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(self).map_err(|err| Error::Io(err.to_string()))?;
        let temp = path.with_extension("json.tmp");
        std::fs::write(&temp, text)?;
        std::fs::rename(&temp, path)?;
        Ok(())
    }
}

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
}

impl Resolved {
    /// Content-digest a component (FR-DIST-3's value).
    pub fn digest_of(component: &[u8]) -> String {
        format!("sha256:{:x}", Sha256::digest(component))
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

// ---------------------------------------------------------------------------
// The approved-grant hash (FR-DIST-7's comparison basis)
// ---------------------------------------------------------------------------

/// Canonical text for a TOML value: tables sorted by key at every depth,
/// so two manifests declaring the same grants in a different written
/// order hash the same, and any changed value hashes differently.
fn canonical(value: &toml::Value) -> String {
    match value {
        toml::Value::Table(table) => {
            let mut keys: Vec<&String> = table.keys().collect();
            keys.sort();
            let inner: Vec<String> = keys
                .into_iter()
                .map(|key| format!("{key}={}", canonical(&table[key])))
                .collect();
            format!("{{{}}}", inner.join(","))
        }
        toml::Value::Array(array) => {
            let inner: Vec<String> = array.iter().map(canonical).collect();
            format!("[{}]", inner.join(","))
        }
        other => other.to_string(),
    }
}

/// Hash of a manifest's declared grant set: exactly what the consent
/// screen showed, nothing else (SRDD's extension-tree paragraph: ad hoc
/// grants live in the user grant store and stay out of this hash).
pub fn grant_hash(manifest: &str) -> Result<String, Error> {
    let parsed: toml::Value = manifest
        .parse()
        .map_err(|err| Error::Invalid(format!("manifest does not parse: {err}")))?;
    let capabilities = parsed
        .get("capabilities")
        .cloned()
        .unwrap_or(toml::Value::Table(toml::map::Map::new()));
    Ok(format!(
        "sha256:{:x}",
        Sha256::digest(canonical(&capabilities).as_bytes())
    ))
}

// ---------------------------------------------------------------------------
// Consent text (capabilities catalog: the manifest is the consent surface)
// ---------------------------------------------------------------------------

/// One consent line per declared capability, in the catalog's words.
pub fn consent_lines(manifest: &str) -> Result<Vec<String>, Error> {
    let parsed: toml::Value = manifest
        .parse()
        .map_err(|err| Error::Invalid(format!("manifest does not parse: {err}")))?;
    let Some(table) = parsed.get("capabilities").and_then(|c| c.as_table()) else {
        return Ok(Vec::new());
    };
    let mut lines = Vec::new();
    let reason = |table: &toml::Value, key: &str| {
        table
            .get(key)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };
    for (name, value) in table {
        match name.as_str() {
            "fs" => {
                let mut scopes = Vec::new();
                if let Some(table) = value.as_table() {
                    let mut keys: Vec<&String> = table.keys().collect();
                    keys.sort();
                    for scope in keys {
                        let mode = table[scope].as_str().unwrap_or("read");
                        scopes.push(match mode {
                            "read-write" => format!("{scope} (read and write)"),
                            other => format!("{scope} ({other})"),
                        });
                    }
                }
                lines.push(format!(
                    "Files: {}. This lets it read and write those files.",
                    scopes.join(", ")
                ));
            }
            "process" => lines.push(format!(
                "{} Each command still asks for your approval.",
                reason(value, "reason")
            )),
            "pty" => lines.push(format!(
                "{} This gives it an interactive terminal session.",
                reason(value, "reason")
            )),
            "net" => {
                if let Some(hosts) = value.get("hosts").and_then(|h| h.as_array()) {
                    let names: Vec<String> = hosts
                        .iter()
                        .filter_map(|h| h.as_str())
                        .map(|h| h.replace("*.", "any subdomain of "))
                        .collect();
                    lines.push(format!("Connect to {}. HTTPS only.", names.join(", and ")));
                }
            }
            "net-local" => lines.push(
                "Connect to a device on your local network, your own machine, \
                 or your private tailnet."
                    .to_string(),
            ),
            "oauth" => lines.push(
                "Open a browser sign-in and receive the response on a local port.".to_string(),
            ),
            "credentials" => lines.push("Store and read its own saved credentials.".to_string()),
            "completion" => lines.push(format!(
                "{} This lets it ask the current model for a response.",
                reason(value, "reason")
            )),
            "ui" => lines.push(
                "Show a segment in the status line and content in the side panel.".to_string(),
            ),
            other => {
                return Err(Error::Invalid(format!(
                    "unknown capability `{other}` in the manifest"
                )));
            }
        }
    }
    Ok(lines)
}

// ---------------------------------------------------------------------------
// The install tree (SRDD's extension-tree paragraph)
// ---------------------------------------------------------------------------

/// Where extensions install: `<root>` is the user data directory's
/// `extensions` tree; the lockfile sits at its top (FR-DIST-6).
pub struct InstallTree {
    root: PathBuf,
}

impl InstallTree {
    /// The tree under a root directory.
    pub fn new(root: impl Into<PathBuf>) -> InstallTree {
        InstallTree { root: root.into() }
    }

    /// Where the lockfile lives.
    pub fn lockfile_path(&self) -> PathBuf {
        self.root.join("lockfile.json")
    }

    /// The installed component's path, named by its content digest
    /// (digest-as-filename is what makes load-by-digest structural,
    /// FR-DIST-8).
    pub fn component_path(&self, name: &str, digest: &str) -> PathBuf {
        let file = digest.replace(':', "-");
        self.root.join(name).join(format!("{file}.wasm"))
    }

    /// The installed manifest's path.
    pub fn manifest_path(&self, name: &str) -> PathBuf {
        self.root.join(name).join("extension.toml")
    }

    /// The denial journal `ext info` counts (FR-EXT-9).
    pub fn denials_path(&self, name: &str) -> PathBuf {
        self.root.join(name).join("denials.jsonl")
    }

    /// Write one verified artifact into the tree and record it.
    pub fn install(&self, resolved: Resolved) -> Result<LockEntry, Error> {
        let manifest = resolved.manifest.clone();
        let value: toml::Value = manifest
            .parse()
            .map_err(|err| Error::Invalid(format!("manifest does not parse: {err}")))?;
        let name = value
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| Error::Invalid("manifest has no name".to_string()))?
            .to_string();
        let entry = LockEntry {
            digest: resolved.digest.clone(),
            source: resolved.source.clone(),
            grant_hash: grant_hash(&manifest)?,
            version: value
                .get("version")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            abi: value
                .get("abi")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
        };
        let dir = self.root.join(&name);
        std::fs::create_dir_all(&dir)?;
        std::fs::write(self.manifest_path(&name), &manifest)?;
        std::fs::write(
            self.component_path(&name, &resolved.digest),
            &resolved.component,
        )?;
        let mut lock = Lockfile::load(&self.lockfile_path())?;
        lock.extensions.insert(name, entry.clone());
        lock.save(&self.lockfile_path())?;
        Ok(entry)
    }

    /// Read an installed component by its recorded digest
    /// (FR-DIST-8: the lockfile decides, no network is consulted).
    pub fn component(&self, name: &str, digest: &str) -> Result<Vec<u8>, Error> {
        std::fs::read(self.component_path(name, digest)).map_err(|err| {
            Error::Io(format!(
                "{}: {err}",
                self.component_path(name, digest).display()
            ))
        })
    }

    /// Read an installed manifest.
    pub fn manifest(&self, name: &str) -> Result<String, Error> {
        std::fs::read_to_string(self.manifest_path(name))
            .map_err(|err| Error::Io(format!("{}: {err}", self.manifest_path(name).display())))
    }

    /// Forget one extension: tree and lockfile record.
    pub fn remove(&self, name: &str) -> Result<bool, Error> {
        let mut lock = Lockfile::load(&self.lockfile_path())?;
        let removed = lock.extensions.remove(name).is_some();
        if removed {
            lock.save(&self.lockfile_path())?;
            let _ = std::fs::remove_dir_all(self.root.join(name));
        }
        Ok(removed)
    }

    /// Every installed record (name, entry), sorted by name.
    pub fn list(&self) -> Result<Vec<(String, LockEntry)>, Error> {
        Ok(Lockfile::load(&self.lockfile_path())?
            .extensions
            .into_iter()
            .collect())
    }

    /// One record, when installed.
    pub fn entry(&self, name: &str) -> Result<Option<LockEntry>, Error> {
        Ok(Lockfile::load(&self.lockfile_path())?
            .extensions
            .get(name)
            .cloned())
    }

    /// How many denials `ext info` should show (FR-EXT-9): lines in the
    /// extension's journal; absent means none.
    pub fn denial_count(&self, name: &str) -> usize {
        std::fs::read_to_string(self.denials_path(name))
            .map(|text| text.lines().filter(|line| !line.trim().is_empty()).count())
            .unwrap_or(0)
    }
}

// ---------------------------------------------------------------------------
// Archives (ADR-0010's zip: extension.toml + the component, nothing else)
// ---------------------------------------------------------------------------

/// Pack the two files ADR-0010 names into a zip (authors and tests).
pub fn pack_archive(manifest: &str, component: &[u8]) -> Result<Vec<u8>, Error> {
    let mut cursor = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut cursor);
        let stored = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        let deflated = zip::write::SimpleFileOptions::default();
        writer
            .start_file("extension.toml", deflated)
            .map_err(|err| Error::Invalid(err.to_string()))?;
        std::io::Write::write_all(&mut writer, manifest.as_bytes())?;
        writer
            .start_file("component.wasm", stored)
            .map_err(|err| Error::Invalid(err.to_string()))?;
        std::io::Write::write_all(&mut writer, component)?;
        writer
            .finish()
            .map_err(|err| Error::Invalid(err.to_string()))?;
    }
    Ok(cursor.into_inner())
}

/// Unpack an archive to the pair everything downstream expects
/// (ADR-0010: exactly these two files, verified digest downstream).
pub fn read_archive(bytes: &[u8]) -> Result<(String, Vec<u8>), Error> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|err| Error::Invalid(format!("not a zip archive: {err}")))?;
    // Names first (owned), so the per-file borrow of the archive ends
    // before the next iteration starts.
    let names: Vec<String> = (0..archive.len())
        .filter_map(|index| {
            archive
                .by_index(index)
                .ok()
                .map(|file| file.name().to_string())
        })
        .collect();
    let mut manifest = None;
    let mut component = None;
    for name in names {
        let mut file = archive
            .by_name(&name)
            .map_err(|err| Error::Invalid(err.to_string()))?;
        if name == "extension.toml" {
            let mut text = String::new();
            file.read_to_string(&mut text)?;
            manifest = Some(text);
        } else if name == "component.wasm" || name.ends_with(".wasm") {
            let mut buffer = Vec::new();
            file.read_to_end(&mut buffer)?;
            component = Some(buffer);
        }
        // Anything else does not belong (ADR-0010: nothing else added).
    }
    match (manifest, component) {
        (Some(manifest), Some(component)) => Ok((manifest, component)),
        _ => Err(Error::Invalid(
            "the archive must carry extension.toml and the component".to_string(),
        )),
    }
}

// ---------------------------------------------------------------------------
// Resolvers (FR-DIST-1/5/9): OCI, HTTPS archive, local path
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Capability comparison for the update prompt (FR-DIST-7)
// ---------------------------------------------------------------------------

fn capability_map(manifest: &toml::Value) -> BTreeMap<String, String> {
    manifest
        .get("capabilities")
        .and_then(|c| c.as_table())
        .map(|table| {
            table
                .iter()
                .map(|(name, value)| (name.clone(), canonical(value)))
                .collect()
        })
        .unwrap_or_default()
}

/// Whether updating to `new_manifest` would apply a capability the
/// approved set does not already cover exactly: every new declaration
/// must already be there, byte for byte (FR-DIST-7's prompt rule).
/// Narrowing or removing a grant never prompts; the consent shown at
/// install time stays what governs.
pub fn update_widens_grants(approved_manifest: &str, new_manifest: &str) -> Result<bool, Error> {
    let approved: toml::Value = approved_manifest
        .parse()
        .map_err(|err| Error::Invalid(format!("approved manifest does not parse: {err}")))?;
    let new: toml::Value = new_manifest
        .parse()
        .map_err(|err| Error::Invalid(format!("new manifest does not parse: {err}")))?;
    let old = capability_map(&approved);
    let incoming = capability_map(&new);
    Ok(incoming
        .iter()
        .any(|(name, value)| old.get(name) != Some(value)))
}

// ---------------------------------------------------------------------------
// HTTP: one client, the existing stack (the closed list's fallback for
// OCI: direct distribution calls over hyper)
// ---------------------------------------------------------------------------

type HttpsClient = hyper_util::client::legacy::Client<
    hyper_rustls::HttpsConnector<hyper_util::client::legacy::connect::HttpConnector>,
    http_body_util::Full<hyper::body::Bytes>,
>;

fn http_client() -> Result<HttpsClient, Error> {
    let https = hyper_rustls::HttpsConnectorBuilder::new()
        .with_webpki_roots()
        .https_or_http() // http carries local/dev registries and the tests' loopback servers;
        // production refs are https (SRDD: fetched over HTTPS)
        .enable_http1()
        .build();
    Ok(
        hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
            .build(https),
    )
}

async fn fetch_bytes(
    client: &HttpsClient,
    url: &str,
    accept: &str,
) -> Result<(Vec<u8>, hyper::HeaderMap), Error> {
    let request = hyper::Request::builder()
        .method("GET")
        .uri(url)
        .header(hyper::header::ACCEPT, accept)
        .body(http_body_util::Full::new(hyper::body::Bytes::new()))
        .map_err(|err| Error::Fetch(err.to_string()))?;
    let response = client
        .request(request)
        .await
        .map_err(|err| Error::Fetch(format!("{url}: {err}")))?;
    let status = response.status();
    let headers = response.headers().clone();
    if !status.is_success() {
        let hint = if status == hyper::StatusCode::UNAUTHORIZED {
            " (the registry wants authentication; anonymous public pulls are all1.0 supports)"
        } else {
            ""
        };
        return Err(Error::Fetch(format!("{url}: HTTP {status}{hint}")));
    }
    let body = http_body_util::BodyExt::collect(response.into_body())
        .await
        .map_err(|err| Error::Fetch(format!("{url}: {err}")))?
        .to_bytes()
        .to_vec();
    Ok((body, headers))
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
    let path = Path::new(source);
    if path.exists() {
        return resolve_local(path, local_manifest);
    }
    resolve_oci(source).await
}

/// FR-DIST-5: read both files from a local path (the manifest defaults
/// to `extension.toml` beside the component).
pub fn resolve_local(component: &Path, manifest_path: Option<&Path>) -> Result<Resolved, Error> {
    let manifest_path = match manifest_path {
        Some(path) => path.to_path_buf(),
        None => component
            .parent()
            .map(|dir| dir.join("extension.toml"))
            .ok_or_else(|| Error::Invalid("no manifest path".to_string()))?,
    };
    let manifest = std::fs::read_to_string(&manifest_path)?;
    let component_bytes = std::fs::read(component)?;
    Ok(Resolved {
        digest: Resolved::digest_of(&component_bytes),
        source: component.display().to_string(),
        manifest,
        component: component_bytes,
    })
}

/// FR-DIST-9: fetch the zip, unpack the two files; the digest is
/// computed here (an archive URL carries no external claim - the
/// lockfile records what this fetch produced, and `ext update` compares
/// against it next time).
pub async fn resolve_archive(url: &str) -> Result<Resolved, Error> {
    let client = http_client()?;
    let (bytes, _) = fetch_bytes(&client, url, "application/zip, application/octet-stream").await?;
    let (manifest, component) = read_archive(&bytes)?;
    Ok(Resolved {
        digest: Resolved::digest_of(&component),
        source: url.to_string(),
        manifest,
        component,
    })
}

/// Split `host/repo/name:tag` (the tag comes after the last colon that
/// sits past the last slash, so `host:5000/repo/name` parses right).
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
    let (host, name, tag) = split_oci_reference(reference)?;
    let tag = if tag.is_empty() { "latest" } else { tag };
    let client = http_client()?;
    let scheme = if host.starts_with("localhost") || host.starts_with("127.0.0.1") {
        "http"
    } else {
        "https"
    };
    let base = format!("{scheme}://{host}/v2/{name}");
    let (manifest_bytes, _headers) = fetch_bytes(
        &client,
        &format!("{base}/manifests/{tag}"),
        MANIFEST_MEDIA_TYPES,
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
    let (extension_toml, _) =
        match fetch_bytes(&client, &format!("{base}/blobs/{config_digest}"), "*/*").await {
            Ok(found) => found,
            Err(config_err) => {
                let layer = manifest_json
                    .get("layers")
                    .and_then(|l| l.as_array())
                    .and_then(|layers| layers.get(1))
                    .ok_or(config_err)?;
                let digest = blob_digest(layer)?;
                let found = fetch_bytes(&client, &format!("{base}/blobs/{digest}"), "*/*").await?;
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
    let (component, _) =
        fetch_bytes(&client, &format!("{base}/blobs/{layer_digest}"), "*/*").await?;
    verify_digest(&layer_digest, &component)?;

    Ok(Resolved {
        digest: Resolved::digest_of(&component),
        source: reference.to_string(),
        manifest: extension_toml,
        component,
    })
}
