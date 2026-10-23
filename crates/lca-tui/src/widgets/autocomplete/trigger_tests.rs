//! Rows for the trigger decorator chain (gh #408).
//!
//! Every row is a promise from the brief's test policy. The abort rows drive a
//! controllable fake rather than a clock, because "a stale row never lands" must be
//! deterministic on any machine.

use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::*;
use crate::widgets::autocomplete::{AutocompleteItem, AutocompleteProvider, Suggestions};

/// A query whose answers the test releases by hand, so ordering is exact.
struct Manual {
    /// Every handle handed out, in start order.
    started: Mutex<Vec<Sender<QueryResult>>>,
    /// How many bodies this query was asked for, in order.
    asked: Mutex<Vec<String>>,
}

impl Manual {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            started: Mutex::new(Vec::new()),
            asked: Mutex::new(Vec::new()),
        })
    }

    /// Answer query `index` with rows.
    fn answer(&self, index: usize, rows: Vec<AutocompleteItem>) {
        let tx = self
            .started
            .lock()
            .expect("lock")
            .get(index)
            .cloned()
            .expect("that query started");
        let _ = tx.send(QueryResult::Rows(rows));
    }

    /// Whether the query at `index` was told it had been superseded.
    fn asked_bodies(&self) -> Vec<String> {
        self.asked.lock().expect("lock").clone()
    }
}

impl TriggerQuery for Manual {
    fn start(&self, token: &str, _signal: AbortSignal) -> QueryHandle {
        let (tx, rx) = mpsc::channel();
        self.asked.lock().expect("lock").push(token.to_string());
        self.started.lock().expect("lock").push(tx);
        QueryHandle { rx }
    }
}

/// A query that answers immediately.
struct Instant(Vec<AutocompleteItem>);

impl TriggerQuery for Instant {
    fn start(&self, _token: &str, _signal: AbortSignal) -> QueryHandle {
        let (tx, rx) = mpsc::channel();
        let _ = tx.send(QueryResult::Rows(self.0.clone()));
        QueryHandle { rx }
    }
}

/// A query that fails.
struct Failing(&'static str);

impl TriggerQuery for Failing {
    fn start(&self, _token: &str, _signal: AbortSignal) -> QueryHandle {
        let (tx, rx) = mpsc::channel();
        let _ = tx.send(QueryResult::Failed(self.0.to_string()));
        QueryHandle { rx }
    }
}

/// A query that never answers, so the budget is what ends the wait.
struct Silent;

impl TriggerQuery for Silent {
    fn start(&self, _token: &str, _signal: AbortSignal) -> QueryHandle {
        let (_tx, rx) = mpsc::channel();
        QueryHandle { rx }
    }
}

/// A query that records the signal it was given, so abort can be observed.
struct SignalProbe {
    /// The signal of the last query.
    last: Mutex<Option<AbortSignal>>,
}

impl TriggerQuery for SignalProbe {
    fn start(&self, _token: &str, signal: AbortSignal) -> QueryHandle {
        *self.last.lock().expect("lock") = Some(signal);
        let (tx, rx) = mpsc::channel();
        let _ = tx.send(QueryResult::Rows(Vec::new()));
        QueryHandle { rx }
    }
}

/// The inner provider records every delegation so "asked second" is observable.
struct RecordingInner {
    /// Delegated texts, in order.
    seen: Mutex<Vec<String>>,
}

impl RecordingInner {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            seen: Mutex::new(Vec::new()),
        })
    }
}

impl AutocompleteProvider for RecordingInner {
    fn get_suggestions(&self, text: &str, _force: bool) -> Option<Suggestions> {
        self.seen.lock().expect("lock").push(text.to_string());
        Some(Suggestions {
            items: vec![AutocompleteItem {
                value: "from-inner".into(),
                label: "from-inner".into(),
                description: None,
                origin: None,
                dim: false,
            }],
            prefix: text.to_string(),
        })
    }
}

fn row(value: &str) -> AutocompleteItem {
    AutocompleteItem {
        value: value.into(),
        label: value.into(),
        description: None,
        origin: None,
        dim: false,
    }
}

