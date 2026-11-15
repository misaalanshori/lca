//! Gh #601: the branch summary's **core** rows - the verb, its record, and the
//! one budget it shares with `/compact`.
//!
//! The strategy is a counting double (`SeeingDouble`, the compaction suite's own)
//! that answers with a named summary, so each row asserts what reached the
//! extension rather than "a function returned Ok".

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use lca_ext_abi::{DeliveryMode, DispatchFuture, ExtensionDispatch, World};
use lca_protocol::DispatchError;
use lca_session::ViewMode;

/// A compaction-world double that counts calls and can be told to fail.
///
/// A double rather than a fake provider because the verb's contract is with the
/// **strategy** (FR-SESS-5: there is no built-in summarizing path), and the
/// budget is the backend's, asserted separately through the real
/// `ProviderBackend`.
struct StrategyDouble {
    calls: Arc<AtomicUsize>,
    fail: bool,
}

impl ExtensionDispatch for StrategyDouble {
    fn name(&self) -> &str {
        "branch-double"
    }

    fn delivery(&self) -> DeliveryMode {
        DeliveryMode::Native
    }

    fn worlds(&self) -> Vec<World> {
        vec![World::Compaction]
    }

    fn tool_specs(&self) -> Result<Vec<lca_protocol::ToolSpec>, DispatchError> {
        Ok(Vec::new())
    }

    fn execute_tool<'a>(
        &'a self,
        _call: &'a lca_protocol::ToolCall,
    ) -> DispatchFuture<'a, Result<lca_protocol::ToolResult, DispatchError>> {
        Box::pin(std::future::ready(Err(DispatchError::MissingWorld {
            extension: self.name().to_string(),
            world: "tool",
        })))
    }

    fn command_specs(&self) -> Result<Vec<lca_protocol::CommandSpec>, DispatchError> {
        Ok(Vec::new())
    }

    fn invoke_command(
        &self,
        _name: &str,
        _argument: &str,
    ) -> Result<lca_protocol::CommandEffect, DispatchError> {
        Ok(lca_protocol::CommandEffect::None)
    }

    fn compact(
        &self,
        records: &[lca_protocol::Record],
    ) -> DispatchFuture<'static, Result<String, DispatchError>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.fail {
            return Box::pin(std::future::ready(Err(DispatchError::Failed(
                "the provider refused (HTTP 500)".to_string(),
            ))));
        }
        let summary = format!("abandoned: {} record(s) of work", records.len());
        Box::pin(std::future::ready(Ok(summary)))
    }
}

fn registry_with(calls: Arc<AtomicUsize>, fail: bool) -> lca_core::ExtensionRegistry {
    let mut registry = lca_core::ExtensionRegistry::new();
    registry.register(Arc::new(StrategyDouble { calls, fail }));
    registry
}

fn seeded(name: &str) -> (Arc<lca_session::SessionStore>, lca_session::Session) {
    let root = lca_testkit::scratch_path(&format!("gh601-{name}"));
    let store = Arc::new(lca_session::SessionStore::new(root));
    let project = lca_testkit::scratch_path(&format!("gh601-{name}-project"));
    let session = store.create_session(&project, "test").expect("create");
    (store, session)
}

fn user(id: &str, content: &str) -> lca_protocol::Record {
    lca_protocol::Record::User {
        v: lca_protocol::FORMAT_VERSION,
        ts: 1,
        id: id.to_string(),
        parent: None,
        content: content.to_string(),
        attachments: vec![],
        queue: None,
    }
}

fn branch_summaries(
    store: &lca_session::SessionStore,
    session: &lca_session::Session,
) -> Vec<lca_protocol::Record> {
    store
        .resolved(session, ViewMode::Audit)
        .expect("audit")
        .records
        .into_iter()
        .filter(|record| matches!(record, lca_protocol::Record::BranchSummary { .. }))
        .collect()
}

