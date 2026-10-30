//! Rows for the platform layer's decisions (`sys.rs`), split out so the parts that
//! do not need a real console can be exercised on every platform.
//!
//! Two of gh #323's four defects are Windows-only and cannot be *driven* here -
//! there is no `SetConsoleMode` and no `INPUT_RECORD` on Linux. What can be tested
//! on any platform is the **decision** each one makes, which is the part that
//! regresses silently: "is this record a resize?" and "does a refused mode change
//! surface?". The OS plumbing stays untested here and the report says so.

use super::*;

// Verifies: gh #323 defect 4 - a console record that changes the buffer size is
// recognised as a resize, and the records the engine genuinely ignores are not.
//
// The defect was `wait_stdin` consuming every non-key record and looping, so a
// `WINDOW_BUFFER_SIZE_EVENT` was read and thrown away. The fix reports it. This row
// holds the classification half; the `ReadConsoleInputW` plumbing that feeds it is
// Windows-only and is named as untested-on-this-box in the report.
//
// The negative half matters as much as the positive one: treating *every* non-key
// record as a resize would repaint on every mouse move and focus change, which is a
// worse bug than the one being fixed.
#[test]
fn only_a_buffer_size_record_counts_as_a_resize() {
    /// `wincon.h`'s record types.
    const KEY_EVENT: u16 = 0x0001;
    const MOUSE_EVENT: u16 = 0x0002;
    const WINDOW_BUFFER_SIZE_EVENT: u16 = 0x0004;
    const MENU_EVENT: u16 = 0x0008;
    const FOCUS_EVENT: u16 = 0x0010;

    assert!(
        console_record_meaning(WINDOW_BUFFER_SIZE_EVENT),
        "a buffer-size record must wake the loop and repaint"
    );
    for ignorable in [KEY_EVENT, MOUSE_EVENT, MENU_EVENT, FOCUS_EVENT, 0, 0x00ff] {
        assert!(
            !console_record_meaning(ignorable),
            "record type {ignorable:#06x} must not be treated as a resize"
        );
    }
}

// Verifies: gh #323 - `WaitOutcome::is_key` is the one predicate callers use, so a
// resize is never mistaken for a key (which would block `read_stdin` forever on a
// console handle that signalled for a non-key event - the hang the old comment
// documents).
#[test]
fn only_the_key_outcome_counts_as_a_key() {
    assert!(WaitOutcome::Key.is_key());
    assert!(!WaitOutcome::Resize.is_key());
    assert!(!WaitOutcome::Timeout.is_key());
}

// Verifies: gh #323 defect 3 - a refused `SetConsoleMode` produces an **error that
// names the handle and carries the OS code**, rather than being dropped on the
// floor.
//
// The defect was the bare `unsafe { SetConsoleMode(..) };` statement: the return
// value was discarded, so a refusal left the app believing it was in raw mode while
// the console stayed cooked. The symptom arrived much later, as garbled input or a
// prompt that will not submit, with nothing pointing at the cause.
//
// What this row cannot do here: call `SetConsoleMode` (Linux has no such function),
// so the *call* is untested on this box and the report says so. What it does hold is
// the contract the caller depends on - an `Err`, not a panic, naming the handle and
// the code.
#[test]
fn a_refused_console_mode_is_an_error_that_names_the_handle_and_the_code() {
    let refusal = set_mode_refusal("stdin", 5);
    let message = refusal.to_string();
    assert!(
        message.contains("stdin"),
        "names the handle so the caller knows which one: {message}"
    );
    assert!(
        message.contains("os error 5"),
        "carries the OS code, which is the only actionable part: {message}"
    );
    assert!(
        message.contains("raw mode"),
        "says what failed to happen, not only which call: {message}"
    );
}
