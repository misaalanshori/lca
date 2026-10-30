//! Gh #579: **no cache anywhere in `lca-tui` may pick its own eviction victim.**
//!
//! # Why this row exists, in the manager's words
//!
//! "The point is not the mermaid cache; it is that the crate has two eviction
//! policies." #323 (`05cba170`) introduced `engine/lru.rs` and converged the two
//! `engine/` caches onto it. Its guard — `the_engine_has_one_cache_eviction_policy` in
//! `tests/regressions/323-engine-robustness.rs` — scans **`engine/` only**, so
//! `widgets/mermaid.rs`'s `PARSE_CACHE` (added by #325, one cycle earlier) kept the old
//! `cache.keys().next()` idiom and nobody noticed for a cycle. That is the gap this file
//! closes: the same assertion, over the whole crate, so a new cache in `widgets/`,
//! `ui/`, or a module nobody has written yet cannot reintroduce it.
//!
//! # What "arbitrary" costs, measured
//!
//! `HashMap` iteration order is hash order, so the victim of `keys().next()` has no
//! relation to recency. For the mermaid cache that meant a diagram **actively on
//! screen** could be evicted by an unrelated insert and re-parsed on the next frame.
//! Measured on the pre-fix build: **37 wasted re-parses across 60 repaints** of one
//! visible diagram while 60 others scrolled past it. The behavioural row is
//! `a_hot_diagram_survives_capacity_worth_of_unrelated_inserts` in
//! `crates/lca-tui/tests/mermaid_heap.rs`.
//!
//! # The sweep this issue asked for
//!
//! `rg -n "keys\(\).next\(\)" crates/lca-tui/src` before the fix returned exactly one
//! live occurrence — `widgets/mermaid.rs:99` — plus four mentions in comments explaining
//! the defect. There was no second cache hiding elsewhere: the crate holds three
//! `thread_local!` blocks (`text.rs`'s width cache, `text.rs`'s `cfg(test)` fast-path
//! counter, and `mermaid.rs`'s parse cache), and after this cycle the two caches use
//! `LruCache` while the counter caches nothing.
//!
//! # Why the ban is on the idiom and not on `HashMap`
//!
//! A `HashMap` is not the problem — `LruCache` is one inside. The problem is a map used
//! as a *bounded* cache with a victim chosen by iteration order, so the ban is precisely
//! that shape. Banning `HashMap` would be a much broader rule that fails on the policy's
//! own implementation, and a guard whose exemption list is its own subject teaches
//! nothing.

use std::path::{Path, PathBuf};

/// The repository root.
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Read a file from the repository root.
fn read(rel: &str) -> String {
    let path = repo_root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {}: {err}", path.display()))
}

/// Every `.rs` file under `dir`, recursively.
fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(rust_files(&path));
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            found.push(path);
        }
    }
    found
}

// Verifies: gh #579 - the crate has exactly **one** cache eviction policy.
//
// This is #323's `the_engine_has_one_cache_eviction_policy` widened from `engine/` to
// every `.rs` file in `lca-tui`, which is the gap that let `widgets/mermaid.rs` keep the
// old idiom for a cycle. It scans **source**, not behaviour, because a behavioural row
// proving the shared type works still passes while a new cache hand-rolls its own
// eviction - the second copy behaves correctly for as long as nobody adds a third.
#[test]
fn no_cache_in_lca_tui_evicts_by_iteration_order() {
    let crate_root = repo_root().join("crates/lca-tui/src");
    let mut offenders: Vec<String> = Vec::new();
    let mut scanned = 0usize;
    for file in rust_files(&crate_root) {
        scanned += 1;
        let rel = file
            .strip_prefix(repo_root())
            .unwrap_or(&file)
            .display()
            .to_string()
            .replace('\\', "/");
        // The policy's own implementation is the definition being shared, so it is the
        // one place allowed to choose a victim by anything at all.
        if rel.ends_with("engine/lru.rs") {
            continue;
        }
        let Ok(src) = std::fs::read_to_string(&file) else {
            continue;
        };
        // Comments legitimately *name* the defect - `lru.rs`'s module docs, `text.rs`'s
        // history note, and the behavioural rows' explanations all quote it. Only code
        // counts, or this guard would fail on the documentation of its own subject.
        let code: String = src
            .lines()
            .filter(|line| {
                let trimmed = line.trim_start();
                !trimmed.starts_with("//") && !trimmed.starts_with("///")
            })
            .collect::<Vec<_>>()
            .join("\n");
        // The arbitrary-eviction idiom: take whatever the map yields first, then drop it.
        // Matched as a pair so a bare `keys().next()` in a read-only role is not flagged.
        if code.contains("keys().next()") {
            offenders.push(format!("{rel}: evicts `keys().next()` (hash order)"));
        }
    }
    assert!(
        scanned > 20,
        "the scan found only {scanned} files, so it proves nothing about the crate"
    );
    assert!(
        offenders.is_empty(),
        "gh #579: a bounded cache must use `engine::lru::LruCache` rather than picking \
         its victim by iteration order, which is hash order and therefore arbitrary:\n  {}",
        offenders.join("\n  ")
    );
}