// Verifies: gh #601 row 1 - **one `branch-summary` record lands, `from_id` is the
// abandoned tip and `parent` the navigation point**, and the abandoned path is
// what the strategy saw. Before this cycle the verb did not exist: nothing in
// `lca-cli` or `lca-ui` ever asked for a summary, so `s` did nothing.
#[test]
fn a_branch_summary_appends_one_record_naming_the_abandoned_path() {
    let (store, session) = seeded("verb");
    store.append(&session, user("u1", "first")).expect("u1");
    store
        .append(&session, user("u2", "the path being left"))
        .expect("u2");
    store
        .append(&session, user("u3", "and its continuation"))
        .expect("u3");
    let calls = Arc::new(AtomicUsize::new(0));
    let summary = lca_core::summarize_branch_now(
        store.clone(),
        session.clone(),
        Arc::new(registry_with(calls.clone(), false)),
        None,
        "u1",
    )
    .expect("summarized");
    assert_eq!(calls.load(Ordering::SeqCst), 1, "exactly one strategy call");
    assert!(!summary.is_empty());

    let records = branch_summaries(&store, &session);
    assert_eq!(records.len(), 1, "exactly one record, never two");
    let lca_protocol::Record::BranchSummary {
        from_id, parent, ..
    } = &records[0]
    else {
        panic!("not a branch summary");
    };
    assert_eq!(
        parent.as_deref(),
        Some("u1"),
        "the navigation point is the summary's parent"
    );
    assert!(
        from_id.as_deref() != Some("u1"),
        "the abandoned tip is what was left, not the point we navigated to: {from_id:?}"
    );
    // The abandoned path is what the strategy saw: u2 and u3, not u1.
    let shown = store
        .resolved(&session, ViewMode::Display)
        .expect("display");
    assert!(
        !shown.records.iter().any(|record| record.id() == Some("u2")),
        "the abandoned path is out of the live view"
    );
    assert!(
        shown
            .records
            .iter()
            .any(|record| matches!(record, lca_protocol::Record::BranchSummary { summary, .. } if !summary.is_empty())),
        "and the summary is in it"
    );
}

// Verifies: gh #601 row 3's core half - **a failed summary writes no record and
// names the error.** The error text carries the strategy's own detail, which is
// what the interface shows as a fault line (gh #594/#169's rule: never a silent
// absence of the record).
#[test]
fn a_failed_strategy_writes_no_record_and_names_the_error() {
    let (store, session) = seeded("fail");
    store.append(&session, user("u1", "first")).expect("u1");
    store.append(&session, user("u2", "abandoned")).expect("u2");
    let calls = Arc::new(AtomicUsize::new(0));
    let error = lca_core::summarize_branch_now(
        store.clone(),
        session.clone(),
        Arc::new(registry_with(calls.clone(), true)),
        None,
        "u1",
    )
    .expect_err("the strategy failed");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(
        error.contains("HTTP 500") && error.contains("branch summary failed"),
        "the error names the provider failure: {error}"
    );
    assert!(
        branch_summaries(&store, &session).is_empty(),
        "and nothing was recorded, so nothing claims a summary exists"
    );
}

// Verifies: gh #601 - **nothing after the navigation point means nothing to
// summarize.** A tip is a valid target and a no-op verb; the refusal names why
// rather than writing an empty summary.
#[test]
fn the_live_tip_has_nothing_to_summarize() {
    let (store, session) = seeded("tip");
    store.append(&session, user("u1", "first")).expect("u1");
    store.append(&session, user("u2", "the tip")).expect("u2");
    let calls = Arc::new(AtomicUsize::new(0));
    let error = lca_core::summarize_branch_now(
        store.clone(),
        session.clone(),
        Arc::new(registry_with(calls, false)),
        None,
        "u2",
    )
    .expect_err("the tip has no abandoned path");
    assert!(error.contains("nothing to summarize"), "{error}");
}