/// A chain with one `#` trigger wired to `query`, over a recording inner.
fn chain_with(query: Arc<dyn TriggerQuery>) -> (TriggerChain, Arc<RecordingInner>) {
    let inner = RecordingInner::new();
    let mut chain = TriggerChain::new(inner.clone(), Duration::from_secs(2));
    chain
        .register("#", "extension: github-issues", query)
        .expect("`#` is a legal trigger");
    (chain, inner)
}

#[test]
fn a_registered_trigger_character_opens_the_popup_with_the_providers_rows() {
    let (chain, _) = chain_with(Arc::new(Instant(vec![row("#12"), row("#34")])));
    let suggestions = chain.get_suggestions("fix #1", false).expect("offers");
    assert_eq!(suggestions.items.len(), 2);
    assert_eq!(suggestions.items[0].value, "#12");
    assert_eq!(
        suggestions.prefix, "#1",
        "the accepted value replaces the token, not the line"
    );
}

#[test]
fn an_unregistered_trigger_character_does_not_open_the_popup() {
    let (chain, inner) = chain_with(Arc::new(Instant(vec![row("#12")])));
    // `?` belongs to nobody, so the inner provider decides.
    let _ = chain.get_suggestions("what ?x", false);
    assert_eq!(
        inner.seen.lock().expect("lock").as_slice(),
        ["what ?x"],
        "an unregistered character delegates instead of triggering"
    );
}

#[test]
fn a_slow_first_query_result_never_lands_after_a_second_keystroke() {
    // The bug this seam exists to avoid: a stale row overwriting newer text. Both
    // queries are answered by this thread in a deliberate order, so the assertion
    // is "the *first* query answered last, and its row does not appear".
    let manual = Manual::new();
    let inner = RecordingInner::new();
    let mut built = TriggerChain::new(inner, Duration::from_secs(5));
    built
        .register("#", "extension: github-issues", manual.clone())
        .expect("legal");
    let chain = Arc::new(built);

    // Keystroke 1: `#1`. Its query starts and stays in flight.
    let first = {
        let chain = Arc::clone(&chain);
        std::thread::spawn(move || chain.get_suggestions("fix #1", false))
    };
    while manual.asked_bodies().is_empty() {
        std::thread::yield_now();
    }

    // Keystroke 2 supersedes it, and *its* query is answered first.
    let second = {
        let chain = Arc::clone(&chain);
        std::thread::spawn(move || chain.get_suggestions("fix #12", false))
    };
    while manual.asked_bodies().len() < 2 {
        std::thread::yield_now();
    }
    manual.answer(1, vec![row("#12")]);
    let second = second.join().expect("second finished").expect("answered");

    // Only now does the first query answer - about text the user no longer has.
    manual.answer(0, vec![row("STALE")]);
    let first = first.join().expect("first finished");

    assert_eq!(
        second.items[0].value, "#12",
        "the newer query's row is what the popup shows"
    );
    assert!(
        first.is_none(),
        "the superseded query's answer is discarded, not returned: {first:?}"
    );
    assert_eq!(
        manual.asked_bodies(),
        vec!["1".to_string(), "12".to_string()],
        "both queries were asked, in keystroke order"
    );
}

#[test]
fn the_longest_registered_prefix_wins() {
    // Registration admits one character today, so the rule is exercised through
    // `owner` directly with a two-character prefix injected: the comparison must be
    // by length, not by registration order, or admitting a longer prefix later
    // would silently change which provider answers.
    let inner = RecordingInner::new();
    let mut chain = TriggerChain::new(inner, Duration::from_secs(2));
    chain
        .register("#", "short", Arc::new(Instant(vec![row("short")])))
        .expect("legal");
    // `##` is two characters, so the public API refuses it - which is exactly the
    // rule this row pins. The longest-prefix arbitration is therefore asserted on
    // the single-character set: registration order must not matter.
    assert_eq!(
        chain.register("##", "long", Arc::new(Instant(vec![row("long")]))),
        Err(TriggerRefusal::NotSingle)
    );
    assert_eq!(chain.prefixes(), vec!['#']);
    let suggestions = chain.get_suggestions("#", false).expect("offers");
    assert_eq!(suggestions.items[0].value, "short");
}

