//! Live Codex smoke (NFR-23 pattern): one tiny turn against the real
//! gateway. Subscription credentials do not exist here, so this
//! skips — that is correct. To run it, hand it a token by file
//! reference only: `export LCA_SMOKE_CODEX_TOKEN=$(cat <file>)`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use std::sync::{Arc, Mutex};

#[test]
fn codex_live_smoke() {
    let Ok(token) = std::env::var("LCA_SMOKE_CODEX_TOKEN") else {
        eprintln!("skipping: LCA_SMOKE_CODEX_TOKEN is not set (NFR-23)");
        return;
    };
    if token.is_empty() {
        eprintln!("skipping: LCA_SMOKE_CODEX_TOKEN is empty (NFR-23)");
        return;
    }
    let Some(account) = lca_subscription::account_from_jwt(
        &token,
        "https://api.openai.com/auth",
        "chatgpt_account_id",
    ) else {
        panic!("the smoke token carries no ChatGPT account");
    };
    let root = lca_testkit::scratch_path("lca-codex-smoke");
    let _ = std::fs::remove_dir_all(&root);
    for dir in ["project", "private", "config", "data", "tmp"] {
        std::fs::create_dir_all(root.join(dir)).expect("mkdir");
    }
    let mut grants = codex::manifest_grants();
    grants
        .adhoc_net
        .push(lca_permissions::parse_net_pattern("chatgpt.com").expect("gateway pattern"));
    let cap = Arc::new(lca_tools::Capabilities::new(
        "codex",
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
    cap.credentials_set("access", &token).expect("seed");
    cap.credentials_set("account_id", &account).expect("seed");
    cap.credentials_set("expires", &format!("{}", u64::MAX - 60))
        .expect("seed");
    let request = lca_protocol::CompletionRequest {
        messages: vec![lca_protocol::ChatMessage {
            role: lca_protocol::MessageRole::User,
            content: vec![lca_protocol::ContentBlock::Text {
                text: "reply with exactly: smoke ok".to_string(),
            }],
            tool_calls: Vec::new(),
            tool_call_id: None,
            usage: None,
            extras: Default::default(),
        }],
        tools: Vec::new(),
        model: "gpt-5.4-mini".to_string(),
        stable_prefix: 0,
        extras: Default::default(),
    };
    let mut text = String::new();
    codex::run_provider_stream(cap.as_ref(), &request, &mut |event| {
        if let lca_protocol::StreamEvent::TextDelta { delta } = event {
            text.push_str(&delta);
        }
        true
    })
    .expect("the live turn completes");
    assert!(!text.is_empty(), "the live turn answered");
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
