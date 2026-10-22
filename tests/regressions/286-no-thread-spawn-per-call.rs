//! gh #286 - the synchronous call path does not spawn a thread per call.
//!
//! `WasmExtension::blocking` used to be `std::thread::spawn(..).join()`, so every
//! `render`/`on_event` - i.e. every TUI frame - and every stream delta created and
//! reaped a thread. The measured cost was ~689 us/call on this box; with a
//! per-extension driver thread it is ~273 us/call (medians of 500-call runs, three
//! runs each way). The guard here is structural, not a stopwatch: a wall-clock
//! assertion would re-create the load sensitivity this repo has spent cycles
//! removing.
//!
//! The behaviour rows live in `crates/lca-ext-host/tests/host.rs`: the spawn count,
//! the mixed-call and two-extension variants, cancellation through the driver,
//! trap isolation, concurrent callers, and the driver shutting down on drop. This
//! is the NFR-24 pin that the mechanism cannot be quietly replaced.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

fn read(path: &str) -> String {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(root.join(path)).unwrap_or_else(|err| panic!("read {path}: {err}"))
}

// Verifies: gh #286 - the sync path hands work to a long-lived driver rather than
// minting a thread, and the driver is per extension.
#[test]
fn the_sync_path_uses_a_long_lived_driver() {
    let host = read("crates/lca-ext-host/src/lib.rs");
    // The per-call spawn is gone from the production path. `std::thread::spawn`
    // may still appear in tests and in `drive_blocking`'s callers; what must not
    // appear is the spawn-and-join inside `blocking`.
    assert!(
        host.contains("self.inner.driver.call(&self.inner, work)"),
        "`WasmExtension::blocking` delegates to the driver"
    );
    assert!(
        !host.contains("std::thread::spawn(move || work(&inner))"),
        "the per-call spawn in `blocking` is back"
    );

    let driver = read("crates/lca-ext-host/src/driver.rs");
    // One thread per extension, created once, receiving jobs over a channel.
    assert!(
        driver.contains("pub(super) struct Driver"),
        "the driver type exists"
    );
    assert!(driver.contains("fn driver_loop("), "and its receive loop");
    // A `Weak`, not a strong `Arc`: with a strong one the driver keeps its own
    // extension -- and so its own instance cache -- alive forever. This is not
    // theoretical; it is what `dropping_an_extension_shuts_its_driver_down`
    // caught in the first version of this change.
    assert!(
        driver.contains("Arc::downgrade(inner)") && driver.contains("Weak<Inner>"),
        "the driver holds its extension weakly, or it never shuts down"
    );
    // Epoch interruption must not be defeated: nothing in the driver may touch
    // the engine's epoch, which is what reaches a guest on any thread.
    assert!(
        !driver.contains("increment_epoch") && !driver.contains("set_epoch_deadline"),
        "the driver must not touch epoch accounting; cancellation depends on it \
         reaching the guest wherever it runs"
    );
}

// Verifies: gh #286 - the guard is a count, and the count is a real one.
//
// The issue's manager note is explicit that a wall-clock assertion is the wrong
// guard; this row pins that the counter the behaviour rows read actually exists
// and moves.
#[test]
fn the_spawn_count_is_a_real_counter() {
    let driver = read("crates/lca-ext-host/src/driver.rs");
    assert!(
        driver.contains("pub static DRIVER_SPAWNS: AtomicU64"),
        "a spawn counter exists"
    );
    assert!(
        driver.contains("DRIVER_SPAWNS.fetch_add(1"),
        "and it is incremented where the thread is started"
    );
    assert!(
        driver.contains("pub static DRIVER_EXITS: AtomicU64"),
        "an exit counter exists too, so shutdown is checkable without /proc"
    );
    assert!(
        driver.contains("DRIVER_EXITS.fetch_add(1"),
        "and it is incremented when the receive loop ends"
    );

    let tests = read("crates/lca-ext-host/tests/host.rs");
    for row in [
        "fn n_synchronous_calls_spawn_a_bounded_number_of_threads",
        "fn mixed_call_kinds_reuse_one_driver",
        "fn two_extensions_use_one_driver_each",
        "fn an_epoch_bump_traps_a_guest_on_the_driver",
        "fn a_trap_on_the_driver_still_evicts_the_cache",
        "fn two_threads_calling_the_same_extension_do_not_deadlock",
        "fn dropping_an_extension_shuts_its_driver_down",
    ] {
        assert!(
            tests.contains(row),
            "the row `{row}` is present; these are the four properties the brief \
             requires a concurrency refactor to keep (spawn bound, cancellation, \
             trap isolation, no leak)"
        );
    }
}

// Verifies: gh #286 - the driver is serial by construction, and says so.
//
// A guest instance is not re-entrant (its store is `&mut` for the duration of a
// call), so two concurrent calls must not run at once. The pre-#286 code got this
// from `checkout` on the cache; the driver gets it from running one job at a
// time. Either way the property is "serialised", and a driver that multiplexed
// would be a correctness change, not an optimisation.
#[test]
fn the_driver_serialises_and_documents_it() {
    let driver = read("crates/lca-ext-host/src/driver.rs");
    assert!(
        driver.contains("while let Ok(job) = rx.recv()"),
        "the driver takes one job at a time off one channel"
    );
    assert!(
        driver.contains("not re-entrant"),
        "and the module says why serial is required rather than incidental"
    );
    // The deadlock argument is stated where a future maintainer will look.
    assert!(
        driver.contains("No re-entry") && driver.contains("cannot deadlock"),
        "the module documents why this cannot deadlock, which is the brief's \
         design point 2"
    );
}
