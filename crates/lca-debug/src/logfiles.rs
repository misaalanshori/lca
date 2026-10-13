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

    /// Rotate when the live file reached the cap.
    fn rotate_if_full(&mut self) -> std::io::Result<()> {
        let size = std::fs::metadata(&self.path)
            .map(|meta| meta.len())
            .unwrap_or(0);
        if size < self.cap {
            return Ok(());
        }
        self.file = None;
        let _ = std::fs::remove_file(&self.spare);
        std::fs::rename(&self.path, &self.spare)?;
        Ok(())
    }
}

impl Write for RotatingWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.rotate_if_full()?;
        if self.file.is_none() {
            self.file = Some(
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&self.path)?,
            );
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
