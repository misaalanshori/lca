//! Regression rows for gh #275: the stable-prefix boundary must be computed
//! on the post-transform message list and must keep advancing after a
//! compaction.
//!
//! Two defects, both in `turn/mod.rs`:
//!
//! 1. `stable_cap` ran on `assembled.messages`, *before* `extensions.transform`,
//!    the skills merge, and the instruction deltas. A transform that inserts a
//!    message before this turn's user message shifts every index after it, so
//!    the cap pointed at the wrong message.
//! 2. `stable_cap` clamped to `assembled.stable_prefix` whenever a compaction
//!    had ever been seen. `assemble` sets that to the summary's position, so
//!    the boundary froze there *permanently*: every turn after a compaction
//!    claimed only `[system, summary]` and the whole live conversation was
//!    un-cacheable.
//!
//! There was a third, quieter one: `sent_stable` recorded the full outgoing
//! list, including per-turn injections appended after the user message. Those
//! are ephemeral (FR-CTX-4 never logs them), so on the next turn a *different*
//! message sat at that index and the boundary read as diverged.
//!
//! Verifies: gh #275 - the boundary tracks the real conversation across and
//! after a compaction, and an ephemeral tail does not narrow it.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lca_core::{Agent, AgentConfig, ExtensionRegistry, TurnEvent, TurnSink};
use lca_permissions::{Decision, GrantStore, PermissionPrompt, ProposalDiff};
use lca_protocol::Record;
use lca_session::{SessionStore, ViewMode};
use lca_testkit::{FakeProvider, fake_usage};
use lca_tools::{CancelFlag, NativeOps, ToolExecutor};

use crate::doubles_275::{Double, TransformKind};

#[derive(Default)]
struct Sink(Vec<TurnEvent>);

impl TurnSink for Sink {
    fn on_event(&mut self, event: TurnEvent) {
        self.0.push(event);
    }
}

struct Prompt;

impl PermissionPrompt for Prompt {
    fn ask(&mut self, _action: &lca_permissions::Action) -> Decision {
        Decision::Denied
    }
    fn review_proposals(&mut self, _diff: &ProposalDiff) -> bool {
        false
    }
}