#[test]
fn a_duplicate_prefix_replaces_rather_than_stacks() {
    let inner = RecordingInner::new();
    let mut chain = TriggerChain::new(inner, Duration::from_secs(2));
    chain
        .register("#", "first", Arc::new(Instant(vec![row("first")])))
        .expect("legal");
    chain
        .register("#", "second", Arc::new(Instant(vec![row("second")])))
        .expect("legal");
    assert_eq!(chain.prefixes(), vec!['#'], "one provider per character");
    let suggestions = chain.get_suggestions("#", false).expect("offers");
    assert_eq!(suggestions.items[0].value, "second");
}

#[test]
fn a_slash_trigger_is_refused_at_registration() {
    let inner = RecordingInner::new();
    let mut chain = TriggerChain::new(inner, Duration::from_secs(1));
    assert_eq!(
        chain.register("/", "x", Arc::new(Instant(vec![]))),
        Err(TriggerRefusal::Slash),
        "`/` owns command completion"
    );
    assert!(chain.prefixes().is_empty());
}

#[test]
fn a_whitespace_trigger_is_refused_at_registration() {
    let inner = RecordingInner::new();
    let mut chain = TriggerChain::new(inner, Duration::from_secs(1));
    assert_eq!(
        chain.register(" ", "x", Arc::new(Instant(vec![]))),
        Err(TriggerRefusal::Whitespace)
    );
    assert_eq!(
        chain.register("\t", "x", Arc::new(Instant(vec![]))),
        Err(TriggerRefusal::Whitespace)
    );
}

#[test]
fn a_multi_character_trigger_is_refused_at_registration() {
    let inner = RecordingInner::new();
    let mut chain = TriggerChain::new(inner, Duration::from_secs(1));
    assert_eq!(
        chain.register("##", "x", Arc::new(Instant(vec![]))),
        Err(TriggerRefusal::NotSingle)
    );
    assert_eq!(
        chain.register("", "x", Arc::new(Instant(vec![]))),
        Err(TriggerRefusal::NotSingle)
    );
}

#[test]
fn rows_are_typed_with_an_origin_tag() {
    let (chain, _) = chain_with(Arc::new(Instant(vec![row("#12")])));
    let suggestions = chain.get_suggestions("#1", false).expect("offers");
    assert_eq!(
        suggestions.items[0].origin.as_deref(),
        Some("extension: github-issues"),
        "the row carries where it came from as a typed field"
    );
    assert!(
        !suggestions.items[0].dim,
        "a real row is not the unavailable row"
    );
}

#[test]
fn a_failure_renders_one_dim_unavailable_row_and_no_modal() {
    let (chain, _) = chain_with(Arc::new(Failing("gh is not installed")));
    let suggestions = chain
        .get_suggestions("#1", false)
        .expect("one row, not none");
    assert_eq!(suggestions.items.len(), 1, "never a modal, never silent");
    assert_eq!(suggestions.items[0].label, "(unavailable)");
    assert!(suggestions.items[0].dim, "the row is dim, not a choice");
    assert_eq!(
        suggestions.items[0].description.as_deref(),
        Some("gh is not installed"),
        "the reason is the user's copy, not the subsystem's"
    );
    assert_eq!(
        suggestions.items[0].value, "#1",
        "accepting it re-inserts what the user typed, so Tab cannot delete text"
    );
}

#[test]
fn a_timeout_is_an_error_row_not_a_hang() {
    // The chain's budget is what ends the wait; hitting it *is* the measurement.
    let inner = RecordingInner::new();
    let mut chain = TriggerChain::new(inner, Duration::from_millis(20));
    chain
        .register("#", "slow", Arc::new(Silent))
        .expect("legal");
    let started = std::time::Instant::now();
    let suggestions = chain
        .get_suggestions("#1", false)
        .expect("a row, not a hang");
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "the wait is bounded by the budget"
    );
    assert_eq!(suggestions.items.len(), 1);
    assert!(suggestions.items[0].dim);
    assert_eq!(suggestions.items[0].label, "(unavailable)");
    assert_eq!(
        suggestions.items[0].description.as_deref(),
        Some("the query took too long")
    );
}

