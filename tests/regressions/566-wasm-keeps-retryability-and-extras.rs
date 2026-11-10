//! NFR-24 guard for gh #566: a WASM subscription provider keeps its retryability
//! and its per-message extras.
//!
//! ## The two defects
//!
//! `crates/lca-subscription/src/wasm.rs` is the shared guest every subscription
//! provider on this kit expands (codex, grok, kimi-coding, meta,
//! github-copilot). Two conversions there lost facts that the native delivery
//! mode kept:
//!
//! - **`open_stream` returned `Result<_, String>`** and flattened a
//!   `StreamFailure` with `.map_err(|failure| failure.message)`. `retryable` is
//!   the one flag the turn loop's retry rule reads (`lca-core`'s
//!   `provider_call`: retry when `retryable`, or when the message names capacity).
//!   Flattened, a 429 from the gateway reached the loop as text and the loop
//!   re-derived its answer from `classify_message`'s string heuristics - the
//!   gh #356 class of guesswork, at the one seam that had an exact answer.
//!   A 401 could be retried three times with backoff and then exit on the wrong
//!   code, and a 429 could be abandoned on the first try.
//! - **`stream_completion` wrote `extras: Default::default()`** where the WIT
//!   `message` record carries `extras: list<extra-pair>`, so a WASM-delivered
//!   turn dropped `extras["queue"]` (ADR-0038's steering marker) and the
//!   thinking-signature key. The host copies those pairs through untouched
//!   (`to_wit_request`), so the guest was the only side dropping them - the same
//!   omission gh #316 fixed for `ModelInfo.extras` one screen above.
//!
//! ## What this pins
//!
//! The behavioural half is split by what can actually be driven here:
//!
//! - **Retryability, end to end through the turn**: `tests/regressions/566-…`'s
//!   rows feed `lca_subscription::open_failure_event` - the exact helper the
//!   guest calls - and run the real `Agent` loop. A 429 schedules a retry and
//!   resumes; a 401 fails on the first attempt. That is the claim the defect
//!   broke, and it is pinned where the decision is made.
//! - **The kit's own half**: `crates/lca-subscription/tests/kit.rs` pins that the
//!   helper carries `retryable` verbatim and that `StreamFailure` survives the
//!   open typing (`429` retryable, `401` not, a missing account never).
//! - **Extras through the WIT conversion**: the guest conversion lives inside the
//!   macro expansion, which only exists for `wasm32-wasip2`; a component fixture
//!   would be the behavioural proof, and refreshing one is outside this cycle's
//!   file boundary. So it is pinned structurally below - the macro copies the
//!   pairs, and no `Default::default()` remains on a record whose WIT twin
//!   carries `list<extra-pair>`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failure.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lca_core::{Agent, AgentConfig, TurnEvent, TurnOutcome, TurnSink};
use lca_permissions::{Decision, GrantStore, PermissionPrompt, ProposalDiff};
use lca_session::SessionStore;
use lca_testkit::FakeProvider;
use lca_tools::{CancelFlag, NativeOps, ToolExecutor};

fn read(rel: &str) -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(root.join(rel)).unwrap_or_else(|err| panic!("read {rel}: {err}"))
}

const WASM: &str = "crates/lca-subscription/src/wasm.rs";

/// The body of one `fn`, from its signature to the next item at column zero.
fn item(source: &str, signature: &str) -> String {
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("`{signature}` exists in {WASM}"));
    let rest = &source[start..];
    let end = rest
        .match_indices('\n')
        .skip(1)
        .map(|(at, _)| at)
        .find(|at| {
            let next = &rest[at + 1..];
            next.starts_with("fn ") || next.starts_with("pub fn ")
        })
        .unwrap_or(rest.len());
    rest[..end].to_string()
}

// ---------------------------------------------------------------------------
// The turn half: retryability survives the seeded stream
// ---------------------------------------------------------------------------

struct NoPrompt;

