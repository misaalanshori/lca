//! The footer's git branch cache (gh #389 gap 2, the expensive half).
//!
//! `Footer::render` runs on **every** repaint, and a repaint happens on every
//! streaming delta — hundreds per turn. Reading `.git/HEAD` there made the
//! frame cost scale with filesystem latency, on the UI thread, for a value that
//! changes when a human types `git checkout`. This cache is the fix: reads
//! happen on an explicit cadence, the frame path only reads a field.
//!
//! # The cadence, and why
//!
//! [`REFRESH`] is 2 seconds. A branch changes on `git checkout`, not per
//! keystroke, so being up to two seconds stale costs nothing; blocking a frame
//! costs a visible stutter, and a stutter on a streaming turn is exactly the
//! "this tool feels flaky" impression the footer's anti-jitter work exists to
//! avoid. A `reftable` repository also spawns `git` on refresh, and a slower
//! cadence keeps that spawn rare enough to be free.
//!
//! It is driven by the tick that already exists (`Chat::tick`, gh #369) rather
//! than a thread: one owner, no synchronization, and no wakeup while the
//! interface is idle (the tick only runs from the loop).
//!
//! # Stale, never blank
//!
//! When the cadence expires and the refresh has not run yet, the last known
//! value stays on screen. A footer that flickers between `main` and nothing is
//! worse than one that is briefly stale.

use std::path::Path;
use std::time::{Duration, Instant};

use super::git::{self, SymbolicRef};

/// How often the branch is re-read. See the module docs.
pub const REFRESH: Duration = Duration::from_secs(2);

/// A branch label plus when it was last confirmed.
///
/// `None` for the label is a real answer ("not in a work tree") and is cached
/// the same way: leaving a repository must not cost a read per frame either.
#[derive(Debug, Clone, Default)]
pub struct GitCache {
    /// The last known label.
    branch: Option<String>,
    /// When [`GitCache::branch`] was last read from disk.
    read_at: Option<Instant>,
    /// The directory the label describes.
    dir: String,
    /// Reads performed, for the per-frame bound the tests pin.
    reads: u64,
}

impl GitCache {
    /// A cache with no value yet: the first [`GitCache::refresh_if_stale`] on
    /// `dir` reads.
    #[must_use]
    pub fn new(dir: &str) -> Self {
        Self {
            dir: dir.to_string(),
            ..Self::default()
        }
    }

    /// The label to display. **Never reads**: this is the frame path.
    #[must_use]
    pub fn branch(&self) -> Option<&str> {
        self.branch.as_deref()
    }

    /// How many filesystem reads this cache has performed. The bound the tests
    /// assert is stated in terms of this.
    #[must_use]
    pub fn reads(&self) -> u64 {
        self.reads
    }

    /// Whether a refresh is due at `now`.
    #[must_use]
    pub fn is_stale(&self, now: Instant) -> bool {
        self.read_at
            .is_none_or(|read_at| now.duration_since(read_at) >= REFRESH)
    }

    /// Re-read the branch if the cadence has expired, reporting whether
    /// anything changed (the loop repaints on change only).
    ///
    /// A change of directory always refreshes: the workspace can be switched
    /// under a running interface, and a label from the old tree would be a
    /// lie rather than staleness.
    pub fn refresh_if_stale(
        &mut self,
        dir: &str,
        now: Instant,
        resolve: Option<&SymbolicRef>,
    ) -> bool {
        let moved = dir != self.dir;
        if !moved && !self.is_stale(now) {
            return false;
        }
        self.dir = dir.to_string();
        self.reads += 1;
        // `None` means "use the production resolver", not "no answer": a
        // `reftable` repository has no branch without it, and the caller that
        // wires the cache into the loop should not have to know that.
        let resolve = resolve.cloned().unwrap_or_else(super::default_symbolic_ref);
        let branch = git::resolve_branch(Path::new(dir), Some(&resolve));
        self.read_at = Some(now);
        if branch == self.branch && !moved {
            return false;
        }
        self.branch = branch;
        true
    }

    /// Force a read regardless of the cadence. For the first frame, so the
    /// footer is correct on startup rather than blank for up to [`REFRESH`].
    pub fn prime(&mut self, dir: &str, now: Instant, resolve: Option<&SymbolicRef>) -> bool {
        self.read_at = None;
        self.refresh_if_stale(dir, now, resolve)
    }
}

#[cfg(test)]
#[path = "cache_tests.rs"]
mod tests;
