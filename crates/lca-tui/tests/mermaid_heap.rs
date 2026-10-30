//! Gh #325: rendering a diagram must not grow the heap.
//!
//! # Why this is its own test binary
//!
//! A `#[global_allocator]` is process-wide, so one installed here counts the
//! allocations of **every test in this binary**. Putting it in the crate's main
//! test module would make every other mermaid row's counts (and, worse, its
//! behaviour under `nextest`'s parallelism) depend on this file's counter. Rust
//! builds one binary per integration-test file, so this file exists to give the
//! counter a process of its own - that isolation is a correctness requirement,
//! not tidiness.
//!
//! # What is measured, and why not the shape of the code
//!
//! The defect was `String::leak` in `split_edge`: two allocations per diagram
//! edge, never freed, on every paint. A row that greps for `leak` would pass the
//! moment the leak moved somewhere else - it describes the fix instead of
//! measuring it. So this counts **live bytes** through a wrapping allocator and
//! asserts the figure stops growing: N renders of the same diagram, then 10N,
//! and the number between them must not scale with N.
//!
//! A live-bytes counter is the right instrument rather than an allocation *count*
//! because the bug is unbounded retention, not churn: a fixed per-render churn is
//! fine (and unavoidable - `Art` is returned by value), while any per-render
//! *retention* eventually consumes the machine.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicIsize, Ordering};

use lca_tui::widgets::mermaid;

/// Bytes currently allocated minus bytes freed, process-wide.
static LIVE_BYTES: AtomicIsize = AtomicIsize::new(0);

/// A pass-through allocator that keeps a running total of live bytes.
struct Counting;

// SAFETY: every method forwards to `System` unchanged and only adjusts a counter,
// so the allocator contract (alignment, ownership, no reentrancy) is `System`'s.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: `layout` came from a caller honouring `GlobalAlloc`.
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            LIVE_BYTES.fetch_add(layout.size() as isize, Ordering::Relaxed);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE_BYTES.fetch_sub(layout.size() as isize, Ordering::Relaxed);
        // SAFETY: `ptr` was returned by `alloc` for this same `layout`.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: `ptr` and `layout` come from a prior allocation of this
        // allocator; `new_size` is the caller's requested size.
        let new_ptr = unsafe { System.realloc(ptr, layout, new_size) };
        if !new_ptr.is_null() {
            LIVE_BYTES.fetch_add(
                new_size as isize - layout.size() as isize,
                Ordering::Relaxed,
            );
        }
        new_ptr
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// Live bytes right now.
fn live_bytes() -> isize {
    LIVE_BYTES.load(Ordering::SeqCst)
}

/// The diagram every row below renders: four nodes, three edges, so the old code
/// leaked six strings per render.
const DIAGRAM: &str = "flowchart TD\n  A[One] --> B[Two]\n  B --> C[Three]\n  C --> D[Four]\n";

// Verifies: gh #325 defect 1 - **no per-render retention inside the parser**.
//
// The old code leaked two `String`s per edge via `String::leak`, so every parse
// of a three-edge diagram retained ~96 bytes that could never be freed.
//
// **The source varies on every iteration, and that is the whole point.** The first
// version of this row rendered one fixed diagram N times and passed even with the
// leak restored, because the parse cache meant the parser ran exactly once - so it
// measured the cache, not the leak. Varying the source forces N real parses, which
// is what the leak was charging for. (Found by mutation-testing this row against a
// leak-restored build: with the cache in place it stayed green. A row that cannot
// fail on the defect is not a guard.)
//
// The assertion is on the **slope**, not an absolute figure: `Art` is returned by
// value and the bounded cache retains its last entries, so a fixed cost is
// expected. Only growth that scales with the parse count is the bug.
#[test]
fn parsing_many_diagrams_does_not_retain_memory_per_parse() {
    // The cache is filled and steady before the baseline, so its own retention
    // is not counted as growth.
    let parse_all = |from: usize, count: usize| -> isize {
        for index in from..from + count {
            // A distinct source each time: the cache cannot serve these, so the
            // parser runs and any per-parse leak is charged here.
            let source =
                format!("flowchart TD\n  A{index}[Node {index}] --> B{index}[End {index}]\n");
            let art = mermaid::render(&source).expect("renders");
            assert!(art.width > 0);
        }
        live_bytes()
    };
    let _ = parse_all(0, 100);
    let after_100 = parse_all(100, 100);
    let after_1100 = parse_all(200, 1000);

    let growth = after_1100 - after_100;
    assert!(
        growth.abs() < 4096,
        "live bytes grew by {growth} between 100 and 1100 parses \
         (after 100 {after_100}, after 1100 {after_1100}); \
         growth that scales with the parse count is the gh #325 leak"
    );
}

