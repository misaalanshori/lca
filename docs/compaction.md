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

## Split spans (phase 2)

Records are atomic, so a span straddling the cut splits at record
granularity: the span head and prefix summarize with the range, the
tail stays verbatim from the window head, and the tool-pair rule
still holds across the split. (Divergence, documented: pi splits
inside one turn with a dedicated prefix prompt and a merged second
summary; LCA's single summary covers prefix and history together.)

## Iterative summaries (phase 2)

The latest summary rides in-band as the candidate's first record — a
`custom` record with `custom_type: "previous-summary"` — so the
strategy refines instead of restarting, with no WIT change. The range
computation skips it; a first compaction is unchanged. The marker text
is capped at 4000 characters (and the strategy caps every excerpt at
4000 characters as a backstop); the prompt wraps it in
`<previous-summary>` tags with an iteration instruction, and the
mechanical fallback carries it as "Earlier summary".

## File tracking (phase 3)

Compaction records carry cumulative `read_files` / `modified_files`
lists (pi's `CompactionDetails`): tool calls in the summarized range
plus the lists of earlier compactions in scope, so repeated
compactions accumulate the same picture. A file both read and
modified counts as modified. The summarizer sees the lists appended
to the summary text in pi's `<read-files>` / `<modified-files>`
shape, when relevant; a fileless range reads unchanged. Divergence,
documented: each list is capped at 200 sorted entries (bounded
storage); pi tracks unbounded sets. Only `read` / `write` / `edit`
calls count, pi's mapping.

## System-message checkpoint (phase 3)

Every compaction checkpoints the system prompt on the record (pi
snapshots the system message onto the entry and replays it before
the summary). A later compaction whose prompt differs from the
latest checkpoint appends a `custom` record of type
`system-prompt-change` first: the detection, not a migration.

## Overflow recovery ordering (phase 3)

A provider failure matching the overflow patterns (`token cap`,
`prompt is too long`, `exceeds token limit`, `max_tokens …
exceed`, `context window` / `length`) compacts and retries the turn
once, instead of ending it dead. The aborted attempt stays visible
to `TurnEnded` (error), the `overflow` compaction runs, and the
retry starts fresh on the compacted log; a still-capped retry
surfaces, and a failed recovery compacts nothing and retries
nothing. Divergence, documented: the aborted attempt's partial text
is not persisted (LCA never persists failed attempts) — the error
marks the boundary. Recovery respects `compaction.enabled`: opted
out means no strategy to compact with, so the error surfaces.

## Retain-none (phase 3)

A `/compact` that keeps nothing anchors the record's own id as
`first_kept_id` (pi's shape: `firstKeptEntryId = own id`); the next
plan starts after the entry instead of the session start. Nothing is
declined: manual compaction is the only retain-none path, and it
follows pi exactly.

## Later phases (explicitly not this cycle)

None: the #36 epic is complete. The symmetrical head-preservation
proposal (`keep_initial_tokens`, raised in review) stays out of
scope: it is not pi behavior.
