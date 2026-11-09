//! NFR-24 guard for gh #390: a truncation is reported whatever else happened, and a
//! diagram drafted in reasoning is source rather than a second drawing.
//!
//! # Gap 1 - the silent truncation
//!
//! `stopReason == Length` (the wire's `finish_reason: "length"` /
//! `stop_reason: "max_tokens"`) had **no representation in the protocol at all**: a
//! truncated response arrived as an ordinary "stop", so
//!
//! 1. nothing told the user the answer was cut,
//! 2. a call cut mid-arguments was treated as runnable (its JSON was a prefix), and
//! 3. because nothing marked the response as cut, the turn **re-requested and
//!    re-attempted the same incomplete call in a loop** - observed live as the same
//!    `write a.txt error` card repeating.
//!
//! pi's rule is that a `length` stop *always* appends
//! `Response was truncated before completion.`, **including with tool calls
//! present**, because the previewed call may never have executed - and the presence
//! of tool calls is exactly the case where the old shape stayed silent.
//!
//! # Gap 2 - the double diagram
//!
//! A ```` ```mermaid ```` fence in a reasoning run is a draft; in the answer it is
//! the artefact. pi skips the mermaid transformer for `assistant-thinking`.
//!
//! # Honesty: was gap 2 reachable?
//!
//! **No.** Reasoning renders through `wrap_text_with_ansi`, not `render_markdown`,
//! so before this cycle the double-render could not occur. The guard at the block
//! renderer is **prevention**: it holds the line for the moment reasoning is routed
//! through the markdown pipeline (the shape pi uses, and the shape the new
//! `AssistantThinking` type exists for). The row that pins the observable
//! (`a_diagram_drafted_in_reasoning_is_drawn_once`) lives beside the transcript.
//!
//! # The error-digest debt, deliberately not paid here
//!
//! `push_error` stores raw provider/tool strings with no digest, no remediation and
//! no collapsed/expanded states. That is a separate design - what a digest *is*, and
//! how a remedy is chosen - so it is named in the cycle report rather than invented
//! as a rider. The truncation banner's own copy *does* carry a remedy, because the
//! remedy for a truncation is known: give the model room.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(rel: &str) -> String {
    let path = root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {path:?}: {err}"))
}

// Verifies: gh #390 gap 1 - **the protocol has a way to say "truncated".** Without
// a representation, the wire's `length` fact cannot reach the interface at all, and
// every rule below has nothing to act on. This is the structural half; the
// behavioural rows live beside the code (`lca-wire-openai`'s decoder tests,
// `lca-core`'s turn rows, `lca-ui`'s banner rows).
#[test]
fn the_protocol_can_represent_a_truncation() {
    let turn = read("crates/lca-protocol/src/turn.rs");
    assert!(
        turn.contains("Length,"),
        "StopReason carries the truncation case"
    );
    assert!(
        turn.contains("max_tokens"),
        "and its doc names the wire spelling a reader will meet"
    );
    let stream = read("crates/lca-protocol/src/stream.rs");
    assert!(
        stream.contains("Truncated,"),
        "StreamEvent carries the wire fact"
    );
    // It must not be an `Error`: a truncation is not a failure, so a retry would
    // reproduce the cut and a script would read a usable answer as a crash.
    let truncated_doc = stream
        .split("Truncated,")
        .next()
        .expect("the variant exists");
    let tail = &truncated_doc[truncated_doc.len().saturating_sub(1200)..];
    assert!(
        tail.contains("not") && tail.contains("Error"),
        "the variant is documented as a fact rather than a failure: {tail}"
    );
}

