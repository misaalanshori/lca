//! Gh #369 (NFR-24): two ambient-status defects.
//!
//! 1. `OSC 9;4` taskbar progress was written once at turn start. Terminal
//!    emulators put a ~10-15 s watchdog on it so a crashed CLI does not spin
//!    forever, so any turn longer than that silently lost its indicator
//!    mid-flight. The keepalive refreshes it once per
//!    `PROGRESS_KEEPALIVE_MS` and goes quiet when the indicator is off (the
//!    house rule: an idle interface never repaints).
//! 2. `OSC 7501` tab status covered turns only. A background `/compact`, an
//!    inline `!`/`!!` run, and the extension install-consent prompt were
//!    invisible to a tab bar: the first two did not badge `working`, and the
//!    consent prompt did not badge `blocked` at all.
//!
//! These rows pin the constants and the single-vocabulary rule at the source
//! level; the behaviour is pinned by the unit rows beside each file.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failing assertion.

fn terminal_source() -> &'static str {
    include_str!("../../crates/lca-tui/src/engine/terminal.rs")
}

fn status_source() -> &'static str {
    include_str!("../../crates/lca-ui/src/status.rs")
}

fn run_source() -> &'static str {
    include_str!("../../crates/lca-ui/src/run.rs")
}

/// The lifecycle → ambient-channel sync, split out of `run.rs` under the
/// 1,200-line ceiling (gate 11) as this batch's brief allows.
fn run_status_source() -> &'static str {
    include_str!("../../crates/lca-ui/src/run_status.rs")
}

/// The keepalive exists, is 1000 ms, and is driven from a tick seam rather
/// than a new thread (pi's cadence; the issue asks which seam).
#[test]
fn the_progress_keepalive_is_a_second_not_a_thread() {
    let source = terminal_source();
    assert!(
        source.contains("pub const PROGRESS_KEEPALIVE_MS: Duration = Duration::from_millis(1000)"),
        "the keepalive window is 1000 ms"
    );
    assert!(
        source.contains("pub struct ProgressKeepalive"),
        "the keepalive is a state machine, not a bare atomic"
    );
    // One emitter pair, the documented bytes.
    assert!(source.contains(r#""\x1b]9;4;3\x07""#), "OSC 9;4;3");
    assert!(source.contains(r#""\x1b]9;4;0\x07""#), "OSC 9;4;0");
    // No spawned thread for it: the UI loop's tick drives it.
    assert!(
        !source.contains("PROGRESS_KEEPALIVE_MS) {\n            std::thread::spawn"),
        "the keepalive must not spawn a thread"
    );
    assert!(
        source.contains("fn tick_progress"),
        "the Terminal trait exposes a tick the loop can drive"
    );
}

/// The three new states read one `WorkKind`, so they cannot disagree about
/// what "working" means, and the badge sync is the single place that arms
/// the spinner (the old hand-written arm/disarm edges are gone).
#[test]
fn one_work_kind_feeds_the_badge_and_the_spinner() {
    let status = status_source();
    for name in ["Turn", "Compact", "Shell"] {
        assert!(
            status.contains(&format!("WorkKind::{name}")),
            "`WorkKind::{name}` exists, so the state is named not implied"
        );
    }
    assert!(
        status.contains("fn work_state(turn_running: bool, compacting: bool, shell_running: bool)"),
        "one classification function takes all three sources"
    );
    let run = run_source();
    let sync = run_status_source();
    assert!(
        sync.contains("crate::status::work_state("),
        "the sync classifies through the shared function"
    );
    assert!(
        sync.contains("chat.compacting") && sync.contains("chat.shell_running()"),
        "compaction and shell runs reach the classifier"
    );
    assert!(
        run.contains("turns.sync_program_status("),
        "the loop drives the sync each tick"
    );
    // The spinner must not be armed from two places (the old `set_progress`
    // calls in `start`/`reap` were the drift this replaced). The one
    // remaining call site is the sync's own edge detector.
    assert_eq!(
        sync.matches("terminal.set_progress(").count(),
        1,
        "exactly one place arms the spinner"
    );
    assert_eq!(
        run.matches("terminal.set_progress(").count(),
        0,
        "and the loop does not arm it a second time"
    );
}

/// The blocked vocabulary is one table, and the install-consent modal is in
/// it: that omission is the issue's third gap.
#[test]
fn the_blocked_vocabulary_is_one_table_including_install_consent() {
    let source = status_source();
    assert!(
        source.contains("pub const BLOCKED_MODALS"),
        "one vocabulary table"
    );
    assert!(
        source.contains("\"install_consent\""),
        "the install-consent modal is in the vocabulary"
    );
    assert!(
        source.contains("chat.install_consent.is_some()"),
        "and it has a reader, so it actually counts as blocked"
    );
}

/// A suspended terminal restores its badge: `$EDITOR` `stop`s the terminal
/// (which writes `state=clear`) and then `start`s it, so without the stored
/// state the tab bar would stay blank for the rest of the session.
#[test]
fn a_suspend_restores_the_badge() {
    let source = terminal_source();
    assert!(
        source.contains("program_status: Option<ProgramState>"),
        "the last real badge is remembered on the terminal"
    );
    assert!(
        source.contains("if let Some(state) = self.program_status {"),
        "`start()` re-emits the remembered badge"
    );
}

/// Privacy: the OSC 7501 payload still carries only `state=…:app=lca`.
/// Nothing from this batch may add prompt, file, or model text.
#[test]
fn the_status_payload_stays_privacy_safe() {
    let source = include_str!("../../crates/lca-tui/src/engine/program_status.rs");
    assert!(
        source.contains(r#""\x1b]7501;state={}:app=lca\x1b\\""#),
        "the wire format is unchanged: state and app only"
    );
    assert!(
        !source.contains("msg="),
        "no message payload was added (pi's `msg=` stays out)"
    );
}