impl PermissionPrompt for NoPrompt {
    fn ask(&mut self, _action: &lca_permissions::Action) -> Decision {
        Decision::Denied
    }
    fn review_proposals(&mut self, _diff: &ProposalDiff) -> bool {
        false
    }
}

#[derive(Default)]
struct CollectingSink {
    events: Vec<TurnEvent>,
}

impl TurnSink for CollectingSink {
    fn on_event(&mut self, event: TurnEvent) {
        self.events.push(event);
    }
}

impl CollectingSink {
    fn count(&self, predicate: impl Fn(&TurnEvent) -> bool) -> usize {
        self.events.iter().filter(|event| predicate(event)).count()
    }

    fn texts(&self) -> String {
        self.events
            .iter()
            .filter_map(|event| match event {
                TurnEvent::TextDelta(delta) => Some(delta.as_str()),
                _ => None,
            })
            .collect()
    }
}

struct Harness {
    store: SessionStore,
    session: lca_session::Session,
    grants: Arc<Mutex<GrantStore>>,
    tools: ToolExecutor,
    provider: FakeProvider,
    config: AgentConfig,
}

fn harness(name: &str, provider: FakeProvider, config: AgentConfig) -> Harness {
    let root = lca_testkit::scratch_path(name);
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
    Harness {
        store,
        session,
        grants,
        tools,
        provider,
        config,
    }
}

fn config() -> AgentConfig {
    AgentConfig {
        provider: "fake".to_string(),
        model: "faux-1".to_string(),
        retry_limit: 3,
        retry_base_delay: Duration::ZERO,
        max_iterations: 50,
        ..AgentConfig::default()
    }
}

async fn run_turn(h: &mut Harness, input: &str, sink: &mut CollectingSink) -> TurnOutcome {
    let mut prompt = NoPrompt;
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
    agent.run_turn(input, sink, &CancelFlag::new()).await
}

/// What the guest does to a failed open: the wire kit names the status and its
/// retryability, and the kit's helper carries that pair across as the stream's
/// own `error` event - the shape the host maps onto `StreamEvent::Error`.
fn seeded_error(status: u16) -> (String, bool) {
    let failure = lca_wire_openai::failure_for_status(status, "mock gateway");
    lca_subscription::open_failure_event(&failure)
}

// Verifies: gh #566 - a 429 from the stream open is retryable **through the
// turn**, not just in the kit.
//
// Before the fix the guest returned `Err(failure.message)` and the host fell
// back to `classify_message`, which reads a `provider returned HTTP 429` prefix
// and answers retryable - so this row would have passed by accident on the text
// shape. What it pins now is the exactness: the flag arrives from the wire kit's
// own classification, through the same helper the guest uses, and the turn
// backs off and resumes.
#[tokio::test]
async fn a_429_from_the_stream_open_is_retried_by_the_turn() {
    let (message, retryable) = seeded_error(429);
    assert!(retryable, "the wire kit says a 429 is worth retrying");
    let provider = FakeProvider::builder()
        .turn(move |t| {
            t.error(&message, retryable)
                .usage(lca_testkit::fake_usage(1, 0, 0, 0))
        })
        .turn(|t| {
            t.text("resumed")
                .usage(lca_testkit::fake_usage(10, 5, 0, 0))
        })
        .build();
    let mut h = harness("566-retry-429", provider, config());
    let mut sink = CollectingSink::default();
    let outcome = run_turn(&mut h, "hi", &mut sink).await;
    assert_eq!(outcome.status, lca_core::TurnStatus::Ok);
    assert_eq!(
        sink.count(|event| matches!(event, TurnEvent::RetryScheduled { .. })),
        1,
        "one backoff before the retry"
    );
    assert!(
        sink.texts().contains("resumed"),
        "the retry's answer is delivered"
    );
}

