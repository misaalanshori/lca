use std::io::Write;

use super::{Capabilities, CapabilityError};

impl Capabilities {
    /// Create a new file inside a granted scope without replacing an existing
    /// resolved path, including a symlink target. Returns its resolved path.
    pub fn fs_write_new(
        &self,
        scope: &str,
        path: &str,
        bytes: &[u8],
    ) -> Result<std::path::PathBuf, CapabilityError> {
        if !self.grants.fs_declared {
            return Err(self.undeclared("fs"));
        }
        let resolved = self.resolve(scope, path, true)?;
        if let Some(parent) = resolved.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&resolved)?;
        file.write_all(bytes)?;
        Ok(resolved)
    }
}
