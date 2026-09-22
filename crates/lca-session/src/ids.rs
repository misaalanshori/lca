//! Sortable identifiers: timestamp prefix first, so a directory listing is
//! chronological without reading any file (`docs/session-log-format.md`).

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// Milliseconds since the Unix epoch.
pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Cheap process-local entropy without a `rand` dependency: hashes the
/// clock, the pid, and a counter through the standard hasher's random seed.
fn entropy() -> u64 {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u64(now_ms());
    hasher.write_u32(std::process::id());
    hasher.write_u32(counter);
    hasher.finish()
}

/// A session identifier: sortable by creation time.
pub(crate) fn session_id(now: u64) -> String {
    format!("{now:013}-{:08x}", entropy() as u32)
}

/// A record identifier: unique within a session, sortable by creation order
/// even inside one millisecond (the counter breaks ties).
pub(crate) fn record_id(now: u64) -> String {
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{now:013}-{counter:08x}-{:04x}", entropy() as u16)
}

/// The on-disk project key: the working copy's directory name for human
/// readability plus a hash of the canonical path.
pub(crate) fn project_key(canonical_path: &str) -> String {
    let dir_name = std::path::Path::new(canonical_path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "project".to_string());
    // FNV-1a: no dependency needed for a display prefix; the canonical path
    // does the identifying work.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in canonical_path.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{dir_name}-{:06x}", (hash >> 16) as u32 & 0xff_ffff)
}
