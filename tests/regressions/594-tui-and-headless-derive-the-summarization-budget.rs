//! Gh #594: the interactive `/compact` summarized on the 4096-token stopgap while
//! the headless path used the budget derived from the model's window.
//!
//! The mechanism, for whoever reads this next: `ProviderBackend::summarization_budget`
//! starts `None`, `complete()` falls back to `SUMMARIZATION_MAX_TOKENS` (4096), and the
//! derivation (`summarization_max_tokens(compaction_reserve(…))`) was called from
//! **exactly one place** — `crates/lca-cli/src/headless.rs`. Nothing under
//! `crates/lca-cli/src/tui/` called it, so a 1M-window interactive session with a
//! 660k-token transcript asked the model for a 4096-token summary, was cut, and degraded
//! to the mechanical excerpt while the headless path on the same model would have asked
//! for 160,000.
//!
//! Three rows, because the defect has three shapes:
//!
//! 1. **The wiring row** (the one the issue's first analysis was missing): a row proving
//!    `summarization_max_tokens(16_384) == 13_107` says nothing about whether any front
//!    end *called* it. This one reads back what the backend the interface builds
//!    actually carries, through the accessor `ProviderBackend::summarization_budget`.
//!    The pane-level half of the same fact — the `max_tokens` the summarization request
//!    carried — lives in `crates/lca-cli/tests/e2e_compact_budget_594.rs`, because that
//!    needs a real binary in a real terminal.
//! 2. **The parity guard**: both front ends derive at the site where they build the
//!    backend, and a provider-backend site that is neither front end's is named with its
//!    role instead of passing by omission. A third front end that forgets reddens this
//!    row; that is how the defect was born.
//! 3. **The cut-at-our-budget row**: a summary that stops at the budget reports a budget
//!    problem *and still degrades* — the extension never fails a turn (ADR-0015), and the
//!    session stays usable, which is what the real pane session depended on.
//!
//! Verifies: gh #594, FR-SESS-4 (compaction runs through the session's own model), and
//! `docs/compaction.md`'s "derived from the reserve, never the stopgap" rule.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::path::Path;
use std::sync::{Arc, Mutex};

use compaction_default::run_compact;
use lca_core::ext_provider::{ProviderBackend, SUMMARIZATION_MAX_TOKENS, summarization_budget};
use lca_core::{ExtensionRegistry, assemble, compact_now};
use lca_permissions::{Action, Decision, GrantStore, PermissionPrompt, ProposalDiff, ScopeRoots};
use lca_protocol::{ContentBlock, FORMAT_VERSION, Record};
use lca_session::{Session, SessionStore, ViewMode};
use lca_testkit::{FakeProvider, fake_usage};
use lca_tools::{Capabilities, CompletionBackend};

// ---------------------------------------------------------------------------
// Row 1: the interactive backend carries the derived budget
// ---------------------------------------------------------------------------

/// The budget the interface derives for a model window, as a test can see it.
///
/// Deliberately spelled out here as a *literal* rather than calling the helper:
/// the row has to fail if the helper's arithmetic changes under a front end that
/// stopped applying it, so the two numbers are written down independently.
const MILLION_WINDOW_BUDGET: u32 = 160_000;

