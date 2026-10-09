//! GitHub issue #169: `/compact` failed exactly when it was most needed.
//!
//! The summarization call went out with **no generation budget** (the
//! endpoint's default decided when it was cut) and an open-ended prompt,
//! and a failed or empty answer degraded to the excerpt summary in
//! silence. The fix is bounded by design: an explicit budget on the
//! completion seam, a structured bounded summary prompt (pi's checkpoint
//! shape), and an honest degradation line that names the failure and the
//! recovery instead of passing an excerpt off as the model's answer.
//!
//! Verifies gh #169's three acceptance rows: a long range compacts and
//! the context measure drops; the request carries the budget and the
//! prompt carries the bound; a forced failure is surfaced honestly while
//! the compaction still lands.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use compaction_default::{Excerpt, run_compact};
use lca_core::ext_provider::{ProviderBackend, SUMMARIZATION_MAX_TOKENS};
use lca_core::{ExtensionRegistry, assemble, compact_now};
use lca_permissions::{Action, Decision, GrantStore, PermissionPrompt, ProposalDiff, ScopeRoots};
use lca_protocol::{ContentBlock, FORMAT_VERSION, Record};
use lca_session::{Session, SessionStore, ViewMode};
use lca_testkit::{FakeProvider, fake_usage};
use lca_tools::{Capabilities, CompletionBackend};

/// The one scripted summary the fake provider answers with.
const SCRIPTED: &str = "Parser rules agreed; next, wire the CLI flags.";

/// The asker the capability engine wants: nothing in this path prompts.
struct QuietPrompt;

impl PermissionPrompt for QuietPrompt {
    fn ask(&mut self, _action: &Action) -> Decision {
        Decision::Denied
    }

    fn review_proposals(&mut self, _diff: &ProposalDiff) -> bool {
        false
    }
}

/// One session, its store, the default strategy, and the completion
/// backend - everything `compact_now` takes.
struct Fixture {
    store: Arc<SessionStore>,
    session: Session,
    extensions: Arc<ExtensionRegistry>,
    backend: Arc<ProviderBackend>,
    provider: Arc<FakeProvider>,
}

fn scratch(name: &str) -> PathBuf {
    let root = lca_testkit::scratch_path(&format!("regression-gh169-{name}"));
    std::fs::create_dir_all(&root).expect("scratch root");
    root
}

fn roots(root: &Path) -> ScopeRoots {
    ScopeRoots {
        workspace: root.join("project"),
        private: root.join("private"),
        home_config: root.join("config"),
        temp: root.join("tmp"),
        state_dir: root.join("data"),
    }
}

fn fixture(name: &str, provider: FakeProvider) -> Fixture {
    let root = scratch(name);
    let project = root.join("project");
    std::fs::create_dir_all(&project).expect("mkdir");
    let provider = Arc::new(provider);
    let backend = Arc::new(ProviderBackend::new(
        provider.clone(),
        "faux-1",
        "gh169-session",
    ));
    let cap = Arc::new(Capabilities::new(
        "compaction-default",
        compaction_default::manifest_grants(),
        roots(&root),
        Arc::new(Mutex::new(QuietPrompt)),
        Arc::new(Mutex::new(
            GrantStore::open(&root.join("grants.json")).expect("grants"),
        )),
        root.join("project"),
        None,
    ));
    cap.set_completion(backend.clone());
    let mut extensions = ExtensionRegistry::new();
    extensions.register(Arc::new(compaction_default::CompactionDefault::new(cap)));
    let store = Arc::new(SessionStore::new(root.join("data")));
    let session = store.create_session(&project, "test").expect("session");
    Fixture {
        store,
        session,
        extensions: Arc::new(extensions),
        backend,
        provider,
    }
}

/// The long range: exchanges of ~3,000 characters each, big enough that
/// the wire prompt is dominated by the range itself.
fn seed(f: &Fixture, exchanges: usize) {
    let filler: String =
        "the parser rule list grows; the fixture pins the expected text ".repeat(48);
    for i in 0..exchanges {
        let ts = 1_700_000_000_000 + i as u64;
        f.store
            .append(
                &f.session,
                Record::User {
                    v: FORMAT_VERSION,
                    ts,
                    id: format!("u{i:04}"),
                    parent: None,
                    content: filler.clone(),
                    attachments: Vec::new(),
                    queue: None,
                },
            )
            .expect("append user");
        f.store
            .append(
                &f.session,
                Record::Assistant {
                    v: FORMAT_VERSION,
                    ts,
                    id: format!("a{i:04}"),
                    parent: None,
                    content: vec![ContentBlock::Text {
                        text: filler.clone(),
                    }],
                    reasoning: None,
                    reasoning_signature: None,
                    provider_thinking_level: None,
                    model: Some("faux-1".to_string()),
                    provider: Some("faux".to_string()),
                    usage: None,
                },
            )
            .expect("append assistant");
    }
}

/// The context measure the footer reports on: what a wire request would
/// carry, in characters, over the resolved (Display) view.
fn wire_chars(f: &Fixture) -> usize {
    let records = f
        .store
        .read_with(&f.session, ViewMode::Display)
        .expect("read")
        .records;
    assemble(&records, "sys")
        .messages
        .iter()
        .flat_map(|message| message.content.iter())
        .map(|block| match block {
            ContentBlock::Text { text } => text.len(),
            _ => 0,
        })
        .sum()
}

fn compact(f: &Fixture) -> String {
    compact_now(
        f.store.clone(),
        f.session.clone(),
        f.extensions.clone(),
        Some(f.backend.clone() as Arc<dyn CompletionBackend>),
        // The budget under test, not the prompt checkpoint.
        None,
    )
    .expect("compaction runs")
}

