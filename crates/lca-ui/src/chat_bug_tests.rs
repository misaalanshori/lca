//! The `/bug` inspection gate (gh #411 gaps 2 and 3).
//!
//! Split out of `chat_overlay_tests.rs` when that file crossed gate 11's 1,200-line
//! ceiling. These rows drive the staged-bundle screen through the `stage_bug` hook:
//! opening it writes nothing, `y` confirms, `n`/`Escape` cancels, any other key
//! keeps it up, and the screen counts as a blocked modal while it waits.
//!
//! The bundle's own scrubbing is tested where it lives - `crates/lca-cli/src/bug.rs`
//! and `tests/regressions/411-bug-bundle-redaction.rs` - because these rows are
//! about the gate, not the redaction.

use super::{chat, strip};
use std::sync::Arc;

// ---------------------------------------------------------------------
// gh #411 gaps 2 and 3: the `/bug` inspection is the gate.
// ---------------------------------------------------------------------

/// A staged bundle whose resolve records what it was told and returns a notice.
fn staged_bug(confirmed: Arc<std::sync::Mutex<Option<bool>>>) -> crate::state::BugReview {
    crate::state::BugReview {
        inspection: "This bundle will contain:\n  session.jsonl    412 bytes\n\
                     staged at: /tmp/lca-bug-stage-1\n\
                     scrubbed 1 API key\n"
            .to_string(),
        destination: "/tmp/lca-bug-abc.zip".to_string(),
        resolve: Arc::new(move |yes: bool| {
            *confirmed.lock().unwrap_or_else(|p| p.into_inner()) = Some(yes);
            if yes {
                "wrote /tmp/lca-bug-abc.zip (scrubbed 1 API key)".to_string()
            } else {
                "bug bundle discarded; nothing was written".to_string()
            }
        }),
    }
}

// Verifies: gh #411 gap 2 - `/bug` opens the inspection instead of zipping
// immediately, and the screen shows the file list, sizes, staged path and the
// scrub summary.
#[test]
fn bug_opens_an_inspection_screen_before_writing() {
    let mut chat = chat();
    let confirmed = Arc::new(std::sync::Mutex::new(None));
    chat.world.options.hooks.stage_bug = Some(Arc::new({
        let confirmed = confirmed.clone();
        move || Ok(staged_bug(confirmed.clone()))
    }));

    for ch in "/bug".chars() {
        chat.handle_key(&ch.to_string());
    }
    chat.handle_key("\r");
    assert!(
        chat.world.bug_review.is_some(),
        "the inspection is open, and nothing was written yet"
    );
    assert!(
        confirmed
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .is_none(),
        "opening the screen writes nothing"
    );

    let viewport = strip(&chat.viewport(120, 40, 0)).join("\n");
    for fact in [
        "bug bundle - review before sharing",
        "session.jsonl",
        "412 bytes",
        "/tmp/lca-bug-stage-1",
        "scrubbed 1 API key",
        "Write the bundle [y] / Cancel [n]",
    ] {
        assert!(
            viewport.contains(fact),
            "the screen shows {fact:?}:\n{viewport}"
        );
    }
}

// Verifies: gh #411 gap 2 - confirming writes; the seam is what decides, and its
// notice lands in the transcript.
#[test]
fn confirming_the_bug_inspection_writes_it() {
    let mut chat = chat();
    let confirmed = Arc::new(std::sync::Mutex::new(None));
    chat.world.options.hooks.stage_bug = Some(Arc::new({
        let confirmed = confirmed.clone();
        move || Ok(staged_bug(confirmed.clone()))
    }));
    for ch in "/bug".chars() {
        chat.handle_key(&ch.to_string());
    }
    chat.handle_key("\r");
    chat.handle_key("y");
    assert!(
        chat.world.bug_review.is_none(),
        "the screen closes on a confirm"
    );
    assert_eq!(
        *confirmed.lock().unwrap_or_else(|p| p.into_inner()),
        Some(true),
        "yes reaches the host seam"
    );
    let viewport = strip(&chat.viewport(120, 40, 0)).join("\n");
    assert!(
        viewport.contains("wrote /tmp/lca-bug-abc.zip"),
        "the seam's notice shows:\n{viewport}"
    );
}

// Verifies: gh #411 gap 2 - cancelling routes to the host with `false` (which
// removes the staging directory), and says nothing was written.
#[test]
fn cancelling_the_bug_inspection_writes_nothing() {
    let mut chat = chat();
    let confirmed = Arc::new(std::sync::Mutex::new(None));
    chat.world.options.hooks.stage_bug = Some(Arc::new({
        let confirmed = confirmed.clone();
        move || Ok(staged_bug(confirmed.clone()))
    }));
    for ch in "/bug".chars() {
        chat.handle_key(&ch.to_string());
    }
    chat.handle_key("\r");
    // The raw ESC byte, not the word: `parse_key` maps the byte, and the editor
    // would otherwise eat the literal string.
    chat.handle_key("");
    assert!(chat.world.bug_review.is_none(), "escape closes the screen");
    assert_eq!(
        *confirmed.lock().unwrap_or_else(|p| p.into_inner()),
        Some(false),
        "escape reaches the host seam as a cancel"
    );
    let viewport = strip(&chat.viewport(120, 40, 0)).join("\n");
    assert!(
        viewport.contains("discarded; nothing was written"),
        "the user is told nothing was written:\n{viewport}"
    );
}

// Verifies: gh #411 gap 2 - a key that is neither answer keeps the screen up.
//
// The whole point is that the user reads it; a stray keypress must not write a
// bundle or throw the inspection away.
#[test]
fn an_unrelated_key_keeps_the_inspection_open() {
    let mut chat = chat();
    let confirmed = Arc::new(std::sync::Mutex::new(None));
    chat.world.options.hooks.stage_bug = Some(Arc::new({
        let confirmed = confirmed.clone();
        move || Ok(staged_bug(confirmed.clone()))
    }));
    for ch in "/bug".chars() {
        chat.handle_key(&ch.to_string());
    }
    chat.handle_key("\r");
    for key in ["a", "down", "left", "enter"] {
        chat.handle_key(key);
        assert!(
            chat.world.bug_review.is_some(),
            "{key:?} must keep the screen up, not answer for the user"
        );
    }
    assert!(
        confirmed
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .is_none(),
        "and nothing was decided"
    );
}

// Verifies: gh #411 - the inspection holds the busy state: a staged bundle is a
// wait on the user, exactly like a permission prompt (gh #369's table).
#[test]
fn the_bug_inspection_counts_as_a_blocked_modal() {
    let mut chat = chat();
    let confirmed = Arc::new(std::sync::Mutex::new(None));
    chat.world.options.hooks.stage_bug = Some(Arc::new({
        let confirmed = confirmed.clone();
        move || Ok(staged_bug(confirmed.clone()))
    }));
    assert!(!crate::status::world_blocked(&chat), "idle before `/bug`");
    for ch in "/bug".chars() {
        chat.handle_key(&ch.to_string());
    }
    chat.handle_key("\r");
    assert!(
        crate::status::world_blocked(&chat),
        "the inspection waits on the user"
    );
}