// Verifies: gh #566 - a 401 is **not** retried. An expired credential is the one
// failure a retry cannot fix, so retrying it burns three attempts against a dead
// token and then reports the wrong exit class.
#[tokio::test]
async fn a_401_from_the_stream_open_is_not_retried() {
    let (message, retryable) = seeded_error(401);
    assert!(!retryable, "the wire kit says a 401 cannot be retried");
    let provider = FakeProvider::builder()
        .turn(move |t| {
            t.error(&message, retryable)
                .usage(lca_testkit::fake_usage(1, 0, 0, 0))
        })
        .build();
    let mut h = harness("566-retry-401", provider, config());
    let mut sink = CollectingSink::default();
    let outcome = run_turn(&mut h, "hi", &mut sink).await;
    assert_eq!(
        outcome.status,
        lca_core::TurnStatus::Error,
        "a refusal ends the turn"
    );
    assert_eq!(
        sink.count(|event| matches!(event, TurnEvent::RetryScheduled { .. })),
        0,
        "no retry for a 401: the text says 401, and the flag says non-retryable"
    );
    assert!(
        sink.count(|event| matches!(event, TurnEvent::Error { .. })) >= 1,
        "the failure is reported, not swallowed"
    );
}

// Verifies: gh #566 - the helper the guest calls carries the wire kit's flag
// unchanged, for the statuses that matter, and never invents one.
//
// This is the seam the defect lived at: `StreamFailure` -> the stream event's
// `(message, retryable)`. `class` has no slot in the WIT record, so it is not
// asserted here - the report says what that costs (the turn labels a
// stream-delivered failure `transport`; the exit code is the same provider
// code for all three classes).
#[test]
fn the_stream_event_carries_the_wire_kits_retryability() {
    for (status, expected) in [
        (429, true),
        (408, true),
        (500, true),
        (529, true),
        (401, false),
        (403, false),
        (400, false),
        (404, false),
    ] {
        let (message, retryable) = seeded_error(status);
        assert_eq!(
            retryable, expected,
            "status {status} keeps the kit's retryability: {message}"
        );
        assert!(
            message.contains(&status.to_string()),
            "and the message still names the status a reader can act on: {message}"
        );
    }
}

