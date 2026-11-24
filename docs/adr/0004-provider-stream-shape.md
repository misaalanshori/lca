# ADR-0004: Typed event stream for the provider world

Status: accepted.

Date: 2026-09-20.

## Context

A provider extension receives a streaming response from a model API and passes it to the host. The shape of what crosses that boundary decides how much of the WIT surface the provider world needs.

The tempting simplification is to pass text. The extension forwards the model's text output and the host parses whatever structure it needs out of that text.

Streaming tool calls break the simplification. Every major model API streams tool call arguments as partial JSON fragments, interleaved with text, and each fragment belongs to a specific call. The host has to know which call a fragment belongs to. That association is structure, and a text channel cannot carry structure without a framing format wrapped around it.

## Decision

The provider world exports a stream of typed events. The variant has cases for text delta, reasoning delta, tool call start, tool call argument delta, tool call end, usage, error, and a reserved `vendor-event`.

`vendor-event` carries a kind string and a JSON payload. It exists from the first release because adding a case to a WIT variant breaks the canonical ABI. Anything the typed cases do not cover travels in `vendor-event` until it earns a typed case in the next major ABI version. Without this case, the first vendor concept that does not fit forces an ABI break.

The stream is a resource with a pull interface. The host calls a next function that returns the next event or end of stream, and drives it from an async task. WASI 0.3 native async streams are not used, because the provider path is the wrong place to depend on the newest part of the specification. The resource shape maps onto native async later without changing the event variant.

The host accumulates tool call arguments. The extension emits a start event with a call identifier and a tool name, then argument deltas keyed by that identifier, then an end event. The host joins the fragments and parses the result. The extension never needs the tool schema, which it has no reason to know.

## Alternatives considered

An opaque text stream with host-side parsing. It gives the smallest WIT surface and it absorbs any future vendor concept without an ABI change. It needs a framing format to carry call identifiers, which reintroduces structure as strings, and it makes the host parse twice: once to find frames, once to read the JSON inside them. Type checking disappears. Rejected.

A typed envelope with an opaque JSON payload for every event. Smaller WIT than the full variant, and vendor extras pass through with no special case. The payload is unchecked, so every field error moves from compile time to run time, in the code path that runs on every token. Rejected, though `vendor-event` keeps its one good property.

Extension-side accumulation, where the extension emits only complete tool calls. It simplifies the host and it removes the ability to show argument text as it streams, which is a real interface feature. It also puts JSON assembly in every provider extension instead of in one place.

## Consequences

The provider world is the largest WIT surface in the ABI. It is also the one that changes least often, because model APIs converge on the same event shapes.

`lca-protocol` carries a stream event type that mirrors the WIT variant, case for case. The mapping is generated rather than written by hand, so the two cannot drift.

The accumulator in `lca-provider` needs tests for the failure paths: a delta with no start, an end with no start, two starts with the same identifier, and a stream that ends with a call still open.

Two requirements follow from this record. The provider extension SHALL emit a tool call start event before any argument delta for that call. IF an argument delta arrives for an identifier with no open start event, THEN the host SHALL discard the delta and record a protocol error.

## Revisit conditions

Phase 0 measurements showing per-event overhead high enough to affect perceived latency. A WASI 0.3 async release stable enough to replace the pull resource, which would be an additive change rather than a reversal.

## Addendum: 2026-11-24 — the first concept that earned its case

**Annotated, not rewritten** (AGENTS.md's ADR rule). The decision above stands; this
records the first time the escape hatch did the job it was designed for, and what the
transition cost.

The output-limit cut (`finish_reason: "length"` / `stop_reason: "max_tokens"`) arrived
before 0.7 as `vendor-event("truncated", "")`, because a little-used concept should not
force a line move by itself (gh #390). When the 0.7 line opened for gh #407, `stream-event`
gained the payload-free `truncated` case (gh #581) and every in-tree guest now emits it.

Three things this confirms rather than changes:

- **The prediction held.** "Adding a case to a WIT variant breaks the canonical ABI" — it
  did, and it was priced correctly: a line move, not a patch on a released line. The
  window still admits older *manifests*, and the package rename still means an older
  binary is refused at link with an actionable sentence, exactly as the 0.7 line's
  annotation in `docs/abi-versioning.md` describes.
- **`vendor-event` remains the hatch.** This case is now typed; the portal around it is
  untouched, and the next vendor concept still travels through it. The case list above is
  the pre-0.7 one and is left as written on purpose — the current list is
  `wit/world-provider.wit`, which is normative.
- **One hop of the transition is a compatibility read, for one minor version.** The host
  still maps `vendor-event("truncated", "")` to the same fact, because a component built
  against `@0.7.0` before the case landed can still send it and dropping the read would
  turn its cut answer into a complete one silently. The typed case wins when both arrive.
  The removal condition (first minor release after this one, no in-tree guest emitting the
  kind) is named where the read lives (`crates/lca-ext-host/src/host_imports/wit.rs`) and
  checked by `tests/regressions/581-typed-truncated-event.rs`.
