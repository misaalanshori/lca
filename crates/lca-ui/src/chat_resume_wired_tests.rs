//! The `/resume` picker's overlay rows (gh #110, split from
//! `chat_overlay_tests.rs` under the file ceiling).
//!
//! The tree itself is pinned beside the code in `chat_resume_tree_tests.rs`;
//! this file keeps the *wired-picker* row, which drives `/resume` through
//! `handle_key` and asserts what a user sees.

use super::*;
use super::{chat, strip};

#[test]
fn resume_picker_searches_sessions() {
    let mut chat = chat();
    chat.world.options.hooks.session_list = Some(Arc::new(|| {
        vec![
            crate::resume::SessionEntry {
                id: "a".into(),
                title: "parser fix".into(),
                messages: 3,
                age: "5m".into(),
                labels: Vec::new(),
                modified_ms: 0,
                parent: None,
            },
            crate::resume::SessionEntry {
                id: "b".into(),
                title: "docs pass".into(),
                messages: 1,
                age: "2d".into(),
                labels: Vec::new(),
                modified_ms: 0,
                parent: None,
            },
        ]
    }));
    for c in "/resume".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    assert!(chat.resume_picker.is_some());
    for c in "docs".chars() {
        chat.handle_key(&c.to_string());
    }
    assert_eq!(chat.resume_picker.as_ref().unwrap().matches, vec![1]);
    let viewport = strip(&chat.viewport(100, 30, 0)).join("\n");
    assert!(viewport.contains("docs pass"), "{viewport}");
    chat.handle_key("\r");
    assert!(
        chat.last_notice()
            .as_deref()
            .unwrap_or("")
            .contains("lca --resume b"),
        "{:?}",
        chat.last_notice()
    );
    chat.handle_key("\x1b");
    assert!(chat.resume_picker.is_none());
}
