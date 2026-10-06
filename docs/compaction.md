# Compaction

Automatic context compaction (gh #36, pi's `compaction.md` shape on our
record vocabulary). FR-SESS-4/FR-SESS-5 are the requirements; the
`compaction` record is documented in `docs/session-log-format.md`.

## Trigger formula

```
context_tokens > context_window − reserve
```

strict on both sides: equality does not fire. `context_tokens` is the
latest assistant call's prompt tokens (input + cache fields);
`context_window` is the model's published window, or the 128k fallback
when the endpoint publishes none. `reserve` is `compaction.reserve_tokens`
when set, else the stopgap's fraction derivation `(1 − threshold) × window`
— so the default behavior is the old threshold by construction, and an
absolute token budget is opt-in. `compaction.enabled = false` skips the
check without error. The check runs once per turn, before the first
provider call; there is no per-model override rung (the settings ladder
has none — a documented divergence from pi's `compaction.modelOverrides`).

## Summarization budget

The summary request carries `max-tokens = floor(0.8 × reserve)` (pi's
derivation), falling back to the 4096 stopgap constant when the reserve
is zero. It rides `CompletionRequest.extras["max-tokens"]`, as before.

## Cut-point rules (phase 1)

The summarized range runs from the previous compaction's kept boundary
(`first_kept_id`, or the entry after that compaction when the id is gone
from the log, or the session start) to the keep-recent window's head.
The window walks back from the turn accumulating eras — an assistant
call's context size, other records inheriting the following call's —
and keeps everything within `keep_recent_tokens` of the latest call
(`0` keeps nothing: the whole range summarizes). The head then snaps
back past tool-pair interiors: a kept tool result rejoins its call, a
summarized call rejoins its kept result. A call and its results never
split across the cut. The record anchors the boundary in `first_kept_id`,
so the next compaction starts there.

## Later phases (explicitly not this cycle)

Split user-message spans, iterative previous-summary context,
cumulative file tracking, the system-message checkpoint, overflow/length
recovery ordering, retain-none. The trigger, the cut planner
(`crates/lca-core/src/compact.rs`), and the record field carry
extension-point comments naming each one; none is built here. The
symmetrical head-preservation proposal (`keep_initial_tokens`, raised
in review) is out of scope: it is not pi behavior.
