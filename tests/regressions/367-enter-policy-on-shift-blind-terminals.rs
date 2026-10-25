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

use lca_tui::engine::keys::enter_is_ambiguous_on;

/// The editor, so the policy's place in the submit arm can be read.
fn editor_source() -> String {
    std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/crates/lca-tui/src/widgets/editor.rs"
    ))
    .expect("editor.rs is readable")
}

/// The platform-neutral console probe (gh #367's Windows half).
fn sys_source() -> String {
    std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/crates/lca-tui/src/engine/sys.rs"
    ))
    .expect("sys.rs is readable")
}

/// The detection rule.
fn keys_source() -> String {
    std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/crates/lca-tui/src/engine/keys.rs"
    ))
    .expect("keys.rs is readable")
}

fn env_of(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
    move |key| {
        pairs
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| (*v).to_string())
    }
}

// Verifies: gh #367 - the detection is a **named predicate with injectable halves**,
// not an inline `cfg!` in the key handler. A `cfg!` inside `handle_key` could not be
// tested on one machine, and the brief asked for a row rather than a branch.
#[test]
fn the_detection_is_a_named_predicate_with_injectable_platforms() {
    assert!(
        enter_is_ambiguous_on(true, true, env_of(&[("TERM_PROGRAM", "Apple_Terminal")])),
        "macOS + Apple_Terminal is the affected terminal"
    );
    assert!(
        !enter_is_ambiguous_on(true, true, env_of(&[("TERM_PROGRAM", "iTerm.app")])),
        "another terminal on a Mac reports Shift"
    );
    assert!(
        !enter_is_ambiguous_on(true, true, env_of(&[])),
        "an unset TERM_PROGRAM is unknown, not Apple's Terminal"
    );
    assert!(
        !enter_is_ambiguous_on(false, true, env_of(&[("TERM_PROGRAM", "Apple_Terminal")])),
        "the platform half of the pair matters"
    );
    assert!(
        !enter_is_ambiguous_on(true, false, env_of(&[("TERM_PROGRAM", "Apple_Terminal")])),
        "over SSH the local keyboard is not the user's"
    );
}

// Verifies: gh #367 - **Windows is deliberately not ambiguous.** Its console latches
// `SHIFT_PRESSED` from the key event, so `Enter` keeps submitting and `Shift+Enter`
// is recognised. Assuming Windows were blind broke the pre-existing
// `a_plain_enter_still_submits` row - the no-regression half of this issue.
#[test]
fn windows_is_not_treated_as_shift_blind() {
    let source = keys_source();
    assert!(
        !source.contains("cfg!(windows) || (cfg!(target_os = \"macos\")"),
        "Windows must not be in the always-ambiguous list"
    );
    // ...because the console answers the question itself.
    let sys = sys_source();
    assert!(
        sys.contains("shift_was_held_on_enter"),
        "the console probe exists"
    );
    assert!(
        sys.contains("dwControlKeyState") || sys.contains("SHIFT_PRESSED"),
        "and it reads the key event's own modifier bits"
    );
}

// Verifies: gh #367 - the override works **in both directions**, so the heuristic is
// escapable. A user on an affected terminal wants `Enter` to submit; a user elsewhere
// may want the newline policy.
#[test]
fn the_override_forces_either_answer_in_both_directions() {
    assert!(
        !enter_is_ambiguous_on(
            true,
            true,
            env_of(&[("TERM_PROGRAM", "Apple_Terminal"), ("LCA_SHIFT_ENTER", "0")])
        ),
        "=0 restores Enter's submit on an affected terminal"
    );
    assert!(
        enter_is_ambiguous_on(true, true, env_of(&[("LCA_SHIFT_ENTER", "1")])),
        "=1 opts an unaffected terminal into newline"
    );
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