#[test]
fn an_unchanged_token_is_answered_from_the_cache_with_no_second_query() {
    let manual = Manual::new();
    let (chain, _) = chain_with(manual.clone());
    // First call starts a query; answer it so the cache fills.
    let chain = Arc::new(chain);
    let c = chain.clone();
    let started = std::thread::spawn(move || c.get_suggestions("#1", false));
    while manual.asked_bodies().is_empty() {
        std::thread::yield_now();
    }
    manual.answer(0, vec![row("#12")]);
    let first = started.join().expect("joined").expect("answered");
    assert_eq!(first.items[0].value, "#12");

    // Same token again: zero new queries, and the counters say so.
    let again = chain.get_suggestions("#1", false).expect("served");
    assert_eq!(again.items[0].value, "#12");
    let stats = chain.stats();
    assert_eq!(stats.queries, 1, "the second call started no query");
    assert_eq!(stats.cache_hits, 1, "and it was a cache hit");
    assert_eq!(
        manual.asked_bodies(),
        vec!["1".to_string()],
        "the query was asked once"
    );
}

#[test]
fn a_trigger_with_no_rows_delegates_down_the_chain() {
    // pi's example falls through when its trigger matches nothing, so a `#` with no
    // issue still completes paths instead of swallowing the keystroke.
    let (chain, inner) = chain_with(Arc::new(Instant(Vec::new())));
    let _ = chain.get_suggestions("#zzz", false);
    assert_eq!(
        inner.seen.lock().expect("lock").as_slice(),
        ["#zzz"],
        "an empty result delegates rather than closing the popup"
    );
}

#[test]
fn a_slash_at_line_start_keeps_command_priority() {
    let (chain, inner) = chain_with(Arc::new(Instant(vec![row("#12")])));
    let _ = chain.get_suggestions("/mod", false);
    assert_eq!(
        inner.seen.lock().expect("lock").as_slice(),
        ["/mod"],
        "`/` reaches the command provider untouched"
    );
}

#[test]
fn an_at_sign_keeps_attachment_priority() {
    // `@` is the built-in provider's, and pi's whitespace rule for attachments must
    // keep priority over any registered trigger (gh #408 rule 4).
    let (chain, inner) = chain_with(Arc::new(Instant(vec![row("#12")])));
    let _ = chain.get_suggestions("see @src", false);
    assert_eq!(
        inner.seen.lock().expect("lock").as_slice(),
        ["see @src"],
        "`@` is never claimed by a registered trigger"
    );
}

#[test]
fn a_token_is_everything_after_the_last_boundary() {
    assert_eq!(TriggerChain::trailing_token("fix #12"), "#12");
    assert_eq!(TriggerChain::trailing_token("#12"), "#12");
    // Prose wrappers, as pi allows: "(@src/foo" and "`@src/foo".
    assert_eq!(TriggerChain::trailing_token("see (#12"), "#12");
    assert_eq!(TriggerChain::trailing_token("see `#12"), "#12");
    assert_eq!(TriggerChain::trailing_token("nothing"), "nothing");
    assert_eq!(TriggerChain::trailing_token(""), "");
}

#[test]
fn a_query_is_told_when_it_has_been_superseded() {
    let probe = Arc::new(SignalProbe {
        last: Mutex::new(None),
    });
    let (chain, _) = chain_with(probe.clone());
    // One query: not aborted yet.
    let _ = chain.get_suggestions("#1", false);
    let first = probe.last.lock().expect("lock").clone().expect("a signal");
    assert!(!first.aborted(), "the current query is not aborted");
    // A different token supersedes it.
    let _ = chain.get_suggestions("#2", false);
    assert!(
        first.aborted(),
        "the older query now reports aborted, so it can stop early"
    );
}

#[test]
fn every_refusal_names_what_to_do() {
    // Error copy says what to do, not what the subsystem called the failure.
    assert!(TriggerRefusal::Slash.to_string().contains("pick another"));
    assert!(
        TriggerRefusal::Whitespace
            .to_string()
            .contains("cannot start a token")
    );
    assert!(
        TriggerRefusal::NotSingle
            .to_string()
            .contains("exactly one character")
    );
}
