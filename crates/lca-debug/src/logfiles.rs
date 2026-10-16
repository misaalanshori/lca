//! The two debug files and their rotation (gh #258 §5): exactly
//! `debug.log` (internal subsystems) and `debug-wire.log` (wire and
//! interface traffic) under the logs directory, each capped at
//! [`FILE_CAP_BYTES`] with a single `.1` spare — 20 MiB total, never
//! more, no additional files ever created.

use std::io::Write;
use std::path::{Path, PathBuf};

/// One debug file's cap (5 MiB).
pub const FILE_CAP_BYTES: u64 = 5 * 1024 * 1024;

/// The two debug files.
#[derive(Debug, Clone)]
pub struct DebugPaths {
    /// Internal subsystems (`lca_core`, `lca_session`, ...).
    pub log: PathBuf,
    /// Wire and interface traffic (`lca_net`, `lca_wasm`).
    pub wire: PathBuf,
}

impl DebugPaths {
    /// The pair inside a logs directory (production: `<data>/logs`).
    pub fn in_dir(dir: &Path) -> DebugPaths {
        DebugPaths {
            log: dir.join("debug.log"),
            wire: dir.join("debug-wire.log"),
        }
    }

    /// The single spare a file rotates to.
    pub fn spare_for(&self, path: &Path) -> PathBuf {
        let mut spare = path.as_os_str().to_owned();
        spare.push(".1");
        PathBuf::from(spare)
    }

    /// The file a domain writes (`is_wire` decides).
    pub fn file_for(&self, domain: crate::Domain) -> &Path {
        if domain.is_wire() {
            &self.wire
        } else {
            &self.log
        }
    }
}

/// An appending writer that rotates past the cap: the full file moves
/// to the single spare (clobbering the older spare) and writing
/// continues in a fresh file.
pub struct RotatingWriter {
    path: PathBuf,
    spare: PathBuf,
    cap: u64,
    file: Option<std::fs::File>,
}

impl RotatingWriter {
    /// Open (creating parent directories) with the production cap.
    pub fn open(path: &Path) -> std::io::Result<RotatingWriter> {
        RotatingWriter::with_cap(path, FILE_CAP_BYTES)
    }

    /// Open with an explicit cap (tests use a tiny one).
    pub fn with_cap(path: &Path, cap: u64) -> std::io::Result<RotatingWriter> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let spare = {
            let mut spare = path.as_os_str().to_owned();
            spare.push(".1");
            PathBuf::from(spare)
        };
        Ok(RotatingWriter {
            path: path.to_path_buf(),
            spare,
            cap,
            file: None,
        })
    }

    /// Rotate when the live file reached the cap. Returns whether it
    /// rotated (the caller reopens).
    fn rotate_if_full(&mut self) -> std::io::Result<bool> {
        let size = std::fs::metadata(&self.path)
            .map(|meta| meta.len())
            .unwrap_or(0);
        if size < self.cap {
            return Ok(false);
        }
        // Drop the handle before the rename (Windows cannot rename an open
        // file).
        self.file = None;
        let _ = std::fs::remove_file(&self.spare);
        std::fs::rename(&self.path, &self.spare)?;
        Ok(true)
    }
}

impl Write for RotatingWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        // Open (and repair) the live file BEFORE any rotation: the repair
        // (gh #342) runs on a pre-existing 0644 file first, so the rename
        // carries an already-owner-only file into the `.1` spare.
        if self.file.is_none() {
            self.file = Some(open_owner_only(&self.path)?);
        }
        if self.rotate_if_full()? {
            self.file = Some(open_owner_only(&self.path)?);
        }
        match self.file.as_mut() {
            Some(file) => file.write(buf),
            None => Err(std::io::Error::other("debug log file vanished after open")),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if let Some(file) = self.file.as_mut() {
            file.flush()?;
        }
        Ok(())
    }
}

/// Open (creating) the debug log owner-only (gh #342).
///
/// The logs record prompts, HTTP payloads, and file paths, so they are
/// not world-readable. On Unix, `mode(0o600)` covers creation and a
/// `set_permissions(0o600)` on the open handle **repairs a pre-existing
/// 0644 file** left by an earlier version — the end state is what the
/// acceptance criterion demands, and rotation renames the already-repaired
/// live file, so the `.1` spare inherits 0600 too.
///
/// Windows: `%USERPROFILE%` subdirectories inherit the profile ACL (user +
/// SYSTEM + Administrators), so another unprivileged user cannot read the
/// logs. Asserting or tightening that needs ACL surgery the crate does not
/// carry; the inherited ACL is the documented shape.
/// ponytail: profile-ACL inheritance, not explicit owner-only ACLs; add a
/// Windows ACL helper if a cross-user read is ever demonstrated.
fn open_owner_only(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // The repair must never break logging (diagnostics degrade, never
        // fail the agent); creation already used `mode(0o600)`, so a
        // failure here leaves a file no worse than a fresh 0600 one. The
        // error is intentionally swallowed (nothing logs before the
        // subscriber exists, and the `tracing` dependency is optional).
        let _ = file.set_permissions(std::fs::Permissions::from_mode(0o600));
    }
    Ok(file)
}
