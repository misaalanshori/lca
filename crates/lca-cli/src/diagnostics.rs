//! Process diagnostics (#152): a bounded rolling file log, plus stderr on
//! `--verbose`. Installed once in `main` before dispatch, because every
//! `tracing::warn!/error!` in the workspace evaporates without a
//! subscriber.
//!
//! Level selection follows the `tracing-subscriber` `EnvFilter`
//! convention, not a new scheme: `LCA_LOG`, then `RUST_LOG`, then `warn`
//! by default (`debug` under `--verbose`). The file log is bounded by
//! startup rotation: an `lca.log` over the size cap moves to a single
//! `lca.log.1` spare, so the directory never holds more than two files
//! from this writer.

use std::path::{Path, PathBuf};

/// The file log's size cap: rotation keeps one spare, so diagnostics own
/// at most twice this under the log directory.
pub const LOG_FILE_MAX_BYTES: u64 = 1024 * 1024;

/// The log directory for a data directory.
pub fn log_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("logs")
}

/// Move an oversized log aside to the single spare, dropping any older
/// spare. A diagnostics log that grows forever is its own defect; a
/// rotation that keeps history forever is the same defect with steps.
pub fn rotate_log_if_oversized(path: &Path) {
    let oversized = std::fs::metadata(path)
        .map(|meta| meta.len() > LOG_FILE_MAX_BYTES)
        .unwrap_or(false);
    if !oversized {
        return;
    }
    let spare = path.with_extension("log.1");
    let _ = std::fs::remove_file(&spare);
    let _ = std::fs::rename(path, &spare);
}

/// Register the crash file's context (gh #81): version and data
/// dir now (the earliest point anything can panic), extension names
/// later at every registry assembly. The hook writes the file; this
/// only stages what it writes.
pub fn init_crash_context() {
    lca_tui::set_crash_context(lca_tui::CrashContext {
        version: crate::version_static().to_string(),
        data_dir: crate::data_dir(),
        extensions: Vec::new(),
    });
}

/// Install the subscriber for the real data directory.
/// `Ok(true)` installed it; `Ok(false)` found one already (idempotent —
/// tests and re-entry never panic); `Err` names a setup failure the
/// caller reports without failing startup (diagnostics must never take
/// the agent down with them).
pub fn init_diagnostics(verbose: bool) -> Result<bool, String> {
    init_diagnostics_with_dir(verbose, &crate::data_dir())
}

/// Install the subscriber with an explicit data directory (the seam the
/// guard drives; production passes the real one).
pub fn init_diagnostics_with_dir(verbose: bool, data_dir: &Path) -> Result<bool, String> {
    use tracing_subscriber::{EnvFilter, prelude::*};

    let dir = log_dir(data_dir);
    std::fs::create_dir_all(&dir)
        .map_err(|err| format!("cannot create log directory {}: {err}", dir.display()))?;
    let path = dir.join("lca.log");
    rotate_log_if_oversized(&path);
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|err| format!("cannot open log file {}: {err}", path.display()))?;

    let directives = std::env::var("LCA_LOG")
        .or_else(|_| std::env::var("RUST_LOG"))
        .unwrap_or_else(|_| (if verbose { "debug" } else { "warn" }).to_string());
    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(std::sync::Mutex::new(file))
        .with_ansi(false)
        .with_filter(EnvFilter::new(directives.clone()));
    let subscriber = tracing_subscriber::registry().with(file_layer);
    if verbose {
        let stderr_layer = tracing_subscriber::fmt::layer()
            .with_writer(std::io::stderr)
            .with_filter(EnvFilter::new(directives));
        match subscriber.with(stderr_layer).try_init() {
            Ok(()) => Ok(true),
            Err(_) => Ok(false),
        }
    } else {
        match subscriber.try_init() {
            Ok(()) => Ok(true),
            Err(_) => Ok(false),
        }
    }
}
