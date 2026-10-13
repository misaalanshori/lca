//! The permission modal's sensitive-read badge (gh #215).
//! Uses the `chat_tests` fixtures like the other overlay journeys.

use super::{chat, strip};

// Verifies: gh #215 - the permission modal badges a sensitive read
// with its security boundary; an ordinary read carries no badge.
#[test]
fn the_permission_modal_badges_sensitive_reads() {
    for (action, badge) in [
        ("read /proj/.env", true),
        ("read /proj/src/main.rs", false),
        ("cargo build", false),
    ] {
        let mut chat = chat();
        chat.world.permission = Some(crate::state::PermissionModal {
            action: action.into(),
            respond: None,
            deadline: None,
        });
        let viewport = strip(&chat.viewport(100, 30, 0)).join("\n");
        assert_eq!(
            viewport.contains("Sensitive file read requested"),
            badge,
            "action {action}"
        );
    }
}