// Verifies: gh #390 gap 1 - **both wire kits emit it**, for their own spelling of
// the same fact: OpenAI's `finish_reason: "length"`, Anthropic's
// `stop_reason: "max_tokens"`. One kit alone would leave half the providers silent,
// which is the defect one layer down.
#[test]
fn both_wire_kits_report_the_cut() {
    let openai = read("crates/lca-wire-openai/src/chat_completions.rs");
    assert!(
        openai.contains("StreamEvent::Truncated"),
        "the Chat Completions decoder emits the truncation"
    );
    // ...and it is emitted *before* the budget check, so an ordinary chat turn
    // (no `max_tokens` of ours) is no longer quiet about the endpoint's own limit.
    let length_arm = openai
        .find("Some(\"length\")")
        .expect("the length arm exists");
    let arm = &openai[length_arm..];
    let emit = arm.find("StreamEvent::Truncated").expect("it emits");
    let budget = arm.find("if let Some(budget)").expect("the budget check");
    assert!(
        emit < budget,
        "the truncation is unconditional, not gated on a budget we set (that gate \
         is what made gh #169 quiet for ordinary turns)"
    );
    let anthropic = read("crates/lca-wire-anthropic/src/messages.rs");
    assert!(
        anthropic.contains("\"max_tokens\"") && anthropic.contains("StreamEvent::Truncated"),
        "Anthropic's spelling maps to the same fact"
    );
}

// Verifies: gh #390 gap 1 - **the turn ends as `Length` before any call runs.** The
// order is the correctness argument: a truncated response's arguments are a JSON
// prefix, so dispatching one runs against a value the model never finished writing;
// and without ending the turn, the loop re-attempts the same call.
#[test]
fn a_truncation_is_decided_before_a_tool_runs() {
    let turn = read("crates/lca-core/src/turn/mod.rs");
    let check = turn
        .find("if response.truncated {")
        .expect("the turn checks the truncation");
    // Both the block-building and the record-writing loops filter on it.
    assert_eq!(
        turn.matches(".filter(|_| !response.truncated)").count(),
        2,
        "the assistant block AND the record loop both omit a truncated call - one \
         writer alone leaves the half-written arguments in the log for a resume"
    );
    assert!(
        check
            < turn
                .find("for call in &response.calls")
                .unwrap_or(usize::MAX),
        "the truncation is decided before the dispatch loop"
    );
}

// Verifies: gh #390 gap 1 - **the banner is unconditional on the reason, and lives
// in the one place a turn ends.** A row asserting only "there is a banner" would
// pass with the old suppression intact if a control path happened to show it; this
// asserts the shape that makes it *always* shown - one site, no `hasToolCalls`
// condition - plus the copy's remedy.
#[test]
fn the_banner_is_unconditional_and_carries_a_remedy() {
    let events = read("crates/lca-ui/src/chat_turn_events.rs");
    assert!(
        events.contains("if stop_reason == StopReason::Length"),
        "the banner is keyed on the reason alone"
    );
    // The suppression shape must not come back: a `hasToolCalls`-style guard is the
    // exact defect (the banner vanished *because* a call was present).
    assert!(
        !events.contains("has_tool_calls"),
        "no `has_tool_calls` gate may reappear: the presence of a call is the case \
         where the banner matters most"
    );
    // Exactly one site appends it.
    assert_eq!(
        events.matches("push_error(TRUNCATED_NOTICE)").count(),
        1,
        "one banner site, so it cannot double-report"
    );
    // The copy says what to do (AGENTS.md's error-copy rule).
    let notice = events
        .split("pub const TRUNCATED_NOTICE")
        .nth(1)
        .expect("the notice exists");
    // The declaration spans a `\`-continued line, so the literal's end is the
    // closing quote of the string, not the first `;` (which a doc comment above it
    // could also carry). Reading to the terminating `";` is what makes this
    // independent of how `cargo fmt` wraps it.
    let notice = &notice[..notice
        .find("\";")
        .map(|end| end + 1)
        .unwrap_or(notice.len())];
    for needle in ["max_tokens", "/compact", "truncated"] {
        assert!(
            notice.contains(needle),
            "the notice names `{needle}`: {notice}"
        );
    }
}

