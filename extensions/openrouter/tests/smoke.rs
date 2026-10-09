//! Live OpenRouter smoke (NFR-23 pattern): one tiny turn against the
//! real gateway. No credentials exist here, so this skips — that is
//! correct. To run it, hand it a key by file reference only:
//! `export LCA_SMOKE_OPENROUTER_KEY=$(cat <file>)`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use std::sync::{Arc, Mutex};

#[test]
fn openrouter_live_smoke() {
    let Ok(key) = std::env::var("LCA_SMOKE_OPENROUTER_KEY") else {
        eprintln!("skipping: LCA_SMOKE_OPENROUTER_KEY is not set (NFR-23)");
        return;
    };
    if key.is_empty() {
        eprintln!("skipping: LCA_SMOKE_OPENROUTER_KEY is empty (NFR-23)");
        return;
    }
    let root = lca_testkit::scratch_path("lca-openrouter-smoke");
    let _ = std::fs::remove_dir_all(&root);
    for dir in ["project", "private", "config", "data", "tmp"] {
        std::fs::create_dir_all(root.join(dir)).expect("mkdir");
    }
    let mut grants = openrouter::manifest_grants();
    grants
        .adhoc_net
        .push(lca_permissions::parse_net_pattern("openrouter.ai").expect("gateway pattern"));
    let cap = Arc::new(lca_tools::Capabilities::new(
        "openrouter",
        grants,
        lca_permissions::ScopeRoots {
            workspace: root.join("project"),
            private: root.join("private"),
            home_config: root.join("config"),
            temp: root.join("tmp"),
            state_dir: root.join("data"),
        },
        Arc::new(Mutex::new(SmokePrompt)),
        Arc::new(Mutex::new(
            lca_permissions::GrantStore::open(&root.join("grants.json")).expect("store"),
        )),
        root.join("project"),
        None,
    ));
    cap.credentials_set("access", &key).expect("seed");
    let request = lca_protocol::CompletionRequest {
        messages: vec![lca_protocol::ChatMessage {
            role: lca_protocol::MessageRole::User,
            content: vec![lca_protocol::ContentBlock::Text {
                text: "reply with exactly: smoke-ok".to_string(),
            }],
            tool_calls: Vec::new(),
            tool_call_id: None,
            usage: None,
            extras: Default::default(),
        }],
        tools: Vec::new(),
        model: "openai/gpt-4o-mini".to_string(),
        stable_prefix: 0,
        extras: Default::default(),
    };
    let mut text = String::new();
    openrouter::run_provider_stream(cap.as_ref(), &request, &mut |event| {
        if let lca_protocol::StreamEvent::TextDelta { delta } = event {
            text.push_str(&delta);
        }
        true
    })
    .expect("live turn streams");
    assert!(text.contains("smoke-ok"), "the gateway answered: {text}");
}

struct SmokePrompt;
impl lca_permissions::PermissionPrompt for SmokePrompt {
    fn ask(&mut self, _action: &lca_permissions::Action) -> lca_permissions::Decision {
        lca_permissions::Decision::Always
    }
    fn review_proposals(&mut self, _diff: &lca_permissions::ProposalDiff) -> bool {
        false
    }
}
