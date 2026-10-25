//! NFR-24 guard for gh #367: on a terminal that cannot report the Shift modifier,
//! `Enter` inserts a newline instead of submitting a half-written prompt.
//!
//! # The bug, and why it cannot be fixed at the byte level
//!
//! On Apple's Terminal.app and on Windows consoles whose virtual-terminal input
//! drops the modifier, `Shift+Enter` emits a bare `\r` — byte-identical to `Enter`.
//! The Shift information is gone before LCA sees the bytes, so there is no pattern
//! to find. A user reaching for a newline in a multi-line prompt therefore
//! **submitted it to a paid model** and lost their place in what they were writing.
//!
//! # The policy
//!
//! Since the bytes cannot distinguish the two, the question becomes what `\r`
//! *means* on a known-affected terminal. LCA prefers the recoverable outcome: an
//! accidental newline costs one keystroke, an accidental submit costs a partial
//! prompt, a charge, and the draft's place.
//!
//! Three things keep that from becoming a new problem, and each has a row here:
//!
//! 1. **The fallback is scoped to the detection, not global.** On a terminal that
//!    reports Shift, `Enter` still submits — otherwise the fix would make the common
//!    case unreliable, which is worse than the bug.
//! 2. **The user can override it**, in both directions, so a platform heuristic is
//!    never a trap for someone who does not fit it.
//! 3. **A protocol that carries the modifier still wins.** Kitty CSI-u
//!    `Shift+Enter` inserts a newline whatever the policy says, and `\`+`Enter`
//!    keeps its documented meaning (the backslash is consumed).
//!
//! # Where the rows live
//!
//! The behaviour rows are beside the code they drive:
//! `crates/lca-tui/src/widgets/editor_tests.rs` for the editor outcome, and
//! `crates/lca-tui/src/engine/keys.rs`'s `enter_ambiguity_tests` for the detection
//! rule. This file guards the properties that a rewrite of either would lose: the
//! detection is a named predicate with injectable halves, the policy is consulted in
//! one place, and the protocol-carrying paths are not short-circuited by it.
//!
//! Verifies: gh #367, FR-UI-1 (the prompt editor), NFR-9/NFR-10 (cross-platform),
//! NFR-24.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use lca_tui::engine::keys::enter_is_ambiguous_with;

/// The editor, so the policy's place in the submit arm can be read.
fn editor_source() -> String {
    std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/crates/lca-tui/src/widgets/editor.rs"
    ))
    .expect("editor.rs is readable")
}

/// The detection rule.
fn keys_source() -> String {
    std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/crates/lca-tui/src/engine/keys.rs"
    ))
    .expect("keys.rs is readable")
}

// Verifies: gh #367 - the detection is a **named predicate with injectable halves**,
// not an inline `cfg!` in the key handler. A `cfg!` inside `handle_key` could not be
// tested on one machine, and the brief asked for a row rather than a branch.
#[test]
fn the_detection_is_a_named_predicate_with_injectable_platforms() {
    // Both platform flags and the environment are parameters, so every combination
    // is exercisable here on Linux.
    let apple = |env: &dyn Fn(&str) -> Option<String>| {
        enter_is_ambiguous_with(false, true, true, |k| env(k))
    };
    let term = |name: &'static str| move |k: &str| (k == "TERM_PROGRAM").then(|| name.to_string());

    assert!(
        apple(&term("Apple_Terminal")),
        "macOS + Apple_Terminal is the affected terminal"
    );
    assert!(
        !apple(&term("iTerm.app")),
        "another terminal on a Mac reports Shift"
    );
    assert!(
        !apple(&|_k: &str| None),
        "an unset TERM_PROGRAM is unknown, not Apple's Terminal"
    );
    // A Linux box claiming the name is not the terminal pi measured.
    assert!(
        !enter_is_ambiguous_with(false, false, true, term("Apple_Terminal")),
        "the platform half of the pair matters"
    );
    // Windows console: pi applies the treatment unconditionally.
    assert!(
        enter_is_ambiguous_with(true, false, true, |_k| None),
        "Windows consoles strip the modifier too"
    );
    // Remote: never asked.
    assert!(
        !enter_is_ambiguous_with(false, true, false, term("Apple_Terminal")),
        "over SSH the local keyboard is not the user's"
    );
}

// Verifies: gh #367 - the override works **in both directions**, so the heuristic is
// escapable. A user on an affected terminal wants `Enter` to submit; a user elsewhere
// may want the newline policy.
#[test]
fn the_override_forces_either_answer_in_both_directions() {
    let forced_off = enter_is_ambiguous_with(false, true, true, |k| match k {
        "TERM_PROGRAM" => Some("Apple_Terminal".into()),
        "LCA_SHIFT_ENTER" => Some("0".into()),
        _ => None,
    });
    let forced_on = enter_is_ambiguous_with(false, true, true, |k| match k {
        "LCA_SHIFT_ENTER" => Some("1".into()),
        _ => None,
    });
    assert!(
        !forced_off,
        "=0 restores Enter's submit on an affected terminal"
    );
    assert!(forced_on, "=1 opts an unaffected terminal into newline");
}

// Verifies: gh #367 - **the policy is consulted in exactly one place**, inside the
// submit arm, and the protocol-carrying paths reach their newline without it. A
// second check somewhere else would be a second policy to keep in step.
#[test]
fn the_policy_is_consulted_once_inside_the_submit_arm() {
    let source = editor_source();
    assert!(
        source.matches("enter_inserts_newline").count() >= 3,
        "the field, its setter and the arm all name it"
    );
    let submit_arm = source
        .find("tui.input.submit")
        .expect("the submit arm exists");
    let arm = &source[submit_arm..];
    let end = arm.find("tui.input.tab").unwrap_or(arm.len());
    let arm = &arm[..end];
    assert!(
        arm.contains("self.enter_inserts_newline"),
        "the policy is read inside the submit arm, not before it"
    );
    // The `\`-workaround keeps its documented meaning: it is checked *before* the
    // policy, so the backslash is consumed rather than left in the buffer.
    let backslash = arm
        .find("cursor_preceded_by_backslash")
        .expect("the workaround is in the same arm");
    let policy = arm
        .find("self.enter_inserts_newline")
        .expect("the policy is in the same arm");
    assert!(
        backslash < policy,
        "the explicit `\\` gesture is checked first, so it keeps its meaning"
    );
}

// Verifies: gh #367 - a **protocol that carries the modifier** is not routed through
// the policy: Kitty CSI-u and xterm `modifyOtherKeys` sequences reach
// `tui.input.newLine` directly. Rewriting their bytes in the policy would undo the
// protocol the terminal negotiated.
#[test]
fn the_shift_carrying_protocols_are_not_rewritten() {
    let source = keys_source();
    // The Enter matcher still consults the protocol sequences for Shift+Enter.
    let enter = source
        .find("\"enter\" | \"return\" =>")
        .expect("the Enter matcher exists");
    let arm = &source[enter..];
    let end = arm.find("\"escape\"").unwrap_or(arm.len().min(2000));
    let arm = &arm[..end];
    assert!(
        arm.contains("MOD_SHIFT"),
        "the Enter matcher still handles the Shift modifier: {arm}"
    );
    assert!(
        arm.contains("matches_kitty"),
        "...including the Kitty sequence, which carries it"
    );
    // And the editor does not special-case those bytes before the matcher.
    let editor = editor_source();
    assert!(
        !editor.contains("\\x1b[13;2u"),
        "the editor must not hard-code the Kitty sequence: the matcher owns it"
    );
}
