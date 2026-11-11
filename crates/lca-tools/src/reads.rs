//! The read-freshness registry: which files the session has read, the
//! fingerprint each had, and the gate an extension write must pass
//! (FR-TOOL-2).
//!
//! Split out of `lib.rs` for gate 11's 1,200-line ceiling (gh #570). The
//! functions keep their paths - `lib.rs` re-exports them - because the CLI,
//! the SDK and the hooks all name them by `lca_tools::…`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::fingerprint;

/// The session's read records, shared between the built-in tools and
/// extension-backed ones (gh #436).
///
/// FR-TOOL-2's guarantee is a *recent read of this file*. The built-in `read`
/// fills this, the built-in `edit` and `write` consult it, and an extension
/// that writes through `fs` must consult the **same** record - a second
/// tracker inside each extension would recreate exactly the drift this fixes,
/// with two places deciding what "recently read" means.
///
/// Scope is the process, which is one session: `lca` runs a session per
/// process, and a fresh process starts with no reads, so nothing survives a
/// boundary. Tests call [`self::reset_read_registry`] to isolate themselves.
static READ_REGISTRY: std::sync::OnceLock<std::sync::Mutex<HashMap<PathBuf, u64>>> =
    std::sync::OnceLock::new();

fn read_registry() -> &'static std::sync::Mutex<HashMap<PathBuf, u64>> {
    READ_REGISTRY.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

/// Record that this session read `path` and saw these bytes.
///
/// Called by every path that presents a file's contents to the model, so the
/// guarantee covers reads from any source (the built-in `read`, a `write`'s
/// own result, or an extension).
pub fn note_read(path: &Path, bytes: &[u8]) {
    let mut registry = read_registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    registry.insert(registry_key(path), fingerprint(bytes));
}

/// The key a path is filed under.
///
/// Canonicalized so the *same file* reached by two spellings is one entry: the
/// built-in tools record the path they resolved, `fs_write` records the path
/// its scope resolved, and on macOS `/var` is a symlink to `/private/var` while
/// Windows mixes `\\?\` verbatim prefixes with plain paths. Falls back to the
/// path as given when it cannot be canonicalized (a file about to be created).
fn registry_key(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Whether the read-before-edit gate is on for this process (gh #117's
/// `tool.edit_requires_read`).
///
/// Set once by the host when it applies config, next to
/// [`ToolExecutor::set_edit_requires_read`]. Process-scoped because a `lca`
/// process is one session, and read as a plain `bool` so extension-backed
/// writers do not need a constructor change to consult the same rule
/// (gh #436).
static EDIT_REQUIRES_READ: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Record the operator's `tool.edit_requires_read` choice for this process.
pub fn set_edit_requires_read(required: bool) {
    EDIT_REQUIRES_READ.store(required, std::sync::atomic::Ordering::Relaxed);
}

/// The current gate value.
pub fn edit_requires_read() -> bool {
    EDIT_REQUIRES_READ.load(std::sync::atomic::Ordering::Relaxed)
}

/// Whether `path` was read this session *and* still hashes to what was read.
///
/// `false` when it was never read, or when the bytes changed since - the two
/// cases FR-TOOL-2 refuses.
pub fn has_fresh_read(path: &Path, current: &[u8]) -> bool {
    let registry = read_registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    registry
        .get(&registry_key(path))
        .is_some_and(|known| *known == fingerprint(current))
}

/// The refusal both editors give when the gate is on and the read is not
/// fresh (gh #436). One string, so a user cannot meet two messages for one
/// rule.
pub fn stale_read_refusal(path: &str) -> String {
    format!(
        "The file {path} changed since it was last read (or was never read this session). \
         Read it again before editing."
    )
}

/// Forget every recorded read (tests, and a session reset).
pub fn reset_read_registry() {
    let mut registry = read_registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    registry.clear();
}
