# ADR-0015: Compaction and context transform as separate worlds

Status: accepted.

Date: 2026-09-20.

## Context

Two different needs were initially discussed as one. The first is compaction: replacing an old range of session records with a summary when token usage crosses a threshold, already specified at the storage level in `docs/session-log-format.md`, where the log stays immutable and a `compaction` record marks what a reader should substitute. The second is a generic way to reshape the message list on its way out to the model on every turn, without touching anything stored, for cases like redaction before the request leaves the machine, injecting a synthetic context line, or a skills extension adding instructions based on matching the latest message against a set of skill descriptions.

Both reached for the word middleware during design discussion, and the question was whether that meant they should be the same mechanism.

They should not, for two reasons independent of taste. Compaction is threshold-triggered and meant to run rarely, with its result cached and reused across many subsequent turns; a generic transform is meant to run on every single outbound request and stay cheap. Folding compaction into a generic transform chain means either every transform author reinvents memoization to avoid re-summarizing every turn, or the host special-cases "this particular transform's result gets persisted," which puts compaction back to being a special case wearing a generic mechanism's clothes. Compaction also writes something durable, a log record visible in later reads and in exports, while a generic transform should be able to touch nothing durable at all, since giving every transform author log-write access so compaction can share their mechanism is a larger grant than redaction or skills injection should ever need.

A further complication surfaced once a real default compaction strategy was considered: summarizing a truncated range well generally means asking a model to produce the summary, which needs the ability to request a completion from whichever provider is active. That capability, described generically as service access in ADR-0008, was deliberately left out of the 1.0 capability set pending a real forcing case rather than a speculative one. Compaction's own default implementation is that case.

## Decision

Two separate worlds, not one.

`compaction` is threshold-triggered and durable. The host calls it when configured usage crosses a threshold or the user runs a manual compact command, passing the candidate record range; the extension returns a summary, which the host writes as a `compaction` record exactly as the storage format already specifies. An extension implementing `compaction` typically also implements `command`, for the manual trigger, which needs no new mechanism since worlds already compose freely.

`context-transform` is per-turn and ephemeral. It exports one function, taking the resolved message list about to go out and returning either a transformed list or a rejection. The host chains every enabled transform extension in a defined order, each one's output feeding the next; a rejection ends the turn with that reason surfaced, the same shape a hook denial already uses. Ordering for 1.0 is simple: installation order, or a manifest priority number if that proves necessary once real transforms exist. Nothing about this world touches the session log. Compaction's cached view is what a transform extension sees as input; the transform's own output is never itself persisted.

`completion` is added to the 1.0 capability set. It lets a granted extension ask the host for a response from whichever provider is currently active, routed the way any other host-mediated service access is, keeping the extension graph a star with the host at the center rather than allowing extension-to-extension calls, consistent with the reasoning in ADR-0008. This revises the capability catalog's 0.1 listing, which had described completion access as deliberately absent pending evidence; the default compaction strategy needing it is that evidence, since shipping a flagship feature whose only strategies are mechanical truncation is a materially worse default than being able to summarize well.

Extensions that do not need `completion` are unaffected. A mechanical compaction strategy, drop the oldest N turns, keep only tool results that errored, keep the first and last K turns, works with no new capability at all. A redaction transform, the PII-sanitization case raised during design discussion, likewise needs nothing beyond what it already would to reach whatever service it checks against; it does not need `completion` unless a particular implementation chooses to use one.

Hooks are unchanged by this record. They remain about gating discrete events, tool calls and turn boundaries, with allow, deny, or replace semantics. Neither compaction nor context transform is folded into hooks, and hooks do not grow a context-injection power of their own; the case that motivated considering that, skills-handling injecting instructions based on matching the latest message, is served by `context-transform` instead.

## Alternatives considered

One merged world covering both compaction and generic transforms, with the host distinguishing durable from ephemeral results by some field the extension sets. This was the original framing under discussion. It survives contact with the caching and durability differences above only by growing special cases inside the one world, which is a worse outcome than two small, honestly-scoped worlds.

Folding context transformation into the existing `hooks` world by adding a content-injection return value to hook points. This was proposed and then dropped during design discussion in favor of a dedicated world, because hooks are shaped around gating events with a verdict, not around reshaping a list of messages, and stretching one world's return type to cover a second, different job blurs a boundary worth keeping clean.

Deferring `completion` access further, shipping compaction 1.0 with mechanical strategies only. This was a live option and is noted here because it was seriously considered: it keeps the capability set as conservative as ADR-0008 originally intended, at the cost of the default compaction experience being noticeably worse than what the extension model is otherwise capable of. Decided against, on the view that a real first-party consumer of the capability, not a speculative one, is exactly the evidence ADR-0008 said would justify adding it.

## Consequences

`completion` needs the same treatment as every other capability: an entry in the catalog, a manifest schema definition, a conformance extension case, and a threat model scenario, since it is the first capability that lets an extension consume model output as an input to its own logic rather than only producing text or tool calls for the model to see. That is a meaningfully different shape of access and deserves its own review attention.

The provider world's exports, expanded under ADR-0012, are what `completion` calls through internally; the host resolves "whichever provider is currently active" the same way the generic `/usage` command does.

Two new worlds join the pre-freeze punch list in `docs/abi-versioning.md`, alongside the provider world's login, logout, and usage additions.

## Revisit conditions

Evidence that per-turn transform chains need an ordering mechanism beyond a manifest priority number, once more than a small number of transform extensions exist together in practice.