// Verifies: gh #601 - **a missing record is a refusal, not a panic and not a
// summary of the wrong range.** `summarize_branch` refuses a missing target too;
// the verb names it before the strategy is ever called.
#[test]
fn an_unknown_record_is_refused_before_the_strategy_runs() {
    let (store, session) = seeded("missing");
    store.append(&session, user("u1", "first")).expect("u1");
    let calls = Arc::new(AtomicUsize::new(0));
    let error = lca_core::summarize_branch_now(
        store.clone(),
        session.clone(),
        Arc::new(registry_with(calls.clone(), false)),
        None,
        "nope",
    )
    .expect_err("unknown record");
    assert!(error.contains("no record with id nope"), "{error}");
    assert_eq!(calls.load(Ordering::SeqCst), 0, "the strategy never ran");
}

// Verifies: gh #601 row 5 - **the summary call carries the session's
// summarization budget, and it is the number `/compact` gets.** Both verbs ask
// the same object (`CompletionRequest.extras["max-tokens"]`), which the
// `ProviderBackend` stamps from
// `ext_provider::summarization_budget` for the live model window. The row drives
// the **real** backend twice - once for each verb - and compares the two
// requests, so a second budget derivation inside either command arm would show
// up as a mismatch.
#[test]
fn both_verbs_send_the_same_summarization_budget() {
    use lca_core::ext_provider::{
        ProviderBackend, SUMMARIZATION_MAX_TOKENS, summarization_budget, summarization_max_tokens,
    };
    use lca_tools::CompletionBackend as _;

    let (store, session) = seeded("budget");
    store.append(&session, user("u1", "first")).expect("u1");
    store.append(&session, user("u2", "abandoned")).expect("u2");

    let fake = Arc::new(
        lca_testkit::FakeProvider::builder()
            .turn(|turn| {
                turn.text("checkpoint")
                    .usage(lca_testkit::fake_usage(10, 2, 0, 0))
            })
            .turn(|turn| {
                turn.text("compacted")
                    .usage(lca_testkit::fake_usage(12, 3, 0, 0))
            })
            .build(),
    );
    let backend = Arc::new(ProviderBackend::new(fake.clone(), "m", "sess-601"));
    // What the interface does on every model change (gh #594): derive from the
    // window, not from a hard-coded stopgap.
    let window = 200_000_u32;
    let derived = summarization_budget(0.8, 0, window, 0);
    backend.set_summarization_budget(derived);
    assert_eq!(derived, 32_000, "the 200k window's derivation");

    // The strategy must be the real one to reach the backend: a double would
    // never call it, and the row would pass while asserting nothing about the
    // request `/compact` and this verb actually share.
    let cap = Arc::new(lca_tools::Capabilities::new(
        "compaction-default",
        compaction_default::manifest_grants(),
        scope_roots("gh601-budget"),
        Arc::new(std::sync::Mutex::new(GrantPrompt {
            answers: Vec::new(),
            asked: Vec::new(),
        })),
        Arc::new(std::sync::Mutex::new(
            lca_permissions::GrantStore::open(
                &lca_testkit::scratch_path("gh601-budget-grants").join("grants.json"),
            )
            .expect("grants"),
        )),
        lca_testkit::scratch_path("gh601-budget-project"),
        None,
    ));
    cap.set_completion(backend.clone());
    let mut registry = lca_core::ExtensionRegistry::new();
    registry.register(Arc::new(compaction_default::CompactionDefault::new(
        cap.clone(),
    )));

    // Verb one: the branch summary, through the same backend.
    lca_core::summarize_branch_now(
        store.clone(),
        session.clone(),
        Arc::new(registry),
        Some(backend.clone()),
        "u1",
    )
    .expect("the branch summary ran");
    let branch_request = fake.last_request().expect("the strategy asked the backend");

    // Verb two: an ordinary completion through the same backend, which is
    // exactly what `/compact`'s strategy does with it.
    let messages = vec![lca_protocol::ChatMessage::text(
        lca_protocol::MessageRole::User,
        "compact this",
    )];
    backend.complete(&messages).expect("answers");
    let compact_request = fake.last_request().expect("the second call landed");

    let budget_of = |extras: &std::collections::BTreeMap<String, String>| {
        extras
            .get("max-tokens")
            .cloned()
            .unwrap_or_else(|| "missing".to_string())
    };
    assert_eq!(
        budget_of(&branch_request.extras),
        derived.to_string(),
        "the branch summary carries the session's derived budget"
    );
    assert_eq!(
        budget_of(&compact_request.extras),
        budget_of(&branch_request.extras),
        "and `/compact` gets the same number - one budget, not two"
    );
    assert_ne!(
        budget_of(&branch_request.extras),
        SUMMARIZATION_MAX_TOKENS.to_string(),
        "not the stopgap a front end that forgot to derive would send"
    );
    // A tiny window still derives its own (smaller) number, which is the whole
    // reason the stopgap is wrong: the same call on a small model must not ask
    // for 160k.
    assert_eq!(summarization_max_tokens(6_400), 5_120);
}

