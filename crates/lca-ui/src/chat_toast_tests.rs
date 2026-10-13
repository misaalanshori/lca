//! Toasts and transcript routing (gh #239): transient confirmations
//! float top-right and fade; informational lines land in history.
//! Uses the `chat_tests` fixtures like the other overlay journeys.

use super::{chat, strip};

// Verifies: gh #239 - `say` lands in the transcript (pi's
// `showStatus`), not in editor chrome.
#[test]
fn say_lands_in_the_transcript() {
    let mut chat = chat();
    chat.say("model for this session: test-model");
    let viewport = strip(&chat.viewport(100, 30, 0)).join("\n");
    assert!(
        viewport.contains("model for this session: test-model"),
        "the line reads in history:\n{viewport}"
    );
}

// Verifies: gh #239 - a toast floats top-right, past the transcript
// text, and fades on expiry.
#[test]
fn toast_floats_top_right_and_fades() {
    let mut chat = chat();
    chat.transcript.push_user("a prompt with a very long line of text here");
    chat.toast("✓ Copied to clipboard");
    let viewport = strip(&chat.viewport(100, 30, 0)).join("\n");
    let row = viewport
        .lines()
        .enumerate()
        .find(|(_, line)| line.contains("Copied to clipboard"))
        .expect("toast paints");
    assert!(row.0 >= 1 && row.0 <= 3, "toast floats near the top");
    let toast_col = row.1.find("Copied").unwrap_or(0);
    assert!(
        toast_col > 50,
        "toast rides on the right half:\n{}",
        row.1
    );
    // Faded toasts leave the frame on tick.
    for toast in &mut chat.toasts {
        toast.until = std::time::Instant::now() - std::time::Duration::from_secs(1);
    }
    assert!(chat.tick(), "expiry repaints");
    let viewport = strip(&chat.viewport(100, 30, 0)).join("\n");
    assert!(
        !viewport.contains("Copied to clipboard"),
        "faded toasts leave"
    );
}

// Verifies: gh #239 - past the cap the oldest toast drops, so a flood
// of confirmations stays one stack, never a wall.
#[test]
fn toast_stack_caps_at_three() {
    let mut chat = chat();
    for i in 0..5 {
        chat.toast(format!("toast {i}"));
    }
    assert_eq!(chat.toasts.len(), crate::chat::TOAST_MAX);
    assert_eq!(chat.toasts.last().map(|t| t.text.as_str()), Some("toast 4"));
}