// Verifies: gh #390 gap 2 - **the reasoning message type exists and the mermaid gate
// asks it.** pi draws the line by message type (`assistant-thinking` skips the
// transformer) rather than by sniffing content, which is what makes the rule hold
// for any future artefact transformer.
#[test]
fn the_reasoning_type_gates_the_artefact_renderer() {
    let transform = read("crates/lca-tui/src/widgets/markdown/transform.rs");
    assert!(
        transform.contains("AssistantThinking,"),
        "the message type carries the reasoning case"
    );
    assert!(
        transform.contains("fn is_reasoning"),
        "and one predicate answers it, so a second transformer has one thing to ask"
    );
    let markdown = read("crates/lca-tui/src/widgets/markdown/mod.rs");
    assert!(
        markdown.contains("_ if reasoning => None,"),
        "the mermaid gate skips the reasoning type"
    );
    // The control: mermaid still renders for the ordinary assistant type, so the
    // skip cannot have disabled diagrams everywhere.
    assert!(
        markdown.contains("crate::widgets::mermaid::render("),
        "art is still produced for answers"
    );
}

// Verifies: gh #390 gap 2 - **the ABI adapter forwards the new type rather than
// folding it into `Assistant`.** An extension that registers an artefact
// transformer needs the same distinction the built-in mermaid path uses, and a `_`
// arm here would silently label a reasoning run as an answer.
#[test]
fn the_extension_adapter_forwards_the_reasoning_type() {
    // Gh #543: read the module - `chat/`-style partitions of `tui/mod.rs` now
    // exist (`cells.rs`, `open.rs`), and a row that reads one filename stops
    // seeing the code a split moves there.
    let adapter = crate::module_text("crates/lca-cli/src/tui/mod.rs");
    assert!(
        adapter.contains("lca_ui::MarkdownMessageType::AssistantThinking"),
        "the adapter maps the reasoning type explicitly"
    );
    let abi = read("crates/lca-ext-abi/src/lib.rs");
    assert!(
        abi.contains("AssistantThinking,"),
        "the Rust-side ABI enum carries it (no WIT declaration, so this is additive)"
    );
}

// Verifies: gh #390 gap 1 - **the cut survives the WASM boundary.** Most providers are
// delivered through the subscription kit, and the WIT stream has no truncation case,
// so a guest re-encodes `Truncated` as the ABI's reserved `vendor-event` and the host
// maps it back. Without both halves the interface would receive a truncated answer as
// a complete one for every extension-delivered provider - the same defect this cycle
// fixes, one hop away.
//
// Source-level because the two halves live in different crates (a guest compiled for
// `wasm32-wasip2` and the host that maps its events); the behavioural pair is the
// wire decoder's own rows plus the turn's.
#[test]
fn the_truncation_survives_the_guest_boundary() {
    let kind = read("crates/lca-protocol/src/stream.rs");
    assert!(
        kind.contains("pub const TRUNCATED_KIND"),
        "the vendor-event kind is a named constant, not a literal in ten guests"
    );
    // Every guest that maps the stream must re-encode it. The subscription kit is the
    // shared one (most providers), so it is asserted first and separately.
    for guest in [
        "crates/lca-subscription/src/wasm.rs",
        "extensions/llama/src/wasm_provider.rs",
        "extensions/conformance/src/provider_world.rs",
    ] {
        let source = read(guest);
        assert!(
            source.contains("P::Truncated =>"),
            "{guest} must re-encode the truncation, or its providers lose it"
        );
    }
    // ...and the host must map it back, or the round trip preserves nothing.
    let host = read("crates/lca-ext-host/src/host_imports.rs");
    assert!(
        host.contains("TRUNCATED_KIND =>"),
        "the host maps the vendor event back to `Truncated`"
    );
    assert!(
        host.contains("P::Truncated"),
        "and the mapping produces the real variant, not another vendor event"
    );
}
