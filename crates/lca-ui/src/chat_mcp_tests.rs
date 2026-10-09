//! `/mcp` dispatch rows (gh #53): verbs route through the host
//! hooks, and a hostless interface stays a notice.
//!
//! Split from `chat_tests.rs` under the workspace's 1,200-line file
//! ceiling. Fixtures (`options`, `chat`) stay in `chat_tests.rs`.

use super::*;
use super::{chat, options};

// Verifies: gh #53 - `/mcp` prints the status block with no verbs
// and routes verbs plus targets to the host action.
#[test]
fn mcp_verbs_route_through_the_hooks() {
    let mut chat_options = options();
    chat_options.hooks.mcp_action = Some(std::sync::Arc::new(|verb: &str, target: &str| {
        if verb.is_empty() {
            return "- echo [direct] (user): connected (1 tools)".to_string();
        }
        format!("verb={verb} target={target}")
    }));
    let mut chat = Chat::new(chat_options, std::sync::Arc::new(KeybindingsManager::new()));
    for c in "/mcp".chars() {
        chat.handle_key(&c.to_string());
    }
    assert_eq!(chat.handle_key("\r"), Action::Continue);
    assert!(
        chat.world.notice.as_deref().unwrap().contains("connected"),
        "{:?}",
        chat.world.notice
    );
    for c in "/mcp disable echo".chars() {
        chat.handle_key(&c.to_string());
    }
    assert_eq!(chat.handle_key("\r"), Action::Continue);
    assert_eq!(
        chat.world.notice.as_deref().unwrap(),
        "verb=disable target=echo"
    );
}

// Verifies: gh #53 - `/mcp` without a host stays a notice, never a
// submit.
#[test]
fn mcp_without_a_host_names_the_guide() {
    let mut chat = chat();
    for c in "/mcp".chars() {
        chat.handle_key(&c.to_string());
    }
    assert_eq!(chat.handle_key("\r"), Action::Continue);
    assert!(
        chat.world
            .notice
            .as_deref()
            .unwrap()
            .contains("authoring guide")
    );
    assert!(chat.take_submitted().is_none());
}

