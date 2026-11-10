//! gh #589 — the harness's load allowance existed only on Linux.
//!
//! ## The defect
//!
//! `crates/lca-cli/tests/common/mod.rs`'s `load_factor()` read `/proc/loadavg`
//! and fell back to `0.0` when the file was absent — which it is on **macOS and
//! Windows**. So `docs/testing-plan.md` §14's promise ("multiplies that budget by
//! the machine's load average, 1x below light load, capped at 4x") was **true only
//! on the Linux dev box**, and false on exactly the two hosted runners whose
//! slowness the allowance was written for. Every readiness primitive gh #447
//! added — `settle`, `wait_until_painted`, `wait_for_layout_width_within`,
//! `wait_for_input_ready` — applies that same factor in eight call sites, so the
//! whole family was unscaled on the platforms that needed it.
//!
//! This is gh #489 round 3's shape with the instrument on the other side: a
//! harness rule that measures a developer's box correctly and measures the runner
//! as idle. It cost the `e2e_stale_selection` row a 41 s red on macOS against a
//! budget that had never been scaled at all.
//!
//! ## What this pins
//!
//! The *structure*, because the behaviour rows live in `harness_contract.rs`
//! (two tables over the pure classification functions). Here: the factor is
//! chosen by a function that takes the load average **as an `Option`**, so a
//! platform without one cannot silently collapse to the pre-fix answer; the probe
//! that answers instead is portable (`thread::sleep` + `Instant`, no `/proc`); and
//! the factor is sampled once per process rather than once per wait.
//!
//! Verifies: gh #589.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failure.

/// The harness **module**, not one filename (gh #574/#543's rule).
///
/// The first version of this guard read `common/mod.rs` alone, and the fix moved
/// the rule into `common/load.rs` for gate 11 — at which point the guard failed
/// loudly rather than going vacuous, which is the good direction, but it was still
/// reading the wrong thing. `module_text` walks `mod` declarations, so the module's
/// whole partition is one source to a guard; the call-site count below still reads
/// `readiness.rs` directly, because *that* file is the subject.
fn harness() -> String {
    crate::module_text("crates/lca-cli/tests/common/mod.rs")
}

/// The `fn` whose signature contains `signature`, from that line to the next
/// top-level `}`. Deliberately textual — this is a guard *about* the source.
fn body_of<'a>(source: &'a str, signature: &str) -> &'a str {
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("`{signature}` is still in the harness"));
    let rest = &source[start..];
    match rest.find("\n}\n") {
        Some(end) => &rest[..end],
        None => rest,
    }
}

// Verifies: gh #589 - the selection is a function of `Option<f64>`, so the
// platform with no load average is a **branch**, not a `unwrap_or(0.0)`.
//
// The defect was one line: `.unwrap_or(0.0)` turned "this platform publishes no
// load average" into "load is zero", which is indistinguishable from an idle box.
// An `Option` in the signature makes that impossible to express by accident, and
// `None` is handled where the rule lives (`factor_from`), not at the call site.
#[test]
fn the_factor_is_chosen_from_an_optional_measurement_not_a_default_of_zero() {
    let source = harness();
    let select = body_of(&source, "pub(crate) fn factor_from(load: Option<f64>");
    assert!(
        select.contains("Some(load) => factor_for_load(load)"),
        "a published load average decides, as it always did on Linux:\n{select}"
    );
    assert!(
        select.contains("None => factor_for_overshoot(overshoot)"),
        "and its absence falls through to the portable probe, not to 1x:\n{select}"
    );
    assert!(
        !source.contains("unwrap_or(0.0)"),
        "the `load average or zero` default is the defect itself: zero is an idle \
         box, and \"no such file\" is not an idle box (gh #589)"
    );
}

// Verifies: gh #589 - **the fallback signal is portable**, and it is a clock.
//
// `/proc/loadavg` is the Linux-only part. The probe must not read it, must not
// shell out, and must measure scheduling pressure with nothing but `thread::sleep`
// and `Instant`, so it works on macOS and Windows without a cfg table to maintain.
// It also samples several times and takes the **minimum**: a box is slow because
// its sleeps are consistently late, not because one was unlucky, and the harness
// must not widen a suite's budget on a single outlier.
#[test]
fn the_portable_signal_measures_overshoot_with_no_platform_file() {
    let source = harness();
    let probe = body_of(&source, "fn portable_overshoot() -> Duration");
    assert!(
        probe.contains("std::thread::sleep(PROBE_SLEEP)"),
        "the signal is a sleep, so it exists on every platform:\n{probe}"
    );
    assert!(
        probe.contains("elapsed()"),
        "and its overshoot is what is measured, not the sleep itself:\n{probe}"
    );
    assert!(
        probe.contains("best.min(") || probe.contains("min("),
        "the minimum of several samples, so one unlucky sleep cannot inflate a \
         suite's budget:\n{probe}"
    );
    assert!(
        !probe.contains("/proc/"),
        "the probe is the platform-independent half; it must not read a Linux file"
    );
    assert!(
        !probe.contains("Command::new"),
        "nor shell out (a harness that runs a process to measure load measures \
         its own child):\n{probe}"
    );
}

// Verifies: gh #589 - the probe runs **once per process**, because it is called
// per wait primitive and eight call sites would otherwise pay eight sleeps.
//
// The receipt for that is the `OnceLock`, and the counter beside it: the flag
// `LCA_LOAD_STATS=1` prints how many callers one probe served, which is the
// number a report can quote instead of asserting "it is cheap".
#[test]
fn the_probe_is_cached_once_per_process_and_can_be_reported() {
    let source = harness();
    // The cache lives in the module the rule moved to, so this row asks the
    // *module* for it rather than the wrapper that delegates (gh #574/#543).
    let factor = body_of(&source, "pub(super) fn cached_factor() -> u32");
    assert!(
        factor.contains("OnceLock") && factor.contains("get_or_init"),
        "the factor is a per-process decision, cached:\n{factor}"
    );
    assert!(
        body_of(&source, "pub(crate) fn load_factor() -> u32").contains("load::cached_factor()"),
        "and the harness's entry point delegates to it, so no second sampling path exists"
    );
    assert!(
        factor.contains("LCA_LOAD_STATS"),
        "and a run can print what it decided, the same way `LCA_SETTLE_STATS` \
         makes a settle's cost visible - a receipt has to come from the run"
    );
    assert!(
        source.contains("fn load_factor_calls()"),
        "the cache's effect is observable, so a report quotes a number"
    );
    // Every call site names the factor rather than re-deriving a budget, so a
    // platform cannot be left at 1x by one primitive growing its own rule.
    let readiness = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("crates/lca-cli/tests/common/readiness.rs");
    let readiness = std::fs::read_to_string(readiness).expect("readiness is readable");
    let sites = readiness.matches("Self::load_factor()").count();
    assert!(
        sites >= 6,
        "the readiness primitives all ride the one allowance (found {sites} call sites); \
         a primitive with its own budget rule is how macOS ended up unscaled"
    );
}
