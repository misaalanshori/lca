//! The `ultrathink` magic keyword (gh #198): a standalone lowercase
//! `ultrathink` in prompt prose boosts reasoning effort to max for
//! that turn only, leaving the session default untouched.
//!
//! Verifies: prose triggers, casing matters (lowercase only), word
//! boundaries hold (`xultrathink` and `ultrathinks` do not), fenced
//! code blocks and inline code never trigger, and an unclosed fence
//! fails quiet (a missed boost beats a cache-busting false boost).

use ultrathink::should_boost;

// Verifies: the word in prose triggers, in all its positions.
#[test]
fn prose_triggers_in_any_position() {
    assert!(should_boost("ultrathink: how should I redesign this?"));
    assert!(should_boost("How should I redesign this? ultrathink"));
    assert!(should_boost(
        "Think carefully — ultrathink! — about the borrow checker."
    ));
    assert!(should_boost("ultrathink"));
}

// Verifies: only the standalone lowercase word counts.
#[test]
fn casing_and_boundaries_hold() {
    assert!(!should_boost("ULTRATHINK this please"));
    assert!(!should_boost("Ultrathink this please"));
    assert!(!should_boost("xultrathink"));
    assert!(!should_boost("ultrathinks hard"));
    assert!(!should_boost("the ultra_think mode"));
    assert!(should_boost("ultrathink-mode LLC")); // hyphen is a boundary
    assert!(should_boost("(ultrathink)"));
}

// Verifies: fenced code blocks never trigger, fenced or not,
// whatever language tag rides the fence.
#[test]
fn fenced_code_never_triggers() {
    assert!(!should_boost("```\nultrathink\n```"));
    assert!(!should_boost("```rust\nlet x = \"ultrathink\";\n```"));
    assert!(!should_boost(
        "Some prose.\n```\nultrathink\n```\nMore prose."
    ));
    assert!(should_boost(
        "Some prose with ultrathink.\n```\ncode without the word\n```"
    ));
}

// Verifies: inline backtick spans never trigger.
#[test]
fn inline_code_never_triggers() {
    assert!(!should_boost("Run `ultrathink` to boost."));
    assert!(!should_boost("Compare `a` and `ultrathink` here."));
    assert!(should_boost("Compare `a` and b, ultrathink, here."));
}

// Verifies: an unclosed fence fails quiet — the rest reads as code,
// so a stray backtick costs a boost, never a false one.
#[test]
fn an_unclosed_fence_fails_quiet() {
    assert!(!should_boost("```\nultrathink"));
    assert!(!should_boost("Oops `ultrathink"));
}

// Verifies: empty and wordless prompts do nothing.
#[test]
fn empty_prompts_do_nothing() {
    assert!(!should_boost(""));
    assert!(!should_boost("   "));
    assert!(!should_boost("Please refactor this."));
}

// Verifies: the manifest test contract — hooks world with the
// status-line region declared for the indicator half.
#[test]
fn manifest_grants_match_the_manifest() {
    let manifest = ultrathink::MANIFEST;
    assert!(manifest.contains("worlds = [\"hooks\"]"), "{manifest}");
    assert!(
        manifest.contains("regions = [\"status-line\"]"),
        "{manifest}"
    );
    assert!(!manifest.contains("[capabilities.fs]"), "{manifest}");
    assert!(!manifest.contains("[capabilities.net]"), "{manifest}");
}
