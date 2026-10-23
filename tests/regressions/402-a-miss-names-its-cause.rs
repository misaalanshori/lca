//! gh #402 - a cache miss says who caused it.
//!
//! # The gap this closes
//!
//! ADR-0017 measures *that* the prompt cache missed and *how much* was re-billed,
//! and the footer shows the ratio. The transcript said nothing, and the one signal
//! that did exist was wire vocabulary: core records
//! `cache-boundary-narrowed: stable region diverged at message 7 (boundary 9 -> 7)`
//! and `chat_turn_events.rs` toasts it verbatim. A user watching `CH%` fall could
//! not learn that a skill matched, that their instructions changed, or that they
//! switched models.
//!
//! # The rule, and the four things that make it honest
//!
//! 1. **Correlate, do not re-derive.** `cache_cause::attribute` reads
//!    `CacheMiss`'s own fields (`idle_ms`, `model_changed`) and the log's
//!    `cache-boundary-narrowed` event plus the `[skill instructions]` /
//!    `[instructions changed]` markers. It never infers a cause from token
//!    arithmetic - that would be the "second copy that disagrees" ADR-0017 warns
//!    about.
//! 2. **Once per cause.** `BTreeSet<Cause>` replaces the `last: usize` counter,
//!    because a counter cannot tell "same divergence again" from "a different
//!    cause".
//! 3. **Counted-by-design is not a defect.** FR-CACHE-2 makes a model switch
//!    deliberate; `Cause::is_defect` gates two wordings so the notice does not cry
//!    wolf on the intended case.
//! 4. **An unknown cause says so.** An invented cause is undebuggable.
//!
//! The behavioural rows live beside the code
//! (`crates/lca-cli/src/tui/display/cache_cause/tests.rs`, 12 rows, plus the
//! once-per-cause and footer-agreement rows in `display.rs`). This file pins the
//! *shape* so a later refactor cannot quietly reintroduce a second derivation.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failure.

fn read(path: &str) -> String {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(root.join(path)).unwrap_or_else(|err| panic!("read {path}: {err}"))
}

/// Code only: doc/line comments removed, so prose naming an old shape cannot trip
/// a guard (these files discuss the very mistake they fixed).
fn code_only(text: &str) -> String {
    text.lines()
        .filter(|line| {
            let t = line.trim_start();
            !t.starts_with("//")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// Verifies: gh #402 - **attribution comes from the measurement.** The notice reads
// `collect_cache_misses` and the recorded signals; it must not compute a miss or a
// cause of its own.
#[test]
fn the_notice_does_not_derive_its_own_cause() {
    let display = code_only(&read("crates/lca-cli/src/tui/display.rs"));
    let seam = display
        .find("fn miss_notice_for(")
        .expect("the notice producer exists");
    let body = &display[seam..];
    let end = body.find("\n}\n").unwrap_or(body.len());
    let body = &body[..end];
    assert!(
        body.contains("collect_cache_misses"),
        "the notice reads the measurement rather than scanning records itself"
    );
    assert!(
        body.contains("cache_cause::attribute"),
        "and asks the one attribution function"
    );
    for invented in ["compute_cache_waste", "detect_miss", "missed_tokens +="] {
        assert!(
            !body.contains(invented),
            "the notice must not re-measure ({invented}): {body}"
        );
    }
}

// Verifies: gh #402 design point 1 - **one place decides the cause.** A second
// `match` over records looking for the markers would drift from the first.
#[test]
fn exactly_one_function_decides_the_cause() {
    let cause = code_only(&read("crates/lca-cli/src/tui/cache_cause/mod.rs"));
    assert_eq!(
        cause.matches("fn attribute(").count(),
        1,
        "one attribution function"
    );
    // The markers are read in one place.
    assert_eq!(
        cause.matches("[skill instructions]").count(),
        1,
        "the skill marker is matched once"
    );
    assert_eq!(
        cause.matches("[instructions changed]").count(),
        1,
        "the instruction marker is matched once"
    );
    // And nothing else in the crate looks for them.
    for other in [
        "crates/lca-cli/src/tui/display.rs",
        "crates/lca-cli/src/tui/hooks.rs",
    ] {
        let text = code_only(&read(other));
        assert!(
            !text.contains("[skill instructions]") && !text.contains("[instructions changed]"),
            "{other} must not re-read the markers"
        );
    }
}

// Verifies: gh #402 design point 2 - **once per cause, structurally.** The state is
// a set of causes, not a count of misses, and the hook owns exactly one.
#[test]
fn the_hook_holds_a_cause_set_not_a_counter() {
    let display = code_only(&read("crates/lca-cli/src/tui/display.rs"));
    let seam = display
        .find("fn cache_miss_notice_hook(")
        .expect("the hook builder exists");
    let body = &display[seam..];
    let end = body.find("\n}\n").unwrap_or(body.len());
    let body = &body[..end];
    assert!(body.contains("BTreeSet::new()"), "the state is a cause set");
    assert!(
        !body.contains("0usize"),
        "not the old miss counter, which cannot tell one cause from another"
    );
    // The signature carries the set, so a caller cannot pass a count.
    let sig = display
        .find("fn miss_notice_for(")
        .expect("the notice producer exists");
    let sig_end = display[sig..].find(')').map(|off| sig + off).unwrap_or(sig);
    assert!(
        display[sig..sig_end].contains("BTreeSet<cache_cause::Cause>"),
        "the parameter is a cause set"
    );
}

// Verifies: gh #402 design point 3 / FR-CACHE-2 - **a counted-by-design cause is
// not worded as a fault**, and the distinction is a property of the type rather
// than a condition duplicated at each call site.
#[test]
fn a_switch_is_excluded_from_the_defect_wording() {
    let cause = code_only(&read("crates/lca-cli/src/tui/cache_cause/mod.rs"));
    let seam = cause
        .find("fn is_defect(")
        .expect("the defect predicate exists");
    let body = &cause[seam..];
    let end = body.find("\n    }\n").unwrap_or(body.len());
    let body = &body[..end];
    assert!(
        body.contains("BoundaryDiverged") && body.contains("Unknown"),
        "only a divergence and an unknown count as defects"
    );
    for counted in ["ModelSwitched", "IdleExpired", "SkillMatched"] {
        assert!(
            !body.contains(counted),
            "{counted} is a normal consequence, not a fault: {body}"
        );
    }
    // And the text builder branches on the predicate, not on a raw match.
    let text = cause
        .find("fn notice_text(")
        .expect("the text builder exists");
    let tail = &cause[text..];
    let text_end = tail.find("\n}\n").unwrap_or(tail.len());
    assert!(
        tail[..text_end].contains("is_defect()"),
        "the two wordings are gated by the one predicate"
    );
}

// Verifies: gh #402 design point 5 - **an unattributable miss is named, not
// guessed.** The fallback is an explicit variant rather than a default that looks
// like an answer.
#[test]
fn an_unknown_cause_is_a_named_variant() {
    let cause = code_only(&read("crates/lca-cli/src/tui/cache_cause/mod.rs"));
    assert!(cause.contains("Unknown,"), "the variant exists");
    assert!(cause.contains("cause unknown"), "and has honest wording");
    // The attribution ends on it rather than on a plausible-looking default.
    let seam = cause.find("fn attribute(").expect("the attribution exists");
    let body = &cause[seam..];
    let end = body.find("\n}\n").unwrap_or(body.len());
    assert!(
        body[..end].trim_end().ends_with("Cause::Unknown"),
        "the function's last answer is `Unknown`, not a guess: {body}"
    );
}
