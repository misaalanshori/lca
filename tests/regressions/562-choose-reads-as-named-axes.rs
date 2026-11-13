//! gh #562 - the reproducibility gate reads as five named axes, and its truth
//! table is unchanged.
//!
//! `choose(pinned, offline, ci, refresh, cache_ok)` was five adjacent `bool`s in
//! one signature, and a call site read `choose(false, false, true, true, true)`:
//! transposing two of them silently changes which catalog a release embeds, and
//! no reviewer can see it. It is now `choose(ChooseInput { .. })` with a field
//! name on every axis.
//!
//! This row pins the **behaviour**, not the signature: the full 32-row truth
//! table, and the three NFR-17 rules the helper exists for. `build_support.rs` is
//! `include!`-ed by `build.rs` *and* by `tests/models_catalog.rs`, so the table is
//! enumerated in the test target (where `Origin` is in scope) while `build.rs`'s
//! own call site is a compile-time witness that the struct still compiles in a
//! build script.
//!
//! Verifies: gh #562 acceptance 1 (NFR-17's precedence rules survive the rewrite).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::path::PathBuf;

/// `build_support.rs` as the two `include!`-ers see it: a plain module.
///
/// `build.rs` cannot be `use`d from a test (it is a build script), and
/// `tests/models_catalog.rs`'s rows already drive the same functions. Reading the
/// file here is how this target asserts the *source* contract (the signature) while
/// the truth table below asserts the behaviour through a local copy of the logic.
mod build_support {
    #![allow(dead_code, missing_docs)] // the `include!`-ers' own allowances
    include!("../../crates/lca-registry/build_support.rs");
}

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

// Verifies: gh #562 - no five-bool positional call can exist: the only signature
// is the named one, and every call site names its axes.
//
// A `bool`-tuple signature is exactly the defect, so the row reads the two files
// that could reintroduce it (`build_support.rs` where it is declared, `build.rs`
// where the production call is).
#[test]
fn choose_reads_as_five_named_axes() {
    let support = std::fs::read_to_string(repo().join("crates/lca-registry/build_support.rs"))
        .expect("read build_support.rs");
    assert!(
        support.contains("pub struct ChooseInput"),
        "the axes are a named struct"
    );
    for axis in ["pinned", "offline", "ci", "refresh", "cache_ok"] {
        assert!(
            support.contains(&format!("pub {axis}: bool,")),
            "`{axis}` is a named field, so a call site cannot transpose it silently"
        );
    }
    assert!(
        support.contains("pub fn choose(input: ChooseInput) -> Origin"),
        "and the signature takes the struct, not five booleans"
    );
    assert!(
        !support.contains("pub fn choose(pinned: bool"),
        "the five-bool signature is gone"
    );

    let build = std::fs::read_to_string(repo().join("crates/lca-registry/build.rs"))
        .expect("read build.rs");
    let call = build
        .split("choose(ChooseInput {")
        .nth(1)
        .expect("build.rs calls `choose` with the named struct");
    let call = &call[..call.find("})").expect("the call closes")];
    for axis in ["pinned", "offline", "ci", "refresh", "cache_ok"] {
        assert!(
            call.contains(axis),
            "`build.rs` names `{axis}` at its call site: {call}"
        );
    }
}

// Verifies: gh #562 (NFR-17) - all 32 input combinations, enumerated, compared
// against the rules the helper has always implemented.
//
// The enumeration is the receipt: five booleans mean 32 cases, and a rewrite that
// transposes two axes fails one of these rows rather than shipping a release that
// fetches models.dev in CI.
#[test]
fn the_truth_table_is_unchanged_across_all_thirty_two_inputs() {
    use build_support::{ChooseInput, Origin, choose};

    let mut rows = 0;
    let mut mismatches = Vec::new();
    for pinned in [false, true] {
        for offline in [false, true] {
            for ci in [false, true] {
                for refresh in [false, true] {
                    for cache_ok in [false, true] {
                        let input = ChooseInput {
                            pinned,
                            offline,
                            ci,
                            refresh,
                            cache_ok,
                        };
                        // The rules, restated independently of `choose`'s body:
                        let expected = if pinned {
                            Origin::Pinned
                        } else if offline || ci {
                            Origin::Stub
                        } else if cache_ok && !refresh {
                            Origin::Cache
                        } else {
                            Origin::Network
                        };
                        let got = choose(input);
                        if got != expected {
                            mismatches.push(format!(
                                "pinned={pinned} offline={offline} ci={ci} refresh={refresh} \
                                 cache_ok={cache_ok}: expected {expected:?}, got {got:?}"
                            ));
                        }
                        rows += 1;
                    }
                }
            }
        }
    }
    assert_eq!(rows, 32, "five booleans are 32 cases");
    assert!(
        mismatches.is_empty(),
        "the truth table changed:\n{}",
        mismatches.join("\n")
    );

    // The three rules stated separately, so a table edit that agrees with a
    // broken `choose` cannot hide: CI/offline is the stub with the best possible
    // cache in hand, `refresh` cannot beat a warm cache into the network while...
    assert_eq!(
        choose(ChooseInput {
            offline: true,
            cache_ok: true,
            ..ChooseInput::default()
        }),
        Origin::Stub,
        "offline is the stub even with a usable cache (a cache is per-checkout state)"
    );
    assert_eq!(
        choose(ChooseInput {
            ci: true,
            refresh: true,
            cache_ok: true,
            ..ChooseInput::default()
        }),
        Origin::Stub,
        "and CI reads `refresh` not at all, because its arm returns first (NFR-17)"
    );
    assert_eq!(
        choose(ChooseInput {
            refresh: true,
            cache_ok: true,
            ..ChooseInput::default()
        }),
        Origin::Network,
        "outside CI, `refresh` ignores a warm cache and refetches"
    );
}