// Verifies: gh #325 - a **repeat** render is served from the cache: the parser
// runs once, and 1000 repaints add nothing to the heap.
//
// This is the other half of the issue, and it needs **both** assertions. Live
// bytes alone cannot prove it: with the leak fixed, a second parse of the same
// text allocates and frees the same bytes, so a cache-bypassed build is
// allocator-neutral and the byte count stays flat. Mutation-testing showed exactly
// that - bypassing the cache left this row green. The parse count is what
// distinguishes "served from cache" from "correctly re-parsed".
#[test]
fn a_repaint_of_the_same_diagram_does_not_reparse() {
    let _ = mermaid::render(DIAGRAM);
    let parses_before = mermaid::parse_count();
    let baseline = live_bytes();
    for _ in 0..1000 {
        let art = mermaid::render(DIAGRAM).expect("renders");
        assert!(art.width > 0);
    }
    assert_eq!(
        mermaid::parse_count(),
        parses_before,
        "1000 repaints of an unchanged diagram must not re-parse; the parsed form \
         is owned by the cache, not re-derived per frame"
    );
    let growth = live_bytes() - baseline;
    assert!(
        growth.abs() < 1024,
        "1000 repaints of one diagram grew the heap by {growth} bytes; a cached \
         repaint retains nothing"
    );
}

// Verifies: gh #579 - a repaint is still served from the cache **when the cache is
// full**, which is the case an eviction policy decides.
//
// `a_repaint_of_the_same_diagram_does_not_reparse` proves the under-filled case, where
// no policy can get it wrong. At capacity the question becomes "which entry goes", so
// this row fills the cache to its bound, keeps one diagram hot across many repaints
// while the cache stays full, and asserts the hot one never re-parses. A policy that
// evicted by insertion order would still pass (the hot key is the oldest insert), which
// is why this row and the survival row above are separate: this one pins "repaints are
// cached at all", that one pins "the right entry survives".
#[test]
fn a_repaint_at_capacity_does_not_reparse() {
    const HOT: &str = "flowchart LR\n  Full[At capacity] --> B[End]\n";
    // Fill exactly to the bound with unrelated diagrams, so every further insert must
    // evict something.
    let _ = mermaid::render(HOT);
    for index in 0..(mermaid::PARSE_CACHE_SIZE - 1) {
        let filler = format!("flowchart TD\n  F{index}[Filler {index}] --> G{index}[End]\n");
        let _ = mermaid::render(&filler);
    }
    let parses_before = mermaid::parse_count();
    for round in 0..200 {
        let art = mermaid::render(HOT).expect("renders");
        assert!(art.width > 0, "round {round}");
    }
    assert_eq!(
        mermaid::parse_count(),
        parses_before,
        "200 repaints of a diagram already in a full cache must not re-parse it"
    );
}

// Verifies: gh #325 - the cache is **bounded**, which is what makes "does not grow
// with the session" true.
//
// **This row deliberately does not consult the cache**, and that is the lesson from
// writing it three ways. Every version that asked "was my source evicted?" was
// order-dependent: the cache is process-wide, `nextest` interleaves tests, and a
// sibling row churns it between this row's insert and its check - so the row passed
// alone and failed in the suite, which is the worst kind of flake.
//
// It asserts the *invariant* the bound exists for instead: after rendering many more
// distinct diagrams than the bound, live bytes do not scale with the count. An
// unbounded cache fails this immediately - measured, along with
// `many_distinct_diagrams_are_bounded_by_the_cache`.
#[test]
fn the_cache_bound_keeps_retention_from_scaling_with_the_session() {
    // Past the bound once, to fill it and start evicting.
    for index in 0..64 {
        let source = format!("flowchart TD\n  A{index}[N{index}] --> B{index}[M{index}]\n");
        let _ = mermaid::render(&source);
    }
    let after_warmup = live_bytes();
    // Five times the warmup, in one uninterrupted run.
    for index in 64..384 {
        let source = format!("flowchart TD\n  A{index}[N{index}] --> B{index}[M{index}]\n");
        let _ = mermaid::render(&source);
    }
    let growth = live_bytes() - after_warmup;
    assert!(
        growth < 64 * 1024,
        "320 further distinct diagrams grew the heap by {growth} bytes; with a \
         bounded cache the retained set cannot scale with the session"
    );
}

