//! The `fs` capability: scoped reads, writes, listing, and stats
//! (capability catalog, ADR-0005). Split from `mod.rs` for the workspace
//! file ceiling (gate 11).

use super::*;

impl Capabilities {
    /// Read a file inside a granted scope.
    pub fn fs_read(&self, scope: &str, path: &str) -> Result<Vec<u8>, CapabilityError> {
        if !self.grants.fs_declared {
            return Err(self.undeclared("fs"));
        }
        let resolved = self.resolve(scope, path, false)?;
        Ok(std::fs::read(resolved)?)
    }

    /// Write a file inside a granted scope, creating its parent.
    pub fn fs_write(&self, scope: &str, path: &str, bytes: &[u8]) -> Result<(), CapabilityError> {
        if !self.grants.fs_declared {
            return Err(self.undeclared("fs"));
        }
        let resolved = self.resolve(scope, path, true)?;
        if let Some(parent) = resolved.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(resolved, bytes)?;
        Ok(())
    }

    /// Write a **new** file inside a granted scope, refusing to clobber
    /// (gh #344).
    ///
    /// [`Self::fs_write`] is `std::fs::write`: it truncates whatever is at
    /// the resolved path and follows a symlink there. That is right for a
    /// caller overwriting its own file and wrong for staging fresh content,
    /// where the resolved path could be a link planted inside the scope.
    /// This creates with `create_new` (a planted link is an error, never
    /// followed), owner-only `0600` on Unix, and non-executable.
    ///
    /// The scope check runs first and identically: this is the same
    /// `resolve` call, so the state-directory exclusion, the root
    /// confinement, and the deny-recording all apply.
    pub fn fs_write_new(
        &self,
        scope: &str,
        path: &str,
        bytes: &[u8],
    ) -> Result<(), CapabilityError> {
        if !self.grants.fs_declared {
            return Err(self.undeclared("fs"));
        }
        let resolved = self.resolve(scope, path, true)?;
        if let Some(parent) = resolved.parent() {
            std::fs::create_dir_all(parent)?;
            // Owner-only directory too (gh #344): the staged bytes are 0600,
            // but a group- or world-writable parent still lets a local user
            // create entries beside them (and the default umask gives 0775).
            // Tightening the directory this call created is the difference
            // between "the file is private" and "the staging area is private".
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
            }
        }
        // Owner-only on Unix; `create_new` refuses an existing entry of any
        // kind, including a symlink, so nothing planted is ever written
        // through.
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&resolved).map_err(|err| {
            self.record(
                "fs",
                &format!("{scope}:{path}"),
                &format!("cannot stage: {err}"),
            );
            CapabilityError::Io(err.to_string())
        })?;
        std::io::Write::write_all(&mut file, bytes).map_err(|err| {
            self.record(
                "fs",
                &format!("{scope}:{path}"),
                &format!("cannot stage: {err}"),
            );
            CapabilityError::Io(err.to_string())
        })?;
        Ok(())
    }

    /// A granted scope's own root directory, when the scope is granted.
    ///
    /// A scope resolves *relative* guest paths, so a caller that needs to
    /// hand a real path back to the model (a staged file, gh #344) has to
    /// know where the scope actually points. The root is returned only for
    /// a scope in the granted set, and it is the host's configured root -
    /// never guest input.
    pub fn scope_root(&self, scope: &str) -> Option<PathBuf> {
        if !self.grants.fs_declared {
            return None;
        }
        self.grants
            .fs
            .iter()
            .any(|grant| grant.scope == scope)
            .then(|| self.roots.root_for(scope).map(Path::to_path_buf))
            .flatten()
    }

    /// List a directory inside a granted scope; sorted for determinism.
    pub fn fs_list(&self, scope: &str, path: &str) -> Result<Vec<String>, CapabilityError> {
        if !self.grants.fs_declared {
            return Err(self.undeclared("fs"));
        }
        let resolved = self.resolve(scope, path, false)?;
        let mut names: Vec<String> = std::fs::read_dir(resolved)?
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        Ok(names)
    }

    /// Stat a path inside a granted scope.
    pub fn fs_stat(&self, scope: &str, path: &str) -> Result<(bool, u64), CapabilityError> {
        if !self.grants.fs_declared {
            return Err(self.undeclared("fs"));
        }
        let resolved = self.resolve(scope, path, false)?;
        let meta = std::fs::metadata(resolved)?;
        Ok((meta.is_dir(), meta.len()))
    }

    /// Resolve a granted scope's own directory: the working directory for
    /// spawned programs must be a scope the extension can already see
    /// (capability catalog).
    pub(super) fn scope_dir(&self, scope: &str) -> Result<PathBuf, CapabilityError> {
        if !self.grants.fs_declared {
            return Err(self.undeclared("fs"));
        }
        self.resolve(scope, ".", false)
    }
}
