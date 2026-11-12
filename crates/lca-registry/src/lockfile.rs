//! The lockfile: the format version, one installed extension's record, and the
//! document at the top of the install tree (FR-DIST-6/7/8, ADR-0010).
//!
//! `load` is the reader `ext list`/`ext update` start from; `save` is the atomic
//! writer (temp file, then rename - the session log's durability rule, applied to
//! this store). A lockfile from a newer host is refused, not silently truncated.

use std::collections::BTreeMap;
use std::path::Path;

use super::Error;

/// The lockfile format version.
pub const LOCKFILE_VERSION: u32 = 1;

/// One installed extension's lockfile record (SRDD's extension-tree
/// paragraph: digest, source reference, approved-capability hash).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LockEntry {
    /// `sha256:<hex>` of the component bytes; every later load uses it
    /// and never re-resolves a moving tag (FR-DIST-8).
    pub digest: String,
    /// What it was resolved from: an OCI reference, an HTTPS URL, a
    /// local path, or a `git+...` source (ADR-0010: same machinery,
    /// different string).
    pub source: String,
    /// The pinned commit for git sources (gh #59); empty otherwise.
    /// Old lockfiles load with empty (never pinned by commit).
    #[serde(default)]
    pub commit: String,
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
