//! Gh #590: one failed turn paints **one** error line.
//!
//! Every row about errors before this one asserted that the text *appears*,
//! which is exactly why nobody saw the duplicate: a terminal failure arrived on
//! two channels at once. `TurnEvent::Error` painted it as `Entry::Error` (the
//! turn's own event - the channel every host's listener sees, and the one
//! `headless` turns into its JSON `error` envelope), and `TurnState::reap`
//! painted `outcome.error` as an `Entry::Notice` (the channel every *command*
//! answer uses). Both carried the same string, so the screen said it twice.
//!
//! These rows assert the **count**, and each drives the seam that paints it:
//! row 1 goes through `TurnState::reap` (a row that only called
//! `Chat::on_turn_event` would see one line and pass on the broken tree - that
//! seam split is why the old rows were blind), and rows 2-4 pin the fallback so
//! the fix cannot delete it.
//!
//! Verifies: gh #590, FR-CORE-7 (the user sees the failure and keeps control).

use crate::transcript::Entry;

/// The text both channels carry - one string, which is what made the duplicate
/// invisible: `TurnOutcome::error` and `TurnEvent::Error { message }` agree by
/// construction, because the event is built from the same value.
const FAILURE: &str = "provider returned HTTP 401: the key is wrong";

/// How many painted entries carry this text, split by channel.
fn painted(chat: &crate::chat::Chat, text: &str) -> (usize, usize) {
    let errors = chat
        .transcript
        .entries()
        .iter()
        .filter(|entry| matches!(entry, Entry::Error(line) if line.contains(text)))
        .count();
    let notices = chat
        .transcript
        .entries()
        .iter()
        .filter(|entry| matches!(entry, Entry::Notice(line) if line.contains(text)))
        .count();
    (errors, notices)
}

/// A finished turn handle carrying an outcome, as the worker leaves behind.
fn finished(
    outcome: lca_protocol::TurnOutcome,
) -> std::thread::JoinHandle<lca_protocol::TurnOutcome> {
    std::thread::spawn(move || outcome)
}