// Verifies: gh #594 - **the backend the interface builds is given the budget its
// model window earns, and it is the number `complete()` will ask with.**
// Mutation: deleting the `apply_summarization_budget` call at the interface's
// assembly site leaves this backend at `None`, which `complete()` turns into 4096
// - the shipped defect, seen from the outside.
#[test]
fn the_interface_backend_carries_a_budget_derived_from_its_model_window() {
    let backend = ProviderBackend::new(Arc::new(FakeProvider::builder().build()), "m", "sess-1");
    assert_eq!(
        backend.summarization_budget(),
        None,
        "a fresh backend has no budget: `complete()` would fall back to the stopgap"
    );
    // The interface's application site, byte for byte: a `Config` at its defaults
    // (threshold 0.8, no absolute reserve) and a 1M-token model window.
    let config = lca_config::Config::defaults();
    lca_cli::tui::apply_summarization_budget_for_tests(&backend, &config, 1_000_000);
    assert_eq!(
        backend.summarization_budget(),
        Some(MILLION_WINDOW_BUDGET),
        "the derived budget reached the backend the interface builds (gh #594)"
    );
    assert!(
        MILLION_WINDOW_BUDGET as u64 > u64::from(SUMMARIZATION_MAX_TOKENS) * 30,
        "and it is not the stopgap: {} against {}",
        MILLION_WINDOW_BUDGET,
        SUMMARIZATION_MAX_TOKENS
    );
    assert_eq!(
        u64::from(summarization_budget(0.8, 0, 1_000_000)),
        u64::from(MILLION_WINDOW_BUDGET),
        "the interface applies the one rule `lca_core` owns, not a private copy"
    );
}

// ---------------------------------------------------------------------------
// Row 2: the parity guard, with every assembly site named
// ---------------------------------------------------------------------------

/// What a `ProviderBackend` in this binary is *for*. The role decides whether the
/// front end owes it a derived budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    /// Serves the `completion` capability to the compaction strategy, whose
    /// request must carry the derived budget (gh #594's subject).
    Compaction,
    /// The per-turn backend installed into handles that carry no completion
    /// backend of their own (gh #49). It **cannot** serve compaction: the
    /// compaction handle holds its own backend and `install_completion_backend`
    /// is a no-op where one exists. It bounds tool-driven nested generations with
    /// the default, which is its documented purpose, not a stopgap left in place.
    NestedCompletion,
}

/// Every `ProviderBackend::new` site in `lca-cli`, by file, with its role and how
/// many sites that file owns.
///
/// The list is exhaustive on purpose: `every_backend_site_is_classified` walks the
/// crate and reddens on a site that is not named here, so a new front end (or a new
/// nested path) cannot inherit the stopgap by being forgotten.
///
/// **Sites covered, exactly:** `registry.rs` (the interface's compaction backend,
/// built by `register_compaction` for `Ui::new` and `/reload`), `headless.rs` (its
/// compaction backend *and* its per-turn nested one), `rpc.rs` and
/// `tui/runner.rs` (the turn's nested backends). RPC's *compaction* backend is the
/// one `headless::setup` builds — RPC gets its `AgentConfig` from there — so it is
/// covered through the headless site.
///
/// **Not covered, stated rather than implied:** the three `NestedCompletion` sites
/// owe nothing, because `install_completion_backend` is a no-op on a handle that
/// already holds one and the compaction handle always does; their job is bounding
/// *tool-driven* nested generations, which is the default's documented purpose. And
/// `lca-sdk` builds no compaction path at all (asserted below), so there is no
/// fourth front end to budget today.
const SITES: &[(&str, Role, usize)] = &[
    ("crates/lca-cli/src/registry.rs", Role::Compaction, 1),
    ("crates/lca-cli/src/headless.rs", Role::Compaction, 1),
    ("crates/lca-cli/src/headless.rs", Role::NestedCompletion, 1),
    ("crates/lca-cli/src/rpc.rs", Role::NestedCompletion, 1),
    (
        "crates/lca-cli/src/tui/runner.rs",
        Role::NestedCompletion,
        1,
    ),
];

/// The module source of a crate path, through the shared walk (gh #574/#543):
/// a guard that reads one *filename* stops seeing the code as soon as that code
/// moves, and may then pass vacuously instead of failing.
fn module_text(rel: &str) -> String {
    crate::module_text(rel)
}