fn scratch(name: &str) -> PathBuf {
    let dir = lca_testkit::scratch_path(&format!("regression-275-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("mkdir");
    dir
}

/// Build a harness whose threshold crossing fires on turn 2 (as
/// `loop_phases.rs` does) so the post-compaction turns can be inspected.
struct Harness {
    store: SessionStore,
    session: lca_session::Session,
    provider: FakeProvider,
    tools: ToolExecutor,
    grants: Arc<Mutex<GrantStore>>,
    config: AgentConfig,
}

async fn harness(name: &str, registry: ExtensionRegistry) -> Harness {
    let root = scratch(name);
    let project = root.join("project");
    std::fs::create_dir_all(&project).expect("mkdir");
    let store = SessionStore::new(root.join("data"));
    let session = store.create_session(&project, "test").expect("session");
    let grants = Arc::new(Mutex::new(
        GrantStore::open(&root.join("grants.json")).expect("grants"),
    ));
    let tools = ToolExecutor::new(
        Arc::new(NativeOps::default()),
        project.clone(),
        project.clone(),
        65536,
        Some(Duration::from_secs(30)),
    );
    // The threshold fires when the *previous* assistant record's prompt
    // exceeds `window - reserve` (0.5 of 10_000, so 5_000). The usage is
    // shaped so exactly one compaction happens: turn 2 reports a big prompt,
    // which makes turn 3 compact, and turns 3+ report small ones so nothing
    // re-fires. A uniform large prompt would compact on every single turn -
    // which is how an earlier draft of this file ended up with the boundary
    // pinned at 1 for a reason that had nothing to do with gh #275.
    let provider = FakeProvider::builder()
        .turn(|t| t.text("ok").usage(fake_usage(100, 10, 0, 0)))
        .turn(|t| t.text("ok").usage(fake_usage(6_000, 10, 0, 0)))
        .turn(|t| t.text("ok").usage(fake_usage(100, 10, 0, 0)))
        .turn(|t| t.text("ok").usage(fake_usage(100, 10, 0, 0)))
        .turn(|t| t.text("ok").usage(fake_usage(100, 10, 0, 0)))
        .build();
    let config = AgentConfig {
        provider: "fake".to_string(),
        model: "faux-1".to_string(),
        retry_limit: 3,
        retry_base_delay: Duration::ZERO,
        max_iterations: 50,
        model_context_window: 10_000,
        compaction_threshold: 0.5,
        compaction_reserve_tokens: 0,
        compaction_keep_recent_tokens: 0,
        extensions: Arc::new(registry),
        ..AgentConfig::default()
    };
    Harness {
        store,
        session,
        provider,
        tools,
        grants,
        config,
    }
}

async fn run_turn(h: &mut Harness, input: &str) {
    let mut sink = Sink::default();
    let mut prompt = Prompt;
    let mut agent = Agent::new(
        &h.store,
        &h.session,
        &h.provider,
        &mut h.tools,
        h.grants.clone(),
        &mut prompt,
        None,
        h.config.clone(),
    );
    let outcome = agent.run_turn(input, &mut sink, &CancelFlag::new()).await;
    assert_eq!(
        outcome.status,
        lca_core::TurnStatus::Ok,
        "{input} failed: {:?}",
        outcome.error
    );
}

fn count_narrowed(h: &Harness) -> usize {
    h.store
        .read_with(&h.session, ViewMode::Display)
        .expect("records")
        .records
        .iter()
        .filter(|record| {
            matches!(record, Record::ExtensionEvent { event, .. } if event == "cache-boundary-narrowed")
        })
        .count()
}

/// The epicentre of the defect: once a compaction has landed, later turns must
/// keep advancing. Before the fix the boundary froze at the summary for the
/// rest of the session, so everything after it was permanently un-cacheable.
///
/// Shape: the threshold first fires on turn 3, whose own user record is
/// already in the log when `maybe_compact` runs, so the summary lands *after*
/// it (`[sys, third, summary]`). Turn 4 is therefore the first turn with a
/// conversation ahead of the summary, and that is where the old freeze bit.
#[test]
fn the_boundary_keeps_advancing_after_a_compaction() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let mut registry = ExtensionRegistry::new();
        registry.register(Arc::new(Double::summarizer()));
        let mut h = harness("advance", registry).await;

        run_turn(&mut h, "first").await;
        assert_eq!(
            h.provider
                .last_request()
                .expect("request one")
                .stable_prefix,
            1,
            "turn 1 claims only the system prompt"
        );
        run_turn(&mut h, "second").await;

        // Turn 3 crosses the threshold.
        run_turn(&mut h, "third").await;
        let compactions = h
            .store
            .read_with(&h.session, ViewMode::Display)
            .expect("records")
            .records
            .iter()
            .filter(|record| matches!(record, Record::Compaction { .. }))
            .count();
        assert_eq!(
            compactions, 1,
            "the threshold must actually have compacted, or the rows below mean nothing"
        );
        let third = h.provider.last_request().expect("request three");
        assert!(
            third.stable_prefix < third.messages.len(),
            "the compacting turn never claims its own message: {} of {}",
            third.stable_prefix,
            third.messages.len()
        );

        // Turn 4: the first turn with a real conversation ahead of the
        // summary. It must claim more than the turn before it.
        run_turn(&mut h, "fourth").await;
        let fourth = h.provider.last_request().expect("request four");
        assert!(
            fourth.stable_prefix > third.stable_prefix,
            "the boundary advances across the post-compaction conversation \
             (was frozen at the summary): {} -> {}",
            third.stable_prefix,
            fourth.stable_prefix
        );
        assert!(
            fourth.stable_prefix < fourth.messages.len(),
            "and still never claims this turn's own message: {} of {}",
            fourth.stable_prefix,
            fourth.messages.len()
        );

        // Turn 5 keeps growing - the freeze used to hold it forever.
        run_turn(&mut h, "fifth").await;
        let fifth = h.provider.last_request().expect("request five");
        assert!(
            fifth.stable_prefix > fourth.stable_prefix,
            "growth continues: {} -> {}",
            fourth.stable_prefix,
            fifth.stable_prefix
        );
        assert_eq!(
            count_narrowed(&h),
            0,
            "nothing diverged, so nothing is narrowed"
        );
    });
}

