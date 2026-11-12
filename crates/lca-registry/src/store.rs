//! The install tree (SRDD's extension-tree paragraph, FR-DIST-6): where the
//! component, the manifest, the approved-grant denials and the lockfile live, and
//! the only writer of them.
//!
//! The tree is content-addressed by digest (FR-DIST-8), so a load never re-resolves
//! a moving tag. `install` validates the manifest's name before it joins anything
//! onto the filesystem - that name is attacker-influenced.

use std::path::{Path, PathBuf};

use super::Error;
use super::Resolved;
use super::consent::{grant_hash, validate_extension_name};
use super::lockfile::{LockEntry, Lockfile};

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

    /// The tree's root directory.
    pub fn root(&self) -> &Path {
        &self.root
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
        // The name is joined onto the install tree, so it must satisfy the
        // loader's identifier rules here, before any write: a manifest is
        // attacker-influenced and `name = ".."` would otherwise escape the
        // tree (the loader's `Manifest::parse` is the authority; this is the
        // same rule, enforced at the write boundary too).
        validate_extension_name(&name)?;
        let entry = LockEntry {
            digest: resolved.digest.clone(),
            source: resolved.source.clone(),
            commit: resolved.commit.clone(),
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
        // ADR-0030: the package's `resources/` bag. Declared-vs-shipped
        // strictness like `worlds`: every resource's top-level kind must be
        // in the manifest's `resources`, checked before anything is written
        // so a refused install leaves no partial tree.
        let declared: Vec<String> = value
            .get("resources")
            .and_then(|v| v.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        for (rel, _) in &resolved.resources {
            let kind = rel.split('/').next().unwrap_or_default();
            if !declared.iter().any(|declared| declared == kind) {
                return Err(Error::Invalid(format!(
                    "resource `{rel}` has kind `{kind}`, which the manifest does not declare (add it to `resources = [...]`)"
                )));
            }
        }
        let dir = self.root.join(&name);
        std::fs::create_dir_all(&dir)?;
        std::fs::write(self.manifest_path(&name), &manifest)?;
        if !resolved.component.is_empty() {
            std::fs::write(
                self.component_path(&name, &resolved.digest),
                &resolved.component,
            )?;
        }
        // A reinstall replaces the bag wholesale, so a file the new
        // version dropped does not linger from the previous one.
        let resources_dir = dir.join("resources");
        let _ = std::fs::remove_dir_all(&resources_dir);
        for (rel, bytes) in &resolved.resources {
            let target = resources_dir.join(rel);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&target, bytes)?;
        }
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