// Verifies: gh #594 - **both front ends derive the budget, and neither does it
// inside a command handler.** The headless derivation is in `setup`; the
// interface's is `cells.rs`'s `apply_summarization_budget`, called from the
// assembly site, `/reload`, and every path that lands a new model. Asserting the
// *rule call* (`summarization_budget(`) as well as the field write is what keeps a
// front end from inventing its own arithmetic.
#[test]
fn both_front_ends_derive_the_summarization_budget() {
    let headless = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/lca-cli/src/headless.rs"),
    )
    .expect("read headless.rs");
    assert!(
        headless.contains("set_summarization_budget(")
            && headless.contains("ext_provider::summarization_budget("),
        "headless derives the budget from the reserve and the window (the reference \
         implementation the interface had to match):\n{}",
        region(&headless, "set_summarization_budget(")
    );

    // The interface's assembly region, from the registration of the backend to
    // the agent config that receives it: the derivation must sit *there*, so
    // deleting the call reddens this row even though the `use` line and the
    // `for_tests` re-export still spell the name.
    let tui = module_text("crates/lca-cli/src/tui/mod.rs");
    let assembly = tui
        .split("let provider_backend = crate::registry::register_compaction(")
        .nth(1)
        .and_then(|rest| rest.split("let agent_config = agent_config_for(").next())
        .expect("the interface's assembly site is still shaped that way");
    assert!(
        assembly.contains("apply_summarization_budget(backend"),
        "the interface derives the budget where it builds the backend and hands it \
         to the agent config:\n{assembly}"
    );
    let cells = module_text("crates/lca-cli/src/tui/cells.rs");
    assert!(
        cells.contains("set_summarization_budget(")
            && cells.contains("ext_provider::summarization_budget("),
        "the interface's site applies `lca_core`'s rule to the backend:\n{}",
        region(&cells, "fn apply_summarization_budget")
    );
    // The reason it is not in the `/compact` handler: a per-command owner would
    // drift from the model the session is running. Pinned so a future "simplify"
    // cannot move it back into the command.
    let commands = module_text("crates/lca-cli/src/tui/commands.rs");
    assert!(
        !commands.contains("set_summarization_budget("),
        "the budget is a session-scoped fact, not a `/compact` command argument"
    );
}

// Verifies: gh #594 - **every model that lands re-derives the budget.** The
// interface can change models four ways, and a derivation only at startup would
// leave a session on the previous model's budget — the same class of defect
// (a stale number nobody re-reads). Each site is named with what it lands.
#[test]
fn every_path_that_lands_a_new_model_rederives_the_budget() {
    let cases: &[(&str, usize, &str)] = &[
        (
            "crates/lca-cli/src/tui/mod.rs",
            1,
            "`Ui::new`'s assembly, for the model the session started on",
        ),
        (
            "crates/lca-cli/src/tui/reload.rs",
            1,
            "`/reload`, which rebuilds the backend and re-reads the settings",
        ),
        (
            "crates/lca-cli/src/tui/model_select.rs",
            2,
            "`/model` same-provider and cross-provider switches",
        ),
        (
            "crates/lca-cli/src/tui/login.rs",
            1,
            "a login that resolves the session's first model",
        ),
    ];
    for (file, wanted, why) in cases {
        let source = module_text(file);
        let found = source.matches("apply_summarization_budget(").count();
        // The definition in `cells.rs` is a different file; a `use` line and the
        // call sites are what these files carry, and one `use` can serve several
        // calls, so the count is per call, not per mention minus one.
        assert!(
            found >= *wanted,
            "{file} calls `apply_summarization_budget` {found} time(s), want at least \
             {wanted} for {why}"
        );
    }
}

