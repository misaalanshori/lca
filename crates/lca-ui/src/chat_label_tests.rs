//! `/label` `/labels` `/jump` rows (gh #37): bookmarks route through
//! the host hooks, and a hostless interface stays a notice.
//!
//! Split from `chat_tests.rs` under the workspace's 1,200-line file
//! ceiling. Fixtures (`options`, `chat`) stay in `chat_tests.rs`.

use super::*;
use super::{chat, options};

// Verifies: FR-SESS-10 - `/label` bookmarks the latest message by
// default (gh #37).
#[test]
fn label_bookmarks_the_latest_message_by_default() {
    let mut options = options();
    options.hooks.set_label = Some(Arc::new(|index: usize, name: &str| {
        assert_eq!(index, 1, "latest of two user messages");
        assert_eq!(name, "checkpoint");
        Ok("r-abc".to_string())
    }));
    let mut chat = Chat::new(options, Arc::new(KeybindingsManager::new()));
    chat.transcript.push_user("first");
    chat.transcript.push_user("second");
    for c in "/label checkpoint".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    let notice = chat.world.notice.clone().unwrap_or_default();
    assert!(
        notice.contains("checkpoint") && notice.contains("r-abc"),
        "{notice}"
    );
}

// Verifies: FR-SESS-10 - `/label <n> <name>` bookmarks the nth message
// (gh #37).
#[test]
fn label_with_index_bookmarks_the_nth_message() {
    let mut options = options();
    options.hooks.set_label = Some(Arc::new(|index: usize, name: &str| {
        assert_eq!(index, 0);
        assert_eq!(name, "first mark");
        Ok("r-001".to_string())
    }));
    let mut chat = Chat::new(options, Arc::new(KeybindingsManager::new()));
    chat.transcript.push_user("first");
    chat.transcript.push_user("second");
    for c in "/label 0 first mark".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    assert!(
        chat.world.notice.as_deref().unwrap().contains("first mark"),
        "{:?}",
        chat.world.notice
    );
}

// Verifies: FR-SESS-10 - the label verbs refuse without a host (gh #37).
#[test]
fn label_verbs_refuse_without_a_host() {
    let mut chat = chat();
    for command in ["/label x", "/labels", "/jump x"] {
        for c in command.chars() {
            chat.handle_key(&c.to_string());
        }
        chat.handle_key("\r");
        assert!(
            chat.world
                .notice
                .as_deref()
                .unwrap()
                .contains("not available in this host"),
            "{command}: {:?}",
            chat.world.notice
        );
        chat.editor.set_text("");
    }
}

// Verifies: FR-SESS-10 - `/labels` lists every live bookmark (gh #37).
#[test]
fn labels_lists_every_bookmark() {
    let mut options = options();
    options.hooks.list_labels = Some(Arc::new(|| {
        vec![
            ("b-mark".to_string(), "r2".to_string()),
            ("a-mark".to_string(), "r1".to_string()),
        ]
    }));
    let mut chat = Chat::new(options, Arc::new(KeybindingsManager::new()));
    for c in "/labels".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    // gh #234: the list rides the transcript now, not the dock.
    let text: String = chat
        .transcript
        .entries()
        .iter()
        .filter_map(|entry| match entry {
            crate::transcript::Entry::Notice(body) => Some(body.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("a-mark") && text.contains("b-mark"), "{text}");
}

// Verifies: FR-SESS-10 - `/jump` forks at the bookmark and switches
// (gh #37, ADR-0046: navigation is fork-plus-switch).
#[test]
fn jump_forks_at_the_bookmark_and_switches() {
    let mut options = options();
    options.hooks.list_labels = Some(Arc::new(|| vec![("here".to_string(), "r-2".to_string())]));
    options.hooks.fork_record = Some(Arc::new(|id: &str| {
        assert_eq!(id, "r-2");
        crate::state::ForkReport {
            id: Some("newbranch".to_string()),
            notice: "forked".to_string(),
        }
    }));
    options.hooks.switch_session = Some(Arc::new(|_id: &str| Some(vec![])));
    let mut chat = Chat::new(options, Arc::new(KeybindingsManager::new()));
    for c in "/jump here".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    let notice = chat.world.notice.clone().unwrap_or_default();
    assert!(
        notice.contains("here") && notice.contains("newbranch"),
        "{notice}"
    );
}

// Verifies: FR-SESS-10 - `/jump` names the miss (gh #37).
#[test]
fn jump_names_an_unknown_bookmark() {
    let mut options = options();
    options.hooks.list_labels = Some(Arc::new(Vec::new));
    options.hooks.fork_record = Some(Arc::new(|_: &str| crate::state::ForkReport {
        id: None,
        notice: "unreachable".to_string(),
    }));
    let mut chat = Chat::new(options, Arc::new(KeybindingsManager::new()));
    for c in "/jump nowhere".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    assert!(
        chat.world.notice.as_deref().unwrap().contains("nowhere"),
        "{:?}",
        chat.world.notice
    );
}
