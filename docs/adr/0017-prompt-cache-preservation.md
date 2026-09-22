# ADR-0017: Prompt cache preservation and measurement

Status: accepted.

Date: 2026-09-20.

## Context

Every major provider's prompt caching is prefix-based: identical leading content across requests is served from cache, and the first byte that differs from a previous request invalidates everything after it. Anthropic's caches default to a five-minute TTL; providers vary in whether they report cache activity at all, and whether they report writes as well as reads. A design that doesn't take this seriously pays full price on every turn without anyone noticing, since a cache miss looks identical to a cache hit from the outside; the only evidence is a usage field a provider may or may not surface.

Two things about this design in particular put prefix stability at risk in ways a simpler agent wouldn't have. The `context-transform` world runs on every single turn and returns a transformed message list; if a badly-written transform touches content earlier in the list than it needs to, it silently busts caching for the rest of the conversation, and nothing before this record would have caught it. And provider extensions vary in whether they even know where the stable part of a conversation ends, since the message list handed to `stream-completion` carries no signal about which prefix is safe to mark cacheable.

Pi already solves the measurement half of this problem, and solves it well: `cache-stats.ts` scans session history and computes, per assistant turn, how many prompt tokens should have been cache reads based on the previous turn's prompt size, and were not. It resets its baseline on a `compaction` or `branch_summary` entry, since the prompt genuinely changed there and re-billing is expected, not waste. It does not exempt a model switch, on the explicit reasoning that a switch re-bills the full prompt and that is real, visible cost, not free churn. It applies a noise floor, 1024 tokens, below which a miss is breakpoint-granularity noise rather than a real regression. It distinguishes a provider that reports cache reads but never writes from one that never reports caching at all, so the latter doesn't show as a permanent 100 percent miss rate for having nothing to measure.

None of that needed foreknowledge of where a request's cacheable boundary was going to be; it works entirely from what the provider reported after the fact.

## Decision

Adopt pi's measurement approach directly, and add one architectural piece pi does not need: a host-computed cache boundary, because pi has no per-turn transform pipeline sitting between its log and its provider call, and this design does.

**Measurement**, closely following pi's design: the `usage` event in the provider stream gains `cache_read`, `cache_write`, and `cache_write_1h` fields alongside the existing `input` and `output` token counts, mirroring Anthropic's extended one-hour cache tier, which is billed at roughly twice the base input rate for the write. A dedicated pass over the session log, run wherever the status line or a stats command needs a number, computes cumulative cache waste: for each assistant turn, compare this turn's prompt token count against the previous turn's, subtract what was actually served from cache, and count anything over the noise floor as a miss with its own dollar cost, computed from that turn's own effective paid-versus-cache-read rate rather than a static price table. The baseline resets on a `compaction` record, since the prompt legitimately changed and the next turn's tokens are new content, not re-billed content. It does not reset on a model switch; that is counted, on the same reasoning pi already states explicitly. A provider that has never once reported cache activity in a scan segment is treated as having nothing to measure, not as a permanent miss, distinguishing "this provider doesn't do caching" from "this provider's cache missed."

**The cache boundary**, which is new: when the host assembles the resolved message list for a turn, per the context-assembly flow in `docs/flows.md`, it already knows exactly where the stable region ends, since that is the same boundary compaction's own durable record marks. The host passes this boundary to the provider extension alongside the message list on the `stream-completion` call, as a count of leading messages considered stable. A provider extension uses it, where its vendor has an explicit cache-control mechanism, to place the marker at the right position; a provider without one, including a local server with no cache concept at all, ignores it safely, since it is advisory.

Where a `context-transform` extension's output differs from its input within that stable region, amended before implementation to compare against the previous turn's stable content rather than the turn's own input, see Consequences, the host does not reject the turn. It records the divergence as an extension event, the same category `docs/threat-model.md` already uses for a capability denial, and narrows the boundary it reports to the provider for that turn to end just before the divergence, so the request still goes out correctly, just without a cache marker over the part that changed. This follows pi's own posture: measure and surface, don't block. A transform extension that does this repeatedly is a visible, diagnosable problem, not a silent one, and not a hard failure either.

## Alternatives considered

A host-enforced, content-hashed boundary that rejects a context-transform extension's output outright if it touches the stable region. This was the first version of this record. It is more aggressive than pi's own architecture, which never needed to solve this problem at all, and it turns an extension author's mistake into a failed turn rather than a visible, correctable diagnostic. Rejected in favor of the narrow-and-record behavior above, which keeps the turn working and still makes the problem obvious.

Detecting cache misses by hashing and comparing the literal request content sent on each turn, rather than reading provider-reported usage numbers. This is what a first pass at this record assumed before reading pi's actual implementation. It is more code, it requires keeping a full copy of every previous request's cacheable content around, and it cannot account for a provider's own cache TTL expiring between requests, which pi's usage-number approach naturally reflects since an expired cache simply reports as a normal miss. Provider-reported usage is strictly better signal and is what pi actually uses.

No cache-boundary hint at all, leaving every provider extension to infer the stable region itself from the message list's shape. This asks every third-party provider author to reverse-engineer something the host already knows exactly, and gets it wrong differently per author. The host computing it once, from the same compaction record that already exists, costs little and removes the guesswork.

## Consequences

The `provider` world's `usage` event and the `stream-completion` call both gain fields, joining the ABI's pre-freeze punch list in `docs/abi-versioning.md` alongside the other additions already queued there.

`lca-testkit`'s fake provider needs to script realistic per-turn usage numbers, including cache read and write counts, so a test can construct both a clean multi-turn scenario expecting zero waste and a regressed one expecting a specific, countable miss, without touching a real API. This is the primary mechanism `docs/testing-plan.md` uses to make cache behavior testable at all.

The cache-waste figures are worth surfacing to the user directly, the way pi does, through the existing session stats and cost reporting rather than a new command.

Every provider profile under `docs/providers/` that talks to a real hosted vendor should say plainly whether and how it uses the boundary hint; a local server with no cache concept says so too, so the absence reads as expected rather than as an oversight.

**Divergence is measured cross-turn.** The comparison is against what was actually sent on the previous turn, not against the turn's own input. A transform that deterministically rewrites stable content identically every turn is cache-stable in fact, and the cross-turn measure records one divergence and then settles instead of narrowing every turn. A transform whose output drifts turn to turn keeps the boundary narrowed, which is the honest outcome: those requests really do bust the cache. The first divergence narrows the boundary to end before the earliest differing message.

## Revisit conditions

A provider whose caching model doesn't fit the leading-prefix shape this design assumes, which would need its own boundary representation rather than a single leaf-count. Evidence that the narrow-and-record behavior for a transform touching the stable region is too permissive in practice, which would argue for tightening it, though not for returning to outright rejection without first trying a stronger warning.