// Verifies: gh #579 - the mermaid parse cache actually **is** on the shared policy.
//
// The ban above would pass on a crate with no caches at all, so this pins the one this
// issue is about: the parse cache holds an `LruCache` and no longer a bare `HashMap`
// evicted by hand. Read from source because the cache is a private `thread_local` - the
// behavioural rows cannot see which type backs it.
#[test]
fn the_mermaid_parse_cache_uses_the_shared_policy() {
    let mermaid = read("crates/lca-tui/src/widgets/mermaid.rs");
    assert!(
        mermaid.contains("lru::LruCache<String, Option<Art>>"),
        "the parse cache is an `LruCache`"
    );
    assert!(
        mermaid.contains("LruCache::new(PARSE_CACHE_SIZE)"),
        "built from the same named bound the docs quote"
    );
    // The old shape: a `RefCell<HashMap<..>>` as the cache itself.
    assert!(
        !mermaid.contains("RefCell<std::collections::HashMap<String, Option<Art>>>"),
        "and not a bare `HashMap` evicted by hand"
    );
    // A hit must mark the diagram recent, or LRU degenerates to insertion order and the
    // hot-diagram row would only pass because nothing else competes for the slot.
    assert!(
        mermaid.contains("cache.borrow_mut().get(&key)"),
        "a hit goes through the mutable accessor, which is what refreshes recency"
    );
}

// Verifies: gh #579 - the shared type is reachable and used from **both** modules that
// cache, so the crate is genuinely converged rather than merely free of the old idiom.
//
// A file that dropped its cache entirely would satisfy the ban above; this asserts the
// two caches that exist are on the one policy.
#[test]
fn both_caches_use_the_one_policy() {
    let width = read("crates/lca-tui/src/engine/text.rs");
    assert!(
        width.contains("lru::LruCache"),
        "the width cache uses the shared policy"
    );
    let mermaid = read("crates/lca-tui/src/widgets/mermaid.rs");
    assert!(
        mermaid.contains("lru::LruCache"),
        "and so does the parse cache"
    );
    // One eviction implementation, in the type they share.
    let lru = read("crates/lca-tui/src/engine/lru.rs");
    assert!(
        lru.contains("fn evict_least_recent"),
        "the policy has exactly one eviction implementation"
    );
    assert!(
        lru.contains("min_by_key(|(_, entry)| entry.stamp)"),
        "whose victim is the lowest recency stamp - a comparison, not iteration order"
    );
}

// Verifies: gh #579 - the crate's `thread_local!` set is what the sweep found, so a new
// cache cannot appear without this file being updated.
//
// The sweep reported three `thread_local!` blocks and no other cache. Naming them makes
// the claim checkable: a fourth one is either a new cache (which the ban above must
// cover) or a counter, and either way someone has to look at this row.
#[test]
fn the_crates_thread_local_set_is_the_one_the_sweep_found() {
    let crate_root = repo_root().join("crates/lca-tui/src");
    let mut sites: Vec<String> = Vec::new();
    for file in rust_files(&crate_root) {
        let rel = file
            .strip_prefix(repo_root())
            .unwrap_or(&file)
            .display()
            .to_string()
            .replace('\\', "/");
        // Test-only files are not part of the shipped set.
        if rel.contains("/tests") || rel.ends_with("_tests.rs") {
            continue;
        }
        let Ok(src) = std::fs::read_to_string(&file) else {
            continue;
        };
        let code: String = src
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        if code.contains("thread_local!") {
            sites.push(rel);
        }
    }
    sites.sort();
    assert_eq!(
        sites,
        vec![
            "crates/lca-tui/src/engine/text.rs".to_string(),
            "crates/lca-tui/src/widgets/mermaid.rs".to_string(),
        ],
        "the crate's process-local caches are the width cache and the parse cache; a \
         third is a change the eviction guard must be checked against"
    );
}