// Verifies: gh #566 - a **native** cancel lands during the wait too.
//
// The first fix trusted the host call alone, and that is not enough: the probe
// path is served from the extension's own resource tree and never consults the
// cancellation flag, so a native wait slept out its whole interval (measured
// 5.00 s with `interval: 5`). The wait now also reads `ProviderCap::is_cancelled`,
// which the native view answers from the engine's flag and a guest keeps at the
// trait's `false` (it could not act on it without the epoch anyway).
//
// This row drives the **real** capability engine - the one the host's
// `interrupt()` flags - rather than the kit's fake, because the defect was that
// the real engine's probe path says nothing about cancelling.
//
// Measured: the cancel fires at 100 ms and the wait returns at **100.9 ms**
// (two runs), because the flag is read at the end of the very slice in flight.
// Removing the flag check makes this row take **5.02 s** - the whole interval,
// exactly the defect. The bound is 250 ms: 2.5x the measurement, still 20x
// under the interval.
//
// **On macOS the bound is 500 ms** (gh #588; `docs/platform-notes.md` records the
// family). Hosted macOS readings were **298.2 and 366.4 ms** against 250 - the
// cancel lands at 100 ms of *work*, and the rest is the scheduling quantum plus the
// 25 ms slice the flag is read at the end of, so a bound closer than two quanta to
// the cancel's own arrival is measuring the runner. The number is derived from the
// defect, not from the observation: the wait this row rules out is the 5 s interval
// (5.02 s measured with the flag read removed), so 500 ms still proves it 10x over
// on any scheduler. The per-platform shape is `crates/lca-ext-host/tests/nfr.rs`'s.
fn cancel_budget() -> Duration {
    if cfg!(target_os = "macos") {
        Duration::from_millis(500)
    } else {
        Duration::from_millis(250)
    }
}
#[test]
fn a_native_cancel_lands_inside_the_device_code_wait() {
    use std::sync::{Arc, Mutex};

    use lca_permissions::{Decision, GrantStore, PermissionPrompt, ProposalDiff, ScopeRoots};

    struct Always;
    impl PermissionPrompt for Always {
        fn ask(&mut self, _action: &lca_permissions::Action) -> Decision {
            Decision::Always
        }
        fn review_proposals(&mut self, _diff: &ProposalDiff) -> bool {
            false
        }
    }

    let root = lca_testkit::scratch_path("lca-gh566-native-cancel");
    let _ = std::fs::remove_dir_all(&root);
    for dir in ["project", "private", "config", "data", "tmp"] {
        std::fs::create_dir_all(root.join(dir)).expect("mkdir");
    }
    let roots = ScopeRoots {
        workspace: root.join("project"),
        private: root.join("private"),
        home_config: root.join("config"),
        temp: root.join("tmp"),
        state_dir: root.join("data"),
    };
    let grants = lca_tools::CapabilityGrants {
        credentials: true,
        net: vec![lca_permissions::parse_net_pattern("127.0.0.1").expect("loopback pattern")],
        ..lca_tools::CapabilityGrants::default()
    };
    let cap: Arc<lca_tools::Capabilities> = Arc::new(lca_tools::Capabilities::new(
        "gi566",
        grants,
        roots,
        Arc::new(Mutex::new(Always)),
        Arc::new(Mutex::new(
            GrantStore::open(&root.join("grants.json")).expect("store"),
        )),
        root.join("project"),
        None,
    ));

    // The cancel fires 100 ms in: inside the first 5 s interval, after the
    // code page would have been shown.
    let canceller = cap.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        canceller.cancel();
    });

    let auth = lca_subscription::DeviceAuthorization {
        device_code: "d".to_string(),
        user_code: "u".to_string(),
        verification_uri: "https://x.test/device".to_string(),
        interval_secs: 5,
        expires_in_secs: 30,
    };
    let started = std::time::Instant::now();
    let outcome = lca_subscription::poll_device_code(
        cap.as_ref() as &dyn lca_protocol::ProviderCap,
        &auth,
        "https://x.test/token",
        &[("a", "b")],
    );
    let elapsed = started.elapsed();
    let err = outcome.expect_err("a cancelled wait is an error, not a token");
    let budget = cancel_budget();
    assert!(
        elapsed < budget,
        "a native cancel must land inside the budget, not after the {}-second interval: \
         took {elapsed:?} against {budget:?} ({})",
        auth.interval_secs,
        err.0
    );
    assert!(
        err.0.contains("cancelled"),
        "the error names the cancellation: {}",
        err.0
    );
}

// Verifies: gh #566 - the flag is on the trait, defaulted for a guest.
//
// The default is the honest answer for a WASM guest: no host import carries a
// cancellation there, and a polled value could not be acted on without the
// epoch trap that fires at its next host call. Pinned so a future reader cannot
// "fix" the default into an error the guest path would then trip over on every
// call.
#[test]
fn the_cancellation_flag_is_on_the_trait_and_defaults_to_false() {
    struct Bare;
    impl lca_protocol::ProviderCap for Bare {
        fn net_request(
            &self,
            _method: &str,
            _url: &str,
            _headers: &[(&str, &str)],
            _body: Option<&[u8]>,
        ) -> Result<u32, lca_protocol::CapabilityError> {
            Ok(0)
        }
        fn net_response_status(&self, _handle: u32) -> Result<u16, lca_protocol::CapabilityError> {
            Ok(200)
        }
        fn net_read_body(
            &self,
            _handle: u32,
            _max: usize,
        ) -> Result<Option<Vec<u8>>, lca_protocol::CapabilityError> {
            Ok(None)
        }
        fn net_close_response(&self, _handle: u32) -> Result<(), lca_protocol::CapabilityError> {
            Ok(())
        }
        fn credentials_get(&self, _key: &str) -> Option<String> {
            None
        }
        fn credentials_set(
            &self,
            _key: &str,
            _value: &str,
        ) -> Result<(), lca_protocol::CapabilityError> {
            Ok(())
        }
        fn credentials_delete(&self, _key: &str) -> Result<(), lca_protocol::CapabilityError> {
            Ok(())
        }
    }
    assert!(
        !lca_protocol::ProviderCap::is_cancelled(&Bare),
        "a view that cannot see a cancellation says so, and the default is not an error"
    );
}

