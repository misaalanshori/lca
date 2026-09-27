//! Split from `capabilities.rs` (cycle 7, P3). Behaviour unchanged.

use super::*;

impl Capabilities {
    // ------------------------------------------------------------------
    // credentials (FR-PERM-6, FR-PERM-7, NFR-14)
    // ------------------------------------------------------------------

    fn credential_path(&self) -> Result<PathBuf, CapabilityError> {
        if !self.grants.credentials {
            return Err(self.refused(
                "credentials",
                &self.name,
                CapabilityError::NotGranted(
                    "the manifest does not declare the credentials capability".to_string(),
                ),
            ));
        }
        // The namespace IS the extension identity: never guest input
        // (FR-PERM-6), so a cross-namespace read has no address to take
        // (FR-PERM-7).
        Ok(self
            .roots
            .state_dir
            .join("credentials")
            .join(format!("{}.json", self.name)))
    }

    fn load_credentials(&self, path: &Path) -> Result<serde_json::Value, CapabilityError> {
        match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text)
                .map_err(|err| CapabilityError::Io(format!("credential store corrupt: {err}"))),
            // Absence means "no login yet", not an error.
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                Ok(serde_json::Value::Object(serde_json::Map::new()))
            }
            // Any other read failure must surface: silently treating it as
            // empty would let the next `set` overwrite stored credentials.
            Err(err) => Err(CapabilityError::Io(format!(
                "cannot read the credential store: {err}"
            ))),
        }
    }

    /// Write the namespace's credential file atomically with owner-only
    /// permissions (NFR-14). Set and delete share this path, so both get the
    /// same temp+rename and mode treatment (delete used to write in place,
    /// leaving a file the process created with the default umask).
    fn write_credentials(
        &self,
        path: &Path,
        data: &serde_json::Value,
    ) -> Result<(), CapabilityError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let bytes =
            serde_json::to_vec_pretty(data).map_err(|err| CapabilityError::Io(err.to_string()))?;
        let temp = path.with_extension("json.tmp");
        std::fs::write(&temp, bytes)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o600))?;
        }
        #[cfg(windows)]
        {
            // NFR-14 on Windows: the file would otherwise rely on the
            // user-profile directory's inherited ACL. Replace it with an
            // explicit owner-only DACL before the rename publishes the file.
            windows_acl::set_owner_only(&temp).map_err(CapabilityError::Io)?;
        }
        std::fs::rename(&temp, path)?;
        Ok(())
    }

    /// Read one key from this extension's own namespace. Returns `None`
    /// when absent; a denied or undeclared capability is a recorded
    /// permission error (FR-PERM-3), while "no login yet" is `None` so an
    /// extension can check without distinguishing denial from absence
    /// (capability catalog).
    pub fn credentials_get(&self, key: &str) -> Result<Option<String>, CapabilityError> {
        let path = self.credential_path()?;
        let data = self.load_credentials(&path)?;
        Ok(data.get(key).and_then(|v| v.as_str()).map(str::to_string))
    }

    /// Write one key into this extension's own namespace with
    /// owner-only file permissions (NFR-14).
    pub fn credentials_set(&self, key: &str, value: &str) -> Result<(), CapabilityError> {
        let path = self.credential_path()?;
        let mut data = self.load_credentials(&path)?;
        if !data.is_object() {
            data = serde_json::Value::Object(serde_json::Map::new());
        }
        data[key] = serde_json::Value::String(value.to_string());
        self.write_credentials(&path, &data)
    }

    /// Delete one key from this extension's own namespace.
    pub fn credentials_delete(&self, key: &str) -> Result<(), CapabilityError> {
        let path = self.credential_path()?;
        let mut data = self.load_credentials(&path)?;
        if let Some(map) = data.as_object_mut() {
            map.remove(key);
        }
        self.write_credentials(&path, &data)
    }

    // ------------------------------------------------------------------
    // resources (ADR-0030)
    // ------------------------------------------------------------------

    /// Set the extension's resource bag source (ADR-0032: the same seam
    /// serves an installed directory and a compiled-in table).
    pub fn set_resources(&mut self, source: ResourceSource) {
        self.resources = source;
    }

    /// The extension's resource bag source.
    pub fn resources(&self) -> &ResourceSource {
        &self.resources
    }

    /// List resource entries under `prefix`, relative to the extension's
    /// own resource root. Sorted for determinism (the conformance diff
    /// compares both delivery modes entry for entry).
    pub fn resource_list(&self, prefix: &str) -> Result<Vec<(String, u64)>, CapabilityError> {
        let prefix = resource_relative(prefix)?;
        match &self.resources {
            ResourceSource::None => Ok(Vec::new()),
            ResourceSource::Embedded(table) => {
                let mut entries: Vec<(String, u64)> = table
                    .iter()
                    .filter(|(path, _)| resource_under(path, &prefix))
                    .map(|(path, bytes)| ((*path).to_string(), bytes.len() as u64))
                    .collect();
                entries.sort();
                Ok(entries)
            }
            ResourceSource::Dir(root) => {
                // Canonicalize once and list against it: on macOS the
                // scratch path is a symlink (`/var` -> `/private/var`), so a
                // non-canonical root makes every `strip_prefix` fail and the
                // listing lose its relative paths.
                let canonical_root = std::fs::canonicalize(root)
                    .map_err(|_| CapabilityError::NotFound("no resources".into()))?;
                let base = if prefix.is_empty() {
                    canonical_root.clone()
                } else {
                    self.resource_dir(root, &prefix)?
                };
                let mut entries = Vec::new();
                collect_resources(&canonical_root, &base, &mut entries)?;
                entries.sort();
                Ok(entries)
            }
        }
    }

    /// Read one resource's bytes. A path outside the extension's own tree
    /// is a recorded permission error, never a cross-extension read
    /// (ADR-0030: identity-derived, never guest input).
    pub fn resource_read(&self, path: &str) -> Result<Vec<u8>, CapabilityError> {
        let rel = resource_relative(path).inspect_err(|err| {
            self.record("resources", path, &err.to_string());
        })?;
        match &self.resources {
            ResourceSource::None => Err(CapabilityError::NotFound(format!("no resource `{rel}`"))),
            ResourceSource::Embedded(table) => table
                .iter()
                .find(|(entry, _)| *entry == rel)
                .map(|(_, bytes)| bytes.to_vec())
                .ok_or_else(|| CapabilityError::NotFound(format!("no resource `{rel}`"))),
            ResourceSource::Dir(root) => {
                let file = self.resource_dir(root, &rel)?;
                let meta = std::fs::metadata(&file)
                    .map_err(|_| CapabilityError::NotFound(format!("no resource `{rel}`")))?;
                if !meta.is_file() {
                    return Err(CapabilityError::NotFound(format!("no resource `{rel}`")));
                }
                if meta.len() > RESOURCE_READ_MAX_BYTES {
                    return Err(CapabilityError::Invalid(format!(
                        "resource `{rel}` is {} bytes, over the {} byte read cap",
                        meta.len(),
                        RESOURCE_READ_MAX_BYTES
                    )));
                }
                Ok(std::fs::read(file)?)
            }
        }
    }

    // ------------------------------------------------------------------
    // state (ADR-0030)
    // ------------------------------------------------------------------

    /// This extension's state namespace directory
    /// (`<state_dir>/state/<name>`; identity-derived, never guest input).
    pub fn state_dir(&self) -> PathBuf {
        self.roots.state_dir.join("state").join(&self.name)
    }

    /// Read one key from this extension's own state namespace. Absence is
    /// `None`, mirroring `credentials_get`.
    pub fn state_read(&self, key: &str) -> Result<Option<Vec<u8>>, CapabilityError> {
        let path = self.state_path(key)?;
        match std::fs::read(&path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(CapabilityError::Io(err.to_string())),
        }
    }

    /// Write one key into this extension's own state namespace, bounded by
    /// the per-value and per-namespace caps.
    pub fn state_write(&self, key: &str, value: &[u8]) -> Result<(), CapabilityError> {
        if value.len() as u64 > STATE_VALUE_MAX_BYTES {
            return Err(CapabilityError::Invalid(format!(
                "state value `{key}` is {} bytes, over the {STATE_VALUE_MAX_BYTES} byte cap",
                value.len()
            )));
        }
        let path = self.state_path(key)?;
        let mut total = 0u64;
        if let Some(dir) = path.parent().filter(|dir| dir.is_dir()) {
            for entry in std::fs::read_dir(dir)? {
                let entry = entry?;
                if entry.file_name().to_string_lossy() != key {
                    total += entry.metadata()?.len();
                }
            }
        }
        if total + value.len() as u64 > STATE_TOTAL_MAX_BYTES {
            return Err(CapabilityError::Invalid(format!(
                "the state namespace would exceed the {STATE_TOTAL_MAX_BYTES} byte cap"
            )));
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, value)?;
        Ok(())
    }

    /// Delete one key from this extension's own state namespace.
    pub fn state_delete(&self, key: &str) -> Result<(), CapabilityError> {
        let path = self.state_path(key)?;
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(CapabilityError::Io(err.to_string())),
        }
    }

    /// Every key in this extension's state namespace, sorted.
    pub fn state_list(&self) -> Result<Vec<(String, u64)>, CapabilityError> {
        let dir = self.state_dir();
        if !dir.is_dir() {
            return Ok(Vec::new());
        }
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let meta = entry.metadata()?;
            if meta.is_file() {
                entries.push((entry.file_name().to_string_lossy().into_owned(), meta.len()));
            }
        }
        entries.sort();
        Ok(entries)
    }

    /// Resolve a state key to a file inside the extension's own namespace.
    /// Keys are a safe filename charset: a guest key can never carry a path
    /// (ADR-0030: identity namespace, never guest input).
    fn state_path(&self, key: &str) -> Result<PathBuf, CapabilityError> {
        let safe = !key.is_empty()
            && key.len() <= 200
            && key
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
        if !safe {
            let err = CapabilityError::Invalid(format!(
                "state key `{key}` must be 1-200 chars of letters, digits, `-`, `_`, or `.`"
            ));
            self.record("state", key, &err.to_string());
            return Err(err);
        }
        Ok(self.state_dir().join(key))
    }

    /// Resolve a relative resource path inside the bag root, refusing any
    /// escape through `..` or a symlink (the fs scope resolver's rule).
    fn resource_dir(&self, root: &Path, rel: &str) -> Result<PathBuf, CapabilityError> {
        let candidate = if rel.is_empty() {
            root.to_path_buf()
        } else {
            root.join(rel)
        };
        let canonical_root = std::fs::canonicalize(root)
            .map_err(|_| CapabilityError::NotFound("no resources".into()))?;
        let canonical = std::fs::canonicalize(&candidate)
            .map_err(|_| CapabilityError::NotFound(format!("no resource `{rel}`")))?;
        if !canonical.starts_with(&canonical_root) {
            let err = CapabilityError::Permission(format!(
                "resource `{rel}` leaves the extension's resource tree"
            ));
            self.record("resources", rel, &err.to_string());
            return Err(err);
        }
        Ok(canonical)
    }
}