// Verifies: gh #325 - the fix does not merely move the leak.
//
// A diagram that *fails* to parse is asked again on every streaming frame, so its
// `None` is cached too. Rendered 1000 times it must also stop growing.
#[test]
fn a_diagram_the_subset_cannot_draw_fails_soft_without_growing_the_heap() {
    const UNSUPPORTED: &str = "pie title Pets\n  \"Dogs\": 42\n";
    assert!(mermaid::render(UNSUPPORTED).is_none());
    let baseline = live_bytes();
    for _ in 0..1000 {
        assert!(mermaid::render(UNSUPPORTED).is_none());
    }
    let growth = live_bytes() - baseline;
    assert!(
        growth.abs() < 1024,
        "caching a refusal must not retain per call: grew {growth} bytes"
    );
}

// Verifies: gh #325 - distinct diagrams each cost their own parse, and the cache
// is bounded, so a session that shows many diagrams does not retain them all.
//
// The bound is `PARSE_CACHE_SIZE` entries; this renders well past it and asserts
// the heap stops climbing, which is what "bounded" means observably. A cache with
// no bound would grow without limit here - the same bug wearing a longer
// lifetime.
#[test]
fn many_distinct_diagrams_are_bounded_by_the_cache() {
    let render_all = |count: usize| -> isize {
        for index in 0..count {
            let source = format!("flowchart TD\n  A{index}[Node {index}] --> B[End]\n");
            let art = mermaid::render(&source).expect("renders");
            assert!(art.width > 0);
        }
        live_bytes()
    };
    // Past the bound once, then three times past it again: the second figure must
    // be no larger than the first beyond one cache's worth.
    let _ = render_all(50);
    let after_50 = render_all(50);
    let after_200 = render_all(150);
    let growth = after_200 - after_50;
    assert!(
        growth < 64 * 1024,
        "150 further distinct diagrams grew the heap by {growth} bytes; the parse \
         cache is bounded, so the retained set must not scale with the count"
    );
}

// Verifies: gh #579 - a **hot diagram survives unrelated inserts**, which is the row
// that fails under hash-order eviction.
//
// #325 gave this cache `cache.keys().next()`, and `HashMap` iteration order is hash
// order: the victim is arbitrary, so a diagram actively on screen can be evicted by
// an insert that has nothing to do with it and re-parsed on the next frame. Twenty
// entries is small enough that a transcript with a few diagrams plus scrolling
// reaches it, so "arbitrary" is reachable in ordinary use, not only in theory.
//
// The row re-uses the hot diagram every round - the way a repaint does - while feeding
// `PARSE_CACHE_SIZE * 3` distinct others. Under LRU the hot entry is touched each round
// and is the *last* thing to go; under hash order enough draws eventually take it.
#[test]
fn a_hot_diagram_survives_capacity_worth_of_unrelated_inserts() {
    const HOT: &str = "flowchart TD\n  Hot[On screen] --> B[Now]\n";
    let _ = mermaid::render(HOT);
    let parses_before = mermaid::parse_count();

    for index in 0..(mermaid::PARSE_CACHE_SIZE * 3) {
        // A repaint of the visible diagram: must be served from the cache.
        let hot = mermaid::render(HOT).expect("renders");
        assert!(hot.width > 0);
        // An unrelated diagram scrolled into view behind it.
        let other = format!("flowchart TD\n  O{index}[Other {index}] --> P{index}[End]\n");
        let _ = mermaid::render(&other);
    }

    // The hot entry was never evicted, so none of those 60 repaints re-parsed it.
    // Counting parses is stricter than checking the art: a re-parse returns the same
    // `Art`, so only the counter can tell "cached" from "recomputed".
    let parses_after = mermaid::parse_count();
    // 60 unrelated diagrams each parse once; the hot one must add nothing.
    let expected = mermaid::PARSE_CACHE_SIZE * 3;
    assert!(
        parses_after - parses_before <= expected,
        "the hot diagram was re-parsed {} time(s) beyond the {expected} unrelated \
         parses; hash-order eviction drops it arbitrarily",
        (parses_after - parses_before).saturating_sub(expected)
    );
}