// Verifies: gh #566 - the guest no longer flatten a `StreamFailure`.
//
// Two things made the omission invisible: the `String` return type compiled
// everywhere, and the message text kept the HTTP status so the host's heuristics
// usually *guessed* right. The exactness is the fix, so the structure is pinned:
// `open_stream` is typed with the failure, its helpers construct failures rather
// than strings, and `errors` are delivered as the stream's own event.
#[test]
fn open_stream_keeps_the_failure_type_and_delivers_it_as_an_event() {
    let source = read(WASM);
    assert!(
        !source.contains(".map_err(|failure| failure.message)"),
        "the flattening line is gone: it threw away `class` and `retryable`"
    );
    let open = item(&source, "fn open_stream(");
    // Gh #576 moved the type to `lca-protocol` (one definition beside
    // `CapabilityError`); the guest now names the shared type directly, which is
    // the stronger form of the same claim: the failure crossed the boundary with
    // its class and retryability intact, and the kit re-export is no longer what
    // the signature depends on.
    assert!(
        open.contains("::lca_protocol::StreamFailure"),
        "`open_stream` is typed with the failure it actually has:\n{open}"
    );
    assert!(
        !open.contains("-> Result<::lca_wire_openai::ResponseStreamDriver<'static>, String>"),
        "no `Result<_, String>` helper signature survives here"
    );
    assert!(
        open.contains("class: \"invalid\"") && open.contains("retryable: false"),
        "the failure constructors state their class and retryability explicitly:\n{open}"
    );
    // The stream is returned with the failure seeded, and `next` yields it first.
    assert!(
        source.contains("open_failure_event"),
        "the failure crosses as the stream event's `(message, retryable)` pair"
    );
    assert!(
        source.contains("seed: RefCell<Option<(String, bool)>>"),
        "the seeded stream carries exactly the pair the WIT error case has"
    );
    let next = item(&source, "impl GuestCompletionStream for QueuedStream");
    assert!(
        next.contains("WasmEvent::Error((message, retryable))"),
        "the seeded failure is the stream's first event, not a `result` error:\n{next}"
    );
}

// Verifies: gh #566 - per-message extras survive the guest conversion.
//
// The host side already carries them (`to_wit_request` copies `message.extras`
// and adds the reasoning signature under its reserved key); the guest is the
// side that dropped them. A record whose WIT twin has `list<extra-pair>` must
// never be filled with `Default::default()` - the rule this row exists to keep
// after this cycle's fix.
#[test]
fn the_guest_copies_per_message_extras() {
    let source = read(WASM);
    assert!(
        !source.contains("extras: Default::default()"),
        "no record whose WIT twin carries extras is left empty in the shared guest"
    );
    let completion = item(&source, "fn stream_completion(");
    assert!(
        completion.contains("message\n                            .extras"),
        "the message conversion copies the pairs the host sent:\n{completion}"
    );
    assert!(
        !completion.contains("extras: BTreeMap::new()")
            && !completion.contains("extras: Vec::new()"),
        "and does not rebuild an empty map somewhere else:\n{completion}"
    );
    // The host's own half, so a future reader sees both ends of one contract.
    // Gh #552 split `host_imports.rs` into capability domains, so this reads the
    // module (`crate::module_text`), which is the rule gh #574/#543 established:
    // a guard keyed on one filename goes vacuous as soon as the code moves.
    let host = crate::module_text("crates/lca-ext-host/src/host_imports.rs");
    assert!(
        host.contains("extra_pairs(&message.extras)"),
        "the host does send the pairs: one contract, two ends"
    );
}