/// The capability engine's prompt double (the compaction suite's own shape):
/// nothing here answers, because no tool is called.
struct GrantPrompt {
    answers: Vec<String>,
    asked: Vec<String>,
}

impl lca_permissions::PermissionPrompt for GrantPrompt {
    fn ask(&mut self, action: &lca_permissions::Action) -> lca_permissions::Decision {
        self.asked.push(format!("{action:?}"));
        if self.answers.iter().any(|answer| answer == "allow") {
            lca_permissions::Decision::Once
        } else {
            lca_permissions::Decision::Denied
        }
    }

    fn review_proposals(&mut self, _diff: &lca_permissions::ProposalDiff) -> bool {
        false
    }
}

/// The capability engine's scope roots for a scratch name (the compaction
/// suite's own shape, so the strategy's grants resolve the same way).
fn scope_roots(name: &str) -> lca_permissions::ScopeRoots {
    let root = lca_testkit::scratch_path(name);
    lca_permissions::ScopeRoots {
        workspace: root.join("project"),
        private: root.join("private"),
        home_config: root.join("config"),
        temp: root.join("tmp"),
        state_dir: root.join("data"),
    }
}

/// A backend already holding usage **and** a strategy that fails: the shape a real
/// failure takes (the call was made and billed, then something refused), without a nested
/// runtime. `complete` is never reached here because the double fails before asking.
struct PreArmedBackend {
    usage: std::sync::Mutex<Option<lca_protocol::Usage>>,
}

impl lca_tools::CompletionBackend for PreArmedBackend {
    fn complete(
        &self,
        _messages: &[lca_protocol::ChatMessage],
    ) -> Result<(String, lca_protocol::Usage), lca_tools::CompletionError> {
        Err(lca_tools::CompletionError::Provider("unused".to_string()))
    }

    fn take_usage(&self) -> Option<lca_protocol::Usage> {
        self.usage.lock().unwrap_or_else(|p| p.into_inner()).take()
    }
}

#[test]
fn a_failed_summary_still_drains_the_backend() {
    use lca_tools::CompletionBackend as _;
    let (store, session) = seeded("failed-drain");
    store.append(&session, user("u1", "first")).expect("u1");
    store.append(&session, user("u2", "abandoned")).expect("u2");

    // A backend that already holds the failed call's spend, and a strategy that fails.
    let backend = Arc::new(PreArmedBackend {
        usage: std::sync::Mutex::new(Some(lca_protocol::Usage {
            input: 10,
            output: 2,
            ..Default::default()
        })),
    });
    let calls = Arc::new(AtomicUsize::new(0));
    let error = lca_core::summarize_branch_now(
        store.clone(),
        session.clone(),
        Arc::new(registry_with(calls.clone(), true)),
        Some(backend.clone()),
        "u1",
    )
    .expect_err("the strategy failed");
    assert!(error.contains("branch summary failed"), "{error}");
    assert_eq!(calls.load(Ordering::SeqCst), 1, "the call was made");
    assert!(
        backend.take_usage().is_none(),
        "the failed call's usage was drained, so the next /compact cannot claim it"
    );
}
