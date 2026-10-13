//! Session emission (gh #258 §3B, acceptance 5): appends emit
//! type, id, and bytes — never content. Feature-gated.

#![cfg(feature = "debug-trace")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code

use lca_session::SessionStore;

fn capture(filter: &str, run: impl FnOnce()) -> String {
    let root = lca_testkit::scratch_path("debug-session-emit");
    let paths = lca_debug::DebugPaths::in_dir(&root);
    let config = lca_debug::DebugConfig {
        paths: paths.clone(),
        filter: Some(filter.to_string()),
    };
    let subscriber = lca_debug::subscriber(&config).expect("subscriber");
    tracing::subscriber::with_default(subscriber, run);
    std::fs::read_to_string(&paths.log).unwrap_or_default()
}

// Verifies: gh #258 - an append emits its shape without its content.
#[test]
fn appends_emit_shape_not_content() {
    let log = capture("lca_session=trace", || {
        let root = lca_testkit::scratch_path("debug-session-store");
        let project = root.join("project");
        std::fs::create_dir_all(&project).expect("mkdir");
        let store = SessionStore::new(root.clone());
        let session = store.create_session(&project, "test").expect("create");
        store
            .append(
                &session,
                lca_protocol::Record::User {
                    v: lca_protocol::FORMAT_VERSION,
                    ts: 1,
                    id: "user-1".to_string(),
                    parent: None,
                    content: "super secret prompt".to_string(),
                    attachments: vec![],
                    queue: None,
                },
            )
            .expect("append");
    });
    assert!(log.contains("record.append"), "appends emit:\n{log}");
    assert!(log.contains("type=user"), "the type lands:\n{log}");
    assert!(log.contains("user-1"), "the id lands:\n{log}");
    assert!(!log.contains("super secret prompt"), "content never emits");
}
