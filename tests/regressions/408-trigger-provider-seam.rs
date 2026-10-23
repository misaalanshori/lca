//! NFR-24 guard for gh #408: the trigger-provider seam stays a seam.
//!
//! These rows read the source, because the promise is about the *shape* of the seam
//! rather than any single behaviour: a later cycle could pass every behavioural row
//! above while quietly turning the decorator into a registry, unguarding the trigger
//! validator, or letting the chain own `@`. Each row keys on a shape that cannot be
//! renamed away without losing the property.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failure.

fn read(path: &str) -> String {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(root.join(path)).unwrap_or_else(|err| panic!("read {path}: {err}"))
}

/// Code only: comments removed, so prose naming a past mistake cannot trip a guard.
fn code_only(text: &str) -> String {
    text.lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The chain must ask the inner provider, not contain a second copy of the built-in
/// logic. A registry of callbacks would satisfy the behaviour rows and fail this one.
#[test]
fn the_chain_delegates_to_an_inner_provider_rather_than_owning_the_tokens() {
    let source = code_only(&read("crates/lca-tui/src/widgets/autocomplete/trigger.rs"));
    assert!(
        source.contains("inner: Arc<dyn AutocompleteProvider>"),
        "the chain holds the provider it decorates"
    );
    // Every non-trigger token reaches the inner provider.
    assert!(
        source.matches("self.inner.get_suggestions").count() >= 3,
        "an unowned token, a `/` line and an empty result all delegate"
    );
    // And it does not re-implement scoring itself.
    assert!(
        !source.contains("fn fuzzy_match_scored"),
        "the chain must not carry a second matcher"
    );
}

/// A trigger that could hijack command or path completion is the failure this seam
/// can introduce, so the validator must refuse, and must refuse for the right reason.
#[test]
fn a_hijacking_trigger_is_refused_with_a_named_reason() {
    let source = code_only(&read("crates/lca-tui/src/widgets/autocomplete/trigger.rs"));
    assert!(
        source.contains("Err(TriggerRefusal::Slash)"),
        "`/` is refused explicitly, not by falling off the end"
    );
    assert!(source.contains("is_whitespace()"), "whitespace is refused");
    assert!(
        source.contains("(Some(first), None)"),
        "the prefix must be exactly one character"
    );
}

/// `@` is the built-in provider's. The chain must decline it *before* considering its
/// own registrations, or an attachment turns into an issue picker.
#[test]
fn the_chain_declines_the_at_sign_before_consulting_its_triggers() {
    let source = code_only(&read("crates/lca-tui/src/widgets/autocomplete/trigger.rs"));
    let owner = source
        .find("fn owner")
        .expect("the arbitration function exists");
    let body = &source[owner..];
    let at_guard = body
        .find("starts_with('@')")
        .expect("`@` is declined explicitly");
    let arbitration = body
        .find("for trigger in &self.triggers")
        .expect("the registration loop exists");
    assert!(
        at_guard < arbitration,
        "`@` must be declined before any registration is considered"
    );
}

/// A failure or a timeout is one row, never a modal and never a hang. A later cycle
/// could "improve" the timeout into an error dialog; this row is why it cannot.
#[test]
fn a_failure_and_a_timeout_are_both_one_dim_row() {
    let source = code_only(&read("crates/lca-tui/src/widgets/autocomplete/trigger.rs"));
    assert!(
        source.contains("recv_timeout(self.budget)"),
        "the wait is bounded by the budget"
    );
    assert!(
        source.contains("RecvTimeoutError::Timeout"),
        "a timeout is handled, not panicked on"
    );
    // Both the failure and the timeout land on the same row constructor.
    assert!(
        source.matches("Self::unavailable(").count() >= 2,
        "a failure and a timeout render the same single row"
    );
    assert!(
        source.contains("dim: true"),
        "the row is dim: it is news, not a choice"
    );
}

/// Abort is a generation, not a timer: a stale answer is discarded by comparison.
#[test]
fn a_superseded_result_is_discarded_by_generation() {
    let source = code_only(&read("crates/lca-tui/src/widgets/autocomplete/trigger.rs"));
    assert!(
        source.contains("fetch_add(1"),
        "each query takes a new generation"
    );
    assert!(
        source.contains("self.generation.load") && source.contains("!= mine"),
        "a result is compared against the generation that asked for it"
    );
}

/// The row's origin is a typed field. The editor must not be handed a string it has
/// to parse back out (gh #408 design point 4).
#[test]
fn the_origin_is_a_typed_field_not_a_string_in_the_label() {
    let items = code_only(&read("crates/lca-tui/src/widgets/autocomplete.rs"));
    assert!(
        items.contains("pub origin: Option<String>"),
        "the item carries its origin as data"
    );
    let trigger = code_only(&read("crates/lca-tui/src/widgets/autocomplete/trigger.rs"));
    assert!(
        trigger.contains("item.origin = Some("),
        "the chain sets the typed field"
    );
    // And the popup composer is the only place the bracket form is built. Keyed on
    // shape rather than one formatted line: `cargo fmt` reflows a method chain, and a
    // guard that breaks when the formatter runs is a guard that gets deleted.
    let editor = code_only(&read("crates/lca-tui/src/widgets/editor.rs"));
    let popup = editor
        .find("pub fn render_popup")
        .expect("the popup renderer exists");
    let body = &editor[popup..];
    let end = body.find("\n    }\n").unwrap_or(body.len());
    let body = &body[..end];
    assert!(
        body.contains(".origin"),
        "the renderer reads the typed origin field"
    );
    assert!(
        body.contains("[{o}]"),
        "and builds the bracket form from it, in one place"
    );
}

/// The native provider proves the seam from `lca-ui`, the host layer. `lca-tui` may
/// not shell out or reach the network (ADR-0036), so a `std::process` call there
/// would be an architectural regression.
#[test]
fn the_first_party_provider_lives_in_the_host_and_the_engine_stays_world_less() {
    let provider = read("crates/lca-ui/src/autocomplete_issues.rs");
    assert!(
        provider.contains("Command::new(\"gh\")"),
        "the provider shells out from the host"
    );
    let engine = code_only(&read("crates/lca-tui/src/widgets/autocomplete/trigger.rs"));
    assert!(
        !engine.contains("std::process") && !engine.contains("Command::new"),
        "the engine knows nothing about processes"
    );
}

/// The trigger is registered over the built-in provider, so `/` and `@` win by
/// structure rather than by ordering luck.
#[test]
fn the_trigger_is_wrapped_over_the_builtin_provider() {
    let source = code_only(&read("crates/lca-ui/src/chat_commands.rs"));
    assert!(
        source.contains("TriggerChain::new(inner"),
        "the chain decorates the built-in provider"
    );
    assert!(
        source.contains("chain.register("),
        "and a trigger is registered on it"
    );
}