/// A per-turn injection appended after the user message must not narrow the
/// boundary on the next turn. Before the fix `sent_stable` stored that
/// ephemeral message, so next turn's assistant reply at the same index read
/// as a rewrite.
#[test]
fn an_ephemeral_tail_injection_does_not_narrow_the_boundary() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let mut registry = ExtensionRegistry::new();
        registry.register(Arc::new(Double::transformer(
            "tail-inject",
            TransformKind::AppendTail("[skill: test-skill] Follow the skill."),
        )));
        let mut h = harness("ephemeral", registry).await;

        run_turn(&mut h, "first").await;
        let first = h.provider.last_request().expect("request one");
        assert!(
            first
                .messages
                .iter()
                .any(|m| m.content.iter().any(|block| matches!(
                    block,
                    lca_protocol::ContentBlock::Text { text } if text.contains("[skill: test-skill]")
                ))),
            "the injection rode in the request"
        );
        let first_prefix = first.stable_prefix;

        run_turn(&mut h, "second").await;
        let second = h.provider.last_request().expect("request two");
        // The injection was never claimed as stable, so the boundary grew
        // across the messages that *were* sent.
        assert!(
            second.stable_prefix >= first_prefix,
            "the boundary grew: {first_prefix} -> {}",
            second.stable_prefix
        );
        assert_eq!(
            count_narrowed(&h),
            0,
            "an ephemeral tail is not a divergence: the previous turn's injection \
             is simply not at that index this turn"
        );

        run_turn(&mut h, "third").await;
        assert_eq!(
            count_narrowed(&h),
            0,
            "and it stays quiet on every later turn"
        );
        let third = h.provider.last_request().expect("request three");
        assert!(
            third.stable_prefix > second.stable_prefix,
            "the boundary keeps advancing: {} -> {}",
            second.stable_prefix,
            third.stable_prefix
        );
    });
}

/// Without any transform or compaction the cap is simply the current turn's
/// user message: the behaviour the fix must preserve.
#[test]
fn a_plain_conversation_still_grows_by_one_turn() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let mut h = harness("plain", ExtensionRegistry::new()).await;
        run_turn(&mut h, "first").await;
        assert_eq!(
            h.provider.last_request().expect("one").stable_prefix,
            1,
            "system only"
        );
        run_turn(&mut h, "second").await;
        assert_eq!(
            h.provider.last_request().expect("two").stable_prefix,
            2,
            "system + turn 1's user message"
        );
        run_turn(&mut h, "third").await;
        assert_eq!(
            h.provider.last_request().expect("three").stable_prefix,
            4,
            "system + turn 1 (user, assistant) + turn 2's user message"
        );
        assert_eq!(count_narrowed(&h), 0);
    });
}

/// A diverging prefix still narrows once and records one event: the fix must
/// not disable FR-CACHE-6's guard while removing the freeze.
#[test]
fn a_real_divergence_still_narrows_and_records_once() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let mut registry = ExtensionRegistry::new();
        registry.register(Arc::new(Double::transformer(
            "rewriter",
            TransformKind::RewriteSystemOnSecondCall,
        )));
        let mut h = harness("diverge", registry).await;

        run_turn(&mut h, "first").await;
        assert_eq!(
            count_narrowed(&h),
            0,
            "turn 1 sends the system prompt clean, nothing to compare yet"
        );

        // Turn 2 rewrites the system prompt. The previous request sent it
        // clean, so the boundary ends before index 0 - there is nothing
        // stable left. That is one real divergence.
        run_turn(&mut h, "second").await;
        let second = h.provider.last_request().expect("request two");
        assert_eq!(
            second.stable_prefix, 0,
            "the rewritten message is index 0, so nothing before it is stable"
        );
        assert_eq!(count_narrowed(&h), 1, "the rewrite is one divergence");

        // Turn 3 restores the clean text. That is a *second* real
        // divergence - the previous request did send the rewrite - so the
        // event count is 2, not 1. A test expecting 1 here would be
        // asserting that a genuine rewrite went unreported.
        run_turn(&mut h, "third").await;
        assert_eq!(count_narrowed(&h), 2, "the restore is its own divergence");

        // Turn 4 is stable: nothing changed, so nothing new is recorded.
        run_turn(&mut h, "fourth").await;
        assert_eq!(
            count_narrowed(&h),
            2,
            "settled: no repeat event once the content stops changing"
        );
    });
}