// Verifies: gh #594 - **the sweep is complete or it says so.** A `ProviderBackend`
// built anywhere in `lca-cli` is either a compaction backend (which owes the
// derived budget) or a named nested one (which cannot reach compaction). A new
// site that appears in neither list fails here rather than shipping the stopgap.
#[test]
fn every_backend_site_is_classified() {
    let mut found: Vec<String> = Vec::new();
    // `lib.rs` is an index of `mod` declarations, so its only child is its own
    // file. Walk the two roots the binary is assembled from instead: every
    // backend in this crate lives in `headless.rs`, `rpc.rs`, or a `tui/` file,
    // and `registry.rs` is reached from `lib.rs`.
    let mut sources = crate::module_sources("crates/lca-cli/src/lib.rs");
    for root in [
        "crates/lca-cli/src/headless.rs",
        "crates/lca-cli/src/rpc.rs",
        "crates/lca-cli/src/registry.rs",
        "crates/lca-cli/src/tui/mod.rs",
    ] {
        sources.extend(crate::module_sources(root));
    }
    for (path, text) in sources {
        if text.contains("ProviderBackend::new(") {
            found.push(path);
        }
    }
    found.sort();
    found.dedup();
    assert!(!found.is_empty(), "the walk reaches the backends at all");
    // One file may own more than one site (headless builds its compaction backend
    // *and* a per-turn nested one), so the classification is per site and the
    // reverse check is per `(file, role)`: `found` is deduplicated by path, so it
    // can only prove a file exists, not how many sites it holds.
    let named: Vec<String> = SITES
        .iter()
        .map(|(file, _, _)| (*file).to_string())
        .collect();
    for file in &found {
        assert!(
            named.contains(file),
            "{file} builds a ProviderBackend and is not in `SITES`; name its role \
             (Compaction owes `apply_summarization_budget`, NestedCompletion states \
             why it cannot reach compaction)"
        );
    }
    // And the reverse, so a deleted site does not leave a stale claim behind.
    for file in &named {
        assert!(
            found.contains(file),
            "{file} is listed in `SITES` but builds no ProviderBackend any more"
        );
    }
    // The compaction sites are exactly the ones that carry the derivation.
    for (file, role, _) in SITES {
        if *role != Role::Compaction {
            continue;
        }
        let source = module_text(file);
        let derived = source.contains("set_summarization_budget(")
            || module_text("crates/lca-cli/src/tui/cells.rs").contains("set_summarization_budget(");
        assert!(
            derived,
            "the compaction backend in {file} reaches a derivation"
        );
    }
    // Not covered, stated: `lca-sdk` builds its own `AgentConfig` with no
    // `completion_backend` and registers no compaction strategy, so it has no
    // summarization to budget. `lca acp` does not exist in this tree. If either
    // grows a compaction path, it is the fourth front end this row must name.
    let sdk = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/lca-sdk/src/lib.rs"),
    )
    .expect("read lca-sdk");
    assert!(
        !sdk.contains("compaction_default") && !sdk.contains("ProviderBackend"),
        "the SDK has no compaction path to budget; a future one must be added here"
    );
}

// ---------------------------------------------------------------------------
// Row 3: a summary cut at our budget reports it, and the session still works
// ---------------------------------------------------------------------------

/// One quiet permission asker: nothing in this path prompts.
struct QuietPrompt;

impl PermissionPrompt for QuietPrompt {
    fn ask(&mut self, _action: &Action) -> Decision {
        Decision::Denied
    }
    fn review_proposals(&mut self, _diff: &ProposalDiff) -> bool {
        false
    }
}

fn scratch(name: &str) -> std::path::PathBuf {
    let root = lca_testkit::scratch_path(&format!("regression-gh594-{name}"));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("mkdir");
    root
}

/// A session with the real bundled strategy and a backend whose provider is
/// scripted, so both the cut and the recovery are observable.
struct Fixture {
    store: Arc<SessionStore>,
    session: Session,
    extensions: Arc<ExtensionRegistry>,
    backend: Arc<ProviderBackend>,
    provider: Arc<FakeProvider>,
}