/// Wait until the spawned handle has landed (a fresh one is not finished yet).
fn settle(turns: &mut crate::run::TurnState) {
    for _ in 0..200 {
        if turns
            .active
            .as_ref()
            .is_some_and(|handle| handle.is_finished())
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

fn failed_outcome() -> lca_protocol::TurnOutcome {
    lca_protocol::TurnOutcome {
        status: lca_protocol::TurnStatus::Error,
        stop_reason: lca_protocol::StopReason::Error,
        usage: lca_protocol::Usage::default(),
        error: Some(FAILURE.to_string()),
    }
}

fn turn_state() -> crate::run::TurnState {
    let (prompt_tx, prompt_rx) = std::sync::mpsc::sync_channel(4);
    let (_dialog_tx, dialog_rx) = std::sync::mpsc::sync_channel(4);
    crate::run::TurnState::new(prompt_tx, prompt_rx, dialog_rx, super::tests::test_waker())
}

// Verifies: gh #590 - **a failed turn paints exactly one error line.**
//
// Both channels carry the same text: the event arrives first (as it does in a
// real turn, where core emits `TurnEvent::Error` and then returns an outcome
// holding the same `message`), then `reap` sees the outcome. On the pre-fix tree
// this row reads one error line **and one notice** - the second is the duplicate.
#[test]
fn a_failed_turn_paints_one_error_line() {
    let mut turns = turn_state();
    let mut chat = super::tests::chat();
    chat.begin_turn(lca_protocol::steer_queue());
    // Core's order: the error event, then the turn end.
    chat.on_turn_event(lca_protocol::TurnEvent::Error {
        message: FAILURE.to_string(),
        class: "auth".to_string(),
        retryable: false,
    });
    chat.on_turn_event(lca_protocol::TurnEvent::TurnEnded {
        status: lca_protocol::TurnStatus::Error,
        stop_reason: lca_protocol::StopReason::Error,
    });
    turns.active = Some(finished(failed_outcome()));
    settle(&mut turns);
    assert!(turns.reap(&mut chat), "the turn reaped");
    let (errors, notices) = painted(&chat, FAILURE);
    assert_eq!(
        errors,
        1,
        "exactly one error line for one failed turn: {:?}",
        chat.transcript.entries()
    );
    assert_eq!(
        notices, 0,
        "the outcome's notice is the fallback, so it must not repeat a line the \
         turn's own event already painted ({notices} notice(s) for the same failure)"
    );
}

// Verifies: gh #590 - **the fallback survives.** A turn can fail before any sink
// event exists: `run.rs::reap` substitutes an outcome for a panicking join, and
// core's own `fail()` sites return before a sink is handed anything (`turn/mod.rs`
// on a failed session-log write, `lib.rs` on the metadata write). The reader still
// has to see why, so the notice path stays - this row is what stops the fix from
// deleting it.
#[test]
fn a_turn_with_no_sink_still_reports_its_error() {
    let mut turns = turn_state();
    let mut chat = super::tests::chat();
    chat.begin_turn(lca_protocol::steer_queue());
    turns.active = Some(finished(failed_outcome()));
    settle(&mut turns);
    assert!(turns.reap(&mut chat), "the turn reaped");
    let (errors, notices) = painted(&chat, FAILURE);
    assert_eq!(errors, 0, "no event arrived, so no error line was painted");
    assert_eq!(
        notices,
        1,
        "the fallback notice still tells the reader what failed: {:?}",
        chat.transcript.entries()
    );
}

// Verifies: gh #590 - **a retried-then-successful turn paints nothing terminal.**
//
// The brief's second sub-question was "does a retry look terminal?", and the
// answer from the code is no: `TurnEvent::Error` is emitted only on the exhaustion
// arm of `turn/stream.rs` (the retry loop emits `RetryScheduled`/`RetryFinished`
// and formats `(retries exhausted after N)` into the message before that event).
// This row is the receipt.
#[test]
fn a_recovered_turn_paints_nothing_terminal() {
    let mut turns = turn_state();
    let mut chat = super::tests::chat();
    chat.begin_turn(lca_protocol::steer_queue());
    chat.on_turn_event(lca_protocol::TurnEvent::RetryScheduled {
        attempt: 1,
        max: 3,
        delay_ms: 250,
        error: FAILURE.to_string(),
    });
    chat.on_turn_event(lca_protocol::TurnEvent::RetryFinished { success: true });
    chat.on_turn_event(lca_protocol::TurnEvent::TextDelta("ok".to_string()));
    chat.on_turn_event(lca_protocol::TurnEvent::TurnEnded {
        status: lca_protocol::TurnStatus::Ok,
        stop_reason: lca_protocol::StopReason::Stop,
    });
    turns.active = Some(finished(lca_protocol::TurnOutcome {
        status: lca_protocol::TurnStatus::Ok,
        stop_reason: lca_protocol::StopReason::Stop,
        usage: lca_protocol::Usage::default(),
        error: None,
    }));
    settle(&mut turns);
    assert!(turns.reap(&mut chat), "the turn reaped");
    let (errors, notices) = painted(&chat, FAILURE);
    assert_eq!(
        errors,
        0,
        "a recovered turn paints no error line: {:?}",
        chat.transcript.entries()
    );
    assert_eq!(
        notices, 1,
        "the retry countdown names the attempt (that is the retry channel, not a terminal \
         error), and nothing else repeats it"
    );
}

// Verifies: gh #590 with gh #462's routing rule - **a background turn's flag is
// its own.** The turn belongs to the tab that submitted it, so `reap` must read
// that tab's paint flag and transcript: a failing background turn gets its
// fallback in *its* transcript, and the visible tab stays untouched.
/// A chat whose host can open a second tab (the gh #462 fixture shape): `open_tab`
/// refuses without a `new_session` hook, and this row needs two tabs.
fn tabbed_chat() -> crate::chat::Chat {
    let minted = std::sync::Arc::new(std::sync::Mutex::new(2u32));
    let next = minted.clone();
    let mut options = super::tests::options();
    options.hooks.new_session = Some(std::sync::Arc::new(move || {
        let mut n = next.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let id = format!("s{}", *n);
        *n += 1;
        Some(id)
    }));
    options.hooks.current_session_id = Some(std::sync::Arc::new(|| "s1".to_string()));
    crate::chat::Chat::new(
        options,
        std::sync::Arc::new(lca_tui::engine::keybindings::KeybindingsManager::new()),
    )
}

#[test]
fn a_background_turn_reads_its_own_paint_flag() {
    let mut turns = turn_state();
    let mut chat = tabbed_chat();
    // A second tab first: `open_tab` refuses *during* a turn (gh #462), and the
    // refusal is right - a new session would rewrite the cell the running worker
    // reads. So the tabs exist before the turn starts.
    chat.handle_key("\x14");
    assert_eq!(chat.tabs_len(), 2, "a second tab opened");
    assert_eq!(chat.active_tab_index(), 1, "the new tab is visible");
    // Back to tab 0, which is where the turn is submitted from.
    chat.handle_key("\x1b1");
    assert_eq!(chat.active_tab_index(), 0, "tab 0 is visible");
    chat.begin_turn(lca_protocol::steer_queue()); // tab 0 owns the turn
    // Then switch away, so the owner is off-screen when it settles.
    chat.handle_key("\x1b2");
    assert_eq!(chat.active_tab_index(), 1, "the reader moved away");
    turns.active = Some(finished(failed_outcome()));
    settle(&mut turns);
    assert!(turns.reap(&mut chat), "the turn reaped");
    let (visible_errors, visible_notices) = painted(&chat, FAILURE);
    assert_eq!(
        (visible_errors, visible_notices),
        (0, 0),
        "the visible tab's transcript did not paint the background turn's failure"
    );
    // Back to the owner: its transcript holds the one fallback line.
    chat.handle_key("\x1b1");
    assert_eq!(chat.active_tab_index(), 0);
    let (errors, notices) = painted(&chat, FAILURE);
    assert_eq!(
        (errors, notices),
        (0, 1),
        "the failing background turn got its fallback notice exactly once, in its own \
         transcript: {:?}",
        chat.transcript.entries()
    );
}
