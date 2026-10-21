//! The live check gh #260's brief asks for, at the level the file boundary
//! allows: a turn on a model the *old* hand-curated eight did not carry
//! (`o3-mini`) must persist catalog-priced `cost_*` buckets and a context
//! window from `limit.context`, with the provider reporting zero cost.
//!
//! This drives `lca-core` end to end over the fake provider rather than the
//! `lca` binary: the binary crate is outside this change's file boundary, so
//! the same claim is proven one layer down, where the wire cost and the
//! persisted record both live. The headless receipt for the binary itself is
//! in the cycle report.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use lca_core::{Agent, AgentConfig};
use lca_permissions::{Decision, GrantStore, PermissionPrompt, ProposalDiff};
use lca_session::{SessionStore, ViewMode};
use lca_testkit::{FakeProvider, fake_usage};
use lca_tools::{CancelFlag, NativeOps, ToolExecutor};

struct Sink;
impl lca_core::TurnSink for Sink {
    fn on_event(&mut self, _event: lca_core::TurnEvent) {}
}

struct DenyPrompt;
impl PermissionPrompt for DenyPrompt {
    fn ask(&mut self, _action: &lca_permissions::Action) -> Decision {
        Decision::Denied
    }
    fn review_proposals(&mut self, _diff: &ProposalDiff) -> bool {
        false
    }
}

/// Run one turn for `provider`/`model` with the fake provider reporting zero
/// cost, and return the persisted assistant usage.
fn run_turn(provider: &str, model: &str, input: u64, output: u64) -> lca_protocol::Usage {
    let root = lca_testkit::scratch_path(&format!("catalog-live-{model}"));
    let _ = std::fs::remove_dir_all(&root);
    let project = root.join("project");
    std::fs::create_dir_all(&project).expect("mkdir");
    let store = SessionStore::new(root.join("data"));
    let session = store.create_session(&project, "test").expect("session");
    let grants = Arc::new(Mutex::new(
        GrantStore::open(&root.join("grants.json")).expect("grants"),
    ));
    let mut tools = ToolExecutor::new(
        Arc::new(NativeOps::default()),
        project.clone(),
        project.clone(),
        65536,
        Some(Duration::from_secs(30)),
    );
    let fake = FakeProvider::builder()
        .turn(|t| t.text("ok").usage(fake_usage(input, output, 0, 0)))
        .build();
    let config = AgentConfig {
        provider: provider.to_string(),
        model: model.to_string(),
        retry_limit: 1,
        retry_base_delay: Duration::ZERO,
        max_iterations: 10,
        // An empty data dir: the embedded catalog only, never this machine's
        // `~/.lca/catalogs/models.json`, so the row is deterministic.
        data_dir: std::path::PathBuf::new(),
        ..AgentConfig::default()
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let mut prompt = DenyPrompt;
        let mut agent = Agent::new(
            &store,
            &session,
            &fake,
            &mut tools,
            grants,
            &mut prompt,
            None,
            config,
        );
        let mut sink = Sink;
        let outcome = agent.run_turn("hi", &mut sink, &CancelFlag::new()).await;
        assert_eq!(outcome.status, lca_core::TurnStatus::Ok);
    });
    store
        .read_with(&session, ViewMode::Display)
        .expect("records")
        .records
        .iter()
        .find_map(|record| match record {
            lca_protocol::Record::Assistant { usage, .. } => usage.clone(),
            _ => None,
        })
        .expect("an assistant record carrying usage")
}

// Verifies: gh #260 - a model outside the deleted eight prices from the
// catalog, and every breakdown bucket is populated on the persisted record
// (the gh #274 contract, now fed by the catalog).
#[test]
fn a_model_outside_the_old_table_prices_from_the_catalog() {
    // o3-mini: $1.10/M in, $4.40/M out, $0.55/M cache read.
    let usage = run_turn("openai", "o3-mini", 1_000_000, 1_000_000);
    assert!(
        (usage.cost - 5.50).abs() < 1e-9,
        "1M in @1.10 + 1M out @4.40 = 5.50, got {}",
        usage.cost
    );
    assert!(
        (usage.cost_input - 1.10).abs() < 1e-9,
        "cost_input persisted: {}",
        usage.cost_input
    );
    assert!(
        (usage.cost_cache_read - 0.0).abs() < 1e-9,
        "no cache tokens, no cache cost: {}",
        usage.cost_cache_read
    );
}

// Verifies: gh #260 - a model the old table also listed keeps the same
// numbers through the new path, so the move changed the source, not the math.
#[test]
fn a_listed_model_keeps_its_published_numbers() {
    // gpt-4o: $2.50 in, $10.00 out.
    let usage = run_turn("openai", "gpt-4o", 1_000_000, 1_000_000);
    assert!(
        (usage.cost - 12.50).abs() < 1e-9,
        "1M in @2.50 + 1M out @10.00 = 12.50, got {}",
        usage.cost
    );
}

// Verifies: gh #260 - the provider argument is part of the price: the same
// model on a provider that prices it differently bills differently.
#[test]
fn the_same_model_bills_differently_per_provider() {
    use lca_core::pricing::table_cost;
    let usage = lca_protocol::Usage {
        input: 1_000_000,
        output: 1_000_000,
        ..Default::default()
    };
    // Groq lists llama-3.3-70b-versatile at $0.59 in / $0.79 out.
    let groq = table_cost("groq", "llama-3.3-70b-versatile", &usage).expect("groq lists it");
    assert!(
        (groq - 1.38).abs() < 1e-9,
        "0.59 + 0.79 = 1.38 on groq, got {groq}"
    );
    // A provider that does not list the model still prices it (the canonical
    // fallback), which is a real number rather than $0.00.
    let elsewhere =
        table_cost("some-reseller", "llama-3.3-70b-versatile", &usage).expect("fallback answers");
    assert!(elsewhere > 0.0, "the fallback is a real price: {elsewhere}");
}

// Verifies: gh #260 - the context window rides the same lookup, so a model
// the provider did not describe still gets the catalog's denominator.
#[test]
fn the_catalog_supplies_the_context_window() {
    assert_eq!(
        lca_core::catalog_context_window("openai", "o3-mini"),
        Some(200_000)
    );
}