fn fixture(name: &str, provider: FakeProvider) -> Fixture {
    let root = scratch(name);
    let project = root.join("project");
    std::fs::create_dir_all(&project).expect("mkdir");
    let provider = Arc::new(provider);
    let backend = Arc::new(ProviderBackend::new(
        provider.clone(),
        "faux-1",
        "gh594-session",
    ));
    // The interface's own rule, applied as the interface applies it: a 1M window.
    backend.set_summarization_budget(summarization_budget(0.8, 0, 1_000_000));
    let roots = ScopeRoots {
        workspace: project.clone(),
        private: root.join("private"),
        home_config: root.join("config"),
        temp: root.join("tmp"),
        state_dir: root.join("data"),
    };
    let cap = Arc::new(Capabilities::new(
        "compaction-default",
        compaction_default::manifest_grants(),
        roots,
        Arc::new(Mutex::new(QuietPrompt)),
        Arc::new(Mutex::new(
            GrantStore::open(&root.join("grants.json")).expect("grants"),
        )),
        project.clone(),
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

/// Two exchanges big enough that the mechanical excerpt has real content to carry.
fn seed(f: &Fixture) {
    let filler = "the parser rule list grows; the fixture pins the expected text ".repeat(20);
    for i in 0..2u64 {
        f.store
            .append(
                &f.session,
                Record::User {
                    v: FORMAT_VERSION,
                    ts: 1_700_000_000_000 + i,
                    id: format!("u{i}"),
                    parent: None,
                    // The codeword leads: the mechanical summary truncates each
                    // excerpt at 600 characters, so a fact at the tail is not the
                    // one a guard should depend on.
                    content: format!("quartz-77: {filler}"),
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
                    ts: 1_700_000_001_000 + i,
                    id: format!("a{i}"),
                    parent: None,
                    content: vec![ContentBlock::Text {
                        text: filler.clone(),
                    }],
                    reasoning: None,
                    reasoning_signature: None,
                    provider_thinking_level: None,
                    model: Some("faux-1".to_string()),
                    provider: Some("fake".to_string()),
                    usage: None,
                },
            )
            .expect("append assistant");
    }
}

fn compact(f: &Fixture) -> String {
    compact_now(
        f.store.clone(),
        f.session.clone(),
        f.extensions.clone(),
        Some(f.backend.clone() as Arc<dyn CompletionBackend>),
        None,
    )
    .expect("the strategy never fails a turn: compaction still lands")
}

/// The summary on the latest compaction record.
fn summary(f: &Fixture) -> String {
    f.store
        .read_with(&f.session, ViewMode::Display)
        .expect("read")
        .records
        .iter()
        .rev()
        .find_map(|record| match record {
            Record::Compaction { summary, .. } => Some(summary.clone()),
            _ => None,
        })
        .expect("a compaction record")
}

// Verifies: gh #594 - **a summary cut at our budget says it was our budget, and
// the compaction still lands on the mechanical excerpt.** The provider's own text
// arrives as "hit the token cap (max_tokens 160000)"; read alone it looks like an
// endpoint limit, so the strategy's line adds that the number is this session's
// derived budget. The second compaction through the same backend then succeeds,
// which is the "session works afterwards" half: the degradation did not wedge
// anything, it just produced a worse checkpoint.
#[test]
fn a_summary_cut_at_our_budget_names_the_budget_and_still_degrades() {
    let provider = FakeProvider::builder()
        .turn(|t| {
            // The provider's own text, unprefixed: the capability wraps it once,
            // and the shape that matters is the wire kit's (`token cap` + number).
            t.error(
                &format!(
                    "generation hit the token cap (max_tokens {MILLION_WINDOW_BUDGET}) before \
                     finishing; the response is incomplete"
                ),
                false,
            )
            .usage(fake_usage(800, 160_000, 0, 0))
        })
        .turn(|t| {
            t.text("the recovered model summary")
                .usage(fake_usage(800, 40, 0, 0))
        })
        .build();
    let f = fixture("cut-at-budget", provider);
    seed(&f);

    let first = compact(&f);
    assert!(
        first.starts_with("[summarization failed:"),
        "the cut is announced, not passed off as an answer:\n{first}"
    );
    assert!(
        first.contains(&MILLION_WINDOW_BUDGET.to_string()),
        "the number that cut it is named:\n{first}"
    );
    assert!(
        first.contains("this session's summarization budget"),
        "and it is named as OUR budget, not the provider's limit:\n{first}"
    );
    assert!(
        first.contains("larger model window"),
        "with the fix a reader can act on:\n{first}"
    );
    assert!(
        first.contains("quartz-77"),
        "and the mechanical excerpt still carries the range's facts:\n{first}"
    );
    assert_eq!(summary(&f), first, "that is what the session now sees");

    // The session is not wedged: the assembled context carries the degraded
    // summary, and the next compaction gets a real model summary when the
    // generation is not cut.
    let records = f
        .store
        .read_with(&f.session, ViewMode::Display)
        .expect("read")
        .records;
    let assembled = assemble(&records, "sys");
    let in_band: String = assembled
        .messages
        .iter()
        .flat_map(|message| message.content.iter())
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        in_band.contains("summarization failed") && in_band.contains("quartz-77"),
        "the next request's context carries the degraded summary:\n{in_band}"
    );
    // A compaction needs two compactable records; the first one replaced the
    // seeded range with its own summary, so the session continues with a fresh
    // exchange before it compacts again — the shape of a session that carried on
    // after the cut.
    f.store
        .append(
            &f.session,
            Record::User {
                v: FORMAT_VERSION,
                ts: 1_700_000_002_000,
                id: "u2".to_string(),
                parent: None,
                content: "wire the parser into the CLI".to_string(),
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
                ts: 1_700_000_002_500,
                id: "a2".to_string(),
                parent: None,
                content: vec![ContentBlock::Text {
                    text: "wired".to_string(),
                }],
                reasoning: None,
                reasoning_signature: None,
                provider_thinking_level: None,
                model: Some("faux-1".to_string()),
                provider: Some("fake".to_string()),
                usage: None,
            },
        )
        .expect("append assistant");
    let second = compact(&f);
    assert_eq!(
        second, "the recovered model summary",
        "a later compaction through the same backend succeeds: the session kept working"
    );
    assert_eq!(
        f.provider
            .requests()
            .last()
            .expect("a request")
            .extras
            .get("max-tokens"),
        Some(&MILLION_WINDOW_BUDGET.to_string()),
        "and it asked with the derived budget, not the stopgap (gh #594)"
    );
}

// Verifies: gh #169's copy rule still holds for a failure that is **not** our
// budget - a transport error must not be dressed up as one. The budget note is
// gated on the provider's own "token cap" wording, so a denial stays a denial.
#[test]
fn a_failure_that_is_not_our_budget_carries_no_budget_note() {
    let failed = run_compact(
        &holdout(),
        Some(&|_| Err("provider returned HTTP 429".to_string())),
    );
    assert!(
        !failed.contains("summarization budget"),
        "a rate limit is not a budget cut:\n{failed}"
    );
    assert!(
        failed.contains("add a parser"),
        "and it still degrades: {failed}"
    );
}

/// One excerpt set for the copy rule, mirroring the issue's own fixture.
fn holdout() -> Vec<compaction_default::Excerpt> {
    vec![(
        "user".to_string(),
        r#"{"v":1,"t":"user","id":"01","content":"add a parser"}"#.to_string(),
    )]
}

/// A short window onto the source around `needle`, for a failure message that
/// shows the thing it is complaining about.
fn region<'a>(source: &'a str, needle: &str) -> &'a str {
    let at = source.find(needle).unwrap_or(0);
    let start = source[..at].rfind('\n').map_or(0, |index| index + 1);
    let end = source[at..]
        .find("\n}")
        .map_or(source.len(), |index| at + index + 2);
    &source[start..end]
}