/// A short excerpt set: every record kind the prompt has to render.
fn excerpts() -> Vec<Excerpt> {
    vec![
        (
            "user".to_string(),
            r#"{"v":1,"t":"user","id":"01","content":"add a parser"}"#.to_string(),
        ),
        (
            "assistant".to_string(),
            r#"{"v":1,"t":"assistant","id":"02","content":[{"type":"text","text":"done"}]}"#
                .to_string(),
        ),
        (
            "compaction".to_string(),
            r#"{"v":1,"t":"compaction","summary":"earlier: codeword quartz-77"}"#.to_string(),
        ),
    ]
}

// Verifies gh #169 acceptance 1 (the acceptance row): a long candidate
// range compacts through the real strategy and backend, the durable
// `[compaction]` record lands with the model's summary, and the context
// measure drops to the summary's size.
#[test]
fn a_long_session_compacts_lands_the_record_and_the_context_measure_drops() {
    let provider = FakeProvider::builder()
        .turn(|t| t.text(SCRIPTED).usage(fake_usage(1200, 40, 0, 0)))
        .build();
    let f = fixture("acceptance", provider);
    seed(&f, 60);

    let before = wire_chars(&f);
    let summary = compact(&f);
    assert_eq!(summary, SCRIPTED, "the model's answer is the summary");
    let after = wire_chars(&f);
    assert!(
        after * 2 < before,
        "the context measure drops: {before} -> {after} chars"
    );

    let records = f
        .store
        .read_with(&f.session, ViewMode::Display)
        .expect("read")
        .records;
    let named: Vec<&str> = records
        .iter()
        .filter_map(|record| match record {
            Record::Compaction { strategy, .. } => Some(strategy.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        named,
        vec!["compaction-default"],
        "the durable compaction record landed"
    );
}

// Verifies gh #169 acceptance 2 (bounded by design, request seam): the
// summarization request carries an explicit generation budget - the
// documented default - instead of inheriting whatever the endpoint
// chooses when it is never told.
#[test]
fn the_summarization_request_carries_an_explicit_generation_budget() {
    let provider = FakeProvider::builder()
        .turn(|t| t.text(SCRIPTED).usage(fake_usage(800, 40, 0, 0)))
        .build();
    let f = fixture("budget", provider);
    seed(&f, 2);

    compact(&f);

    let requests = f.provider.requests();
    let last = requests
        .last()
        .expect("the summarization request reached the provider");
    let budget = last
        .extras
        .get("max-tokens")
        .expect("gh #169: the request carries a generation budget");
    assert_eq!(
        budget,
        &SUMMARIZATION_MAX_TOKENS.to_string(),
        "the documented summarization budget rides the request extras"
    );
}

// Verifies gh #169 acceptance 2 (bounded by design, prompt seam): the
// summary prompt is pi's structured checkpoint shape - named sections
// that self-limit - with an explicit word bound, so the model stops
// well inside the budget instead of rambling into whatever cap exists.
#[test]
fn the_summary_prompt_is_bounded_and_structured() {
    let prompt = std::cell::RefCell::new(String::new());
    let summary = run_compact(
        &excerpts(),
        Some(&|seen: &str| {
            prompt.borrow_mut().push_str(seen);
            Ok("MODEL SUMMARY".to_string())
        }),
    );
    assert_eq!(summary, "MODEL SUMMARY");
    let prompt = prompt.into_inner();
    for section in [
        "## Goal",
        "## Constraints & Preferences",
        "## Progress",
        "## Key Decisions",
        "## Next Steps",
        "## Critical Context",
    ] {
        assert!(prompt.contains(section), "structured checkpoint: {section}");
    }
    assert!(
        prompt.contains("under 300 words"),
        "the bound is spelled out (and no eaten space): {prompt}"
    );
    assert!(
        prompt.contains("Reply with the summary only"),
        "summary only, no commentary: {prompt}"
    );
}

// Verifies gh #169 acceptance 3 (honest failure): a failed or empty
// generation still compacts - the strategy never fails a turn
// (ADR-0015) - but the result names the failure (here the cap, with the
// number) and the recovery instead of silently degrading.
#[test]
fn a_failed_summarization_names_the_failure_and_still_compacts() {
    let failed = run_compact(
        &excerpts(),
        Some(&|_| {
            Err(
                "completion failed: generation hit the token cap (max_tokens 4096) before \
                 finishing; the response is incomplete"
                    .to_string(),
            )
        }),
    );
    assert!(
        failed.contains("token cap") && failed.contains("4096"),
        "the cap is named with its number: {failed}"
    );
    assert!(
        failed.contains("/compact"),
        "the recovery is spelled out: {failed}"
    );
    assert!(
        failed.contains("add a parser") && failed.contains("quartz-77"),
        "the compaction still lands on the excerpt summary: {failed}"
    );

    let empty = run_compact(&excerpts(), Some(&|_| Ok("   ".to_string())));
    assert!(
        empty.contains("empty summary"),
        "an empty generation is named too: {empty}"
    );
    assert!(
        empty.contains("add a parser"),
        "and still degrades instead of refusing: {empty}"
    );

    // No capability at all is not a failure: the mechanical path stays
    // quiet and complete (ADR-0015's separability).
    let absent = run_compact(&excerpts(), None);
    assert!(
        !absent.contains("summarization failed"),
        "no attempt, no failure line: {absent}"
    );
    assert!(absent.contains("quartz-77"), "{absent}");
}
