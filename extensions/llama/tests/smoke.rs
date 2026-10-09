//! Live llama smoke (NFR-23 pattern): one tiny turn against a real
//! local server. None runs here, so this skips — that is correct. To
//! run it, point at a server with a loaded model:
//! `export LCA_SMOKE_LLAMA_URL=http://127.0.0.1:8080 LCA_SMOKE_LLAMA_MODEL=<id>`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use std::sync::{Arc, Mutex};

#[test]
fn llama_live_smoke() {
    let (Ok(url), Ok(model)) = (
        std::env::var("LCA_SMOKE_LLAMA_URL"),
        std::env::var("LCA_SMOKE_LLAMA_MODEL"),
    ) else {
        eprintln!("skipping: LCA_SMOKE_LLAMA_URL/MODEL are not set (NFR-23)");
        return;
    };
    if url.is_empty() || model.is_empty() {
        eprintln!("skipping: LCA_SMOKE_LLAMA_URL/MODEL are empty (NFR-23)");
        return;
    }
    let root = lca_testkit::scratch_path("lca-llama-smoke");
    let _ = std::fs::remove_dir_all(&root);
    for dir in ["project", "private", "config", "data", "tmp"] {
        std::fs::create_dir_all(root.join(dir)).expect("mkdir");
    }
    let mut grants = llama::manifest_grants();
    grants
        .adhoc_net
        .push(lca_permissions::parse_net_pattern("127.0.0.1").expect("loopback pattern"));
    let cap = Arc::new(lca_tools::Capabilities::new(
        "llama",
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
    cap.credentials_set("base_url", &url).expect("seed");
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
        model,
        stable_prefix: 0,
        extras: Default::default(),
    };
    let mut text = String::new();
    llama::run_provider_stream(cap.as_ref(), &request, &mut |event| {
        if let lca_protocol::StreamEvent::TextDelta { delta } = event {
            text.push_str(&delta);
        }
        true
    })
    .expect("live turn streams");
    assert!(text.contains("smoke-ok"), "the server answered: {text}");
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
