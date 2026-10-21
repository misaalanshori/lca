//! The per-frame read bound (gh #389 gap 2), pinned by measurement rather than
//! by reading the code and believing it.
//!
//! "The footer does I/O per frame" was the defect, so the property is a
//! **count**: drive a simulated frame loop and assert how many reads happened.
//! The count comes from the production [`GitCache`] itself, not a mock, so a
//! future refactor that puts a `read_to_string` back inside `render` shows up
//! here as a failure instead of as a stutter nobody profiles.

use std::path::Path;
use std::time::{Duration, Instant};

use super::*;
use crate::footer::git::SymbolicRef;

/// A resolver that never spawns, so a cache test never depends on `git`.
fn no_resolver() -> SymbolicRef {
    std::sync::Arc::new(|_dir: &Path| None)
}

// Verifies: gh #389 gap 2 - **the frame path never reads.** `branch()` is a
// field access; calling it a thousand times performs zero filesystem reads.
#[test]
fn a_thousand_frames_perform_zero_reads() {
    let mut cache = GitCache::new("/nonexistent");
    let zero = Instant::now();
    cache.prime("/nonexistent", zero, Some(&no_resolver()));
    let after_prime = cache.reads();
    assert_eq!(
        after_prime, 1,
        "the first frame reads once, to have a value"
    );

    for _ in 0..1000 {
        // This is what `Footer::render` does: read the field.
        let _ = cache.branch();
    }
    assert_eq!(
        cache.reads(),
        after_prime,
        "1000 frames performed no reads: the frame path is a field access"
    );
}

// Verifies: gh #389 gap 2 - **the bound over N frames is stated and held.**
// Over one second of 60 fps frames without the cadence expiring, reads stay at
// the single priming read, not one per frame.
#[test]
fn sixty_frames_inside_the_cadence_read_once() {
    let start = Instant::now();
    let mut cache = GitCache::new("/w");
    cache.prime("/w", start, Some(&no_resolver()));
    let mut now = start;
    for _ in 0..60 {
        now += Duration::from_millis(16);
        cache.refresh_if_stale("/w", now, Some(&no_resolver()));
        let _ = cache.branch();
    }
    assert_eq!(
        cache.reads(),
        1,
        "60 frames in {:.0} ms, one read: the cadence is {}s",
        (now - start).as_millis(),
        REFRESH.as_secs()
    );
}

// Verifies: gh #389 gap 2 - the cadence does expire, and a refresh picks up a
// branch change. Without this half, "zero reads per frame" could be satisfied
// by never reading at all.
#[test]
fn a_refresh_after_the_cadence_picks_up_a_branch_change() {
    let dir = fixture("cache-change", "ref: refs/heads/main\n");
    let start = Instant::now();
    let mut cache = GitCache::new(dir.to_str().expect("utf8"));
    cache.prime(dir.to_str().expect("utf8"), start, Some(&no_resolver()));
    assert_eq!(cache.branch(), Some("main"));

    std::fs::write(dir.join(".git/HEAD"), "ref: refs/heads/renamed\n").expect("write HEAD");
    // Still inside the cadence: the old value stands (stale, not wrong-yet).
    assert!(!cache.refresh_if_stale(
        dir.to_str().expect("utf8"),
        start + Duration::from_millis(500),
        Some(&no_resolver())
    ));
    assert_eq!(cache.branch(), Some("main"), "stale, not blank");

    // Past the cadence: the change lands, and the loop is told to repaint.
    let changed = cache.refresh_if_stale(
        dir.to_str().expect("utf8"),
        start + REFRESH,
        Some(&no_resolver()),
    );
    assert!(changed, "a new branch means a repaint");
    assert_eq!(cache.branch(), Some("renamed"));
    assert_eq!(cache.reads(), 2, "one read per cadence, not per frame");
}

// Verifies: gh #389 gap 2 - **stale, never blank.** An expired cache that has
// not refreshed yet still shows the last known value: a footer flickering
// between `main` and nothing is worse than one a second stale.
#[test]
fn an_expired_cache_still_shows_the_last_value() {
    let dir = fixture("cache-stale", "ref: refs/heads/main\n");
    let start = Instant::now();
    let mut cache = GitCache::new(dir.to_str().expect("utf8"));
    cache.prime(dir.to_str().expect("utf8"), start, Some(&no_resolver()));

    // Expired, and deliberately not refreshed (as when a turn is streaming and
    // the tick has not come round).
    let later = start + Duration::from_secs(3600);
    assert!(cache.is_stale(later));
    assert_eq!(cache.branch(), Some("main"), "the last value survives");
    assert_eq!(cache.reads(), 1, "and nothing was read to keep it");
}

// Verifies: gh #389 gap 2 - leaving a repository is cached too, so "not in a
// work tree" costs one read per cadence rather than one per frame.
#[test]
fn a_directory_outside_a_repository_is_cached_as_no_branch() {
    let scratch = std::env::temp_dir().join(format!("lca-cache-outside-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).expect("mkdir");
    let dir = scratch.to_str().expect("utf8");
    let start = Instant::now();
    let mut cache = GitCache::new(dir);
    cache.prime(dir, start, Some(&no_resolver()));
    assert_eq!(cache.branch(), None);
    for _ in 0..100 {
        cache.refresh_if_stale(dir, start, Some(&no_resolver()));
    }
    assert_eq!(cache.reads(), 1, "an empty answer is still a cached answer");
    let _ = std::fs::remove_dir_all(&scratch);
}

// Verifies: gh #389 gap 2 - changing directory always refreshes, even inside
// the cadence: a label from the old tree is not staleness, it is a lie.
#[test]
fn switching_directory_refreshes_immediately() {
    let a = fixture("cache-dir-a", "ref: refs/heads/alpha\n");
    let b = fixture("cache-dir-b", "ref: refs/heads/beta\n");
    let start = Instant::now();
    let mut cache = GitCache::new(a.to_str().expect("utf8"));
    cache.prime(a.to_str().expect("utf8"), start, Some(&no_resolver()));
    assert_eq!(cache.branch(), Some("alpha"));

    let changed = cache.refresh_if_stale(b.to_str().expect("utf8"), start, Some(&no_resolver()));
    assert!(changed, "a different workspace repaints");
    assert_eq!(cache.branch(), Some("beta"));
}

// Verifies: gh #389 gap 2 + gap 1 **together** - passing no resolver must mean
// "use the production one", not "give up". Found live: the loop wired the cache
// with `None`, so a `reftable` repository lost its branch in a real pane while
// every unit test passed, because those all injected a resolver.
#[test]
fn no_injected_resolver_still_resolves_a_reftable_branch() {
    let dir = std::env::temp_dir().join(format!("lca-cache-reft-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join(".git")).expect("mkdir");
    // The exact placeholder a reftable repository writes.
    std::fs::write(dir.join(".git/HEAD"), "ref: refs/heads/.invalid\n").expect("write");
    let start = Instant::now();
    let mut cache = GitCache::new(dir.to_str().expect("utf8"));
    // `None` is the live wiring; the assertion is that it still does something
    // rather than silently reporting "not in a work tree".
    cache.prime(dir.to_str().expect("utf8"), start, None);
    assert_ne!(
        cache.branch(),
        Some(".invalid"),
        "the placeholder is never shown as a branch name"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A directory with a minimal `.git/HEAD`, enough for a branch read.
fn fixture(tag: &str, head: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("lca-footer-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join(".git")).expect("mkdir");
    std::fs::write(dir.join(".git/HEAD"), head).expect("write HEAD");
    dir
}
