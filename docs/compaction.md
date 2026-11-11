# Compaction

Automatic context compaction (gh #36, pi's `compaction.md` shape on our
record vocabulary). FR-SESS-4/FR-SESS-5 are the requirements; the
`compaction` record is documented in `docs/session-log-format.md`.

## Trigger formula

```
context_tokens > context_window − reserve
```

strict on both sides: equality does not fire. `context_tokens` is the
latest assistant call's prompt tokens (input + cache fields) — the
**merged** usage report, never just the last stream event's, because a
provider may split its counters across events (gh #276, Anthropic's
`message_start` + `message_delta`); a whole-struct overwrite there read
zero and this trigger could never fire;
`context_window` is the model's published window, or the 128k fallback
when the endpoint publishes none. `reserve` is `compaction.reserve_tokens`
when set, else the stopgap's fraction derivation `(1 − threshold) × window`
— so the default behavior is the old threshold by construction, and an
absolute token budget is opt-in. `compaction.enabled = false` skips the
check without error. The check runs once per turn, before the first
provider call; there is no per-model override rung (the settings ladder
has none — a documented divergence from pi's `compaction.modelOverrides`).

## Summarization budget

The summary request carries `max-tokens = min(floor(0.8 × reserve), model max
output)` (pi's derivation), where `reserve` is the same one the trigger uses
(`compaction.reserve_tokens` when set, else `(1 − threshold) × window` over the
model's published window, with the 128k estimate standing in when the endpoint
publishes none). So the budget **scales with the model**, and it is computed by
one rule in `lca_core` (`ext_provider::summarization_budget`). It rides
`CompletionRequest.extras["max-tokens"]`, as before.

The **output-ceiling cap** is pi's `Math.min(…, model.maxTokens)` and it is not
decorative: a provider validates `max_tokens` against the model's own ceiling, so
a 200,000-token reserve on a model that can emit 8,192 would be a *rejected
request*, not a summary. A model that publishes no ceiling (`0`, which is what
the bundled `openai-compatible` provider reports) caps nothing, and the derived
number stands.

**The rule: derived from the reserve, never the stopgap once the window is
known.** `SUMMARIZATION_MAX_TOKENS` (4096) is the stopgap for a reserve of zero —
an unknown model, or `compaction.reserve_tokens = 0` with a window nobody
resolved — and it is a number the failure text can name. It is not a default any
front end may ship: a 4096-token summary of a long transcript is an excerpt
pretending to be a checkpoint.

**Parity requirement.** Every front end applies the derivation where it builds
the compaction backend, not inside a command handler (a per-command budget would
be a second owner of a session-scoped fact, and it would drift from the model the
session is running). Today that is `crates/lca-cli/src/headless.rs` and the
interface's `crates/lca-cli/src/tui/cells.rs` (`apply_summarization_budget`),
which the interface calls at assembly, on `/reload`, and every time a new model
lands (`/model`, a cross-provider switch, a login that resolves the first
model). Gh #594 was the interface never deriving one at all: a 1M-window session
with 660k tokens of transcript compacted on 4096 tokens and degraded to the
excerpt summary while the headless path used the derived budget.
`tests/regressions/594-tui-and-headless-derive-the-summarization-budget.rs`
walks every assembly site for the call, so a new front end that forgets reddens
that row instead of shipping the stopgap.

When a summary *is* cut at that budget, the strategy's failure line says so: the
provider's own message names the number, and the compaction layer adds that the
`max_tokens` is this session's derived budget rather than a provider limit (so
the same summary on a larger-window model would not be cut). The session keeps
working either way — the strategy degrades to its mechanical excerpt summary and
never fails a turn.

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
once, instead of ending it dead. The `overflow` compaction runs and
the retry starts fresh on the compacted log; a still-capped retry
surfaces, and a failed recovery compacts nothing and retries
nothing.

`TurnEnded` fires **once**, when the turn actually finishes (gh
#277). It used to fire before the recovery was attempted, so a
recovered turn carried on streaming after its own `TurnEnded` and an
unrecovered one emitted the event twice. The recovery is marked
instead by `CompactionStarted` / `CompactionEnded { reason:
"overflow" }`, which is what a consumer should read as "the turn hit
its cap and recovered"; `TurnEnded` remains the single end-of-turn
signal. A turn that recovered carries no `Error` event at all, since
nothing failed from the caller's point of view.

Divergence from pi, recorded: pi emits its `agent_end` and then tells
subscribers whether a retry will follow (`willRetry` on the event), so
a listener sees one end with a flag rather than a suppressed event.
LCA has no such flag on `TurnEnded`, so the equivalent contract is
enforced by the ordering instead - the recovery's compaction events
precede the single `TurnEnded`. Nothing is lost as long as consumers
read `CompactionStarted`/`CompactionEnded` for recovery and
`TurnEnded` for the end; a consumer that needs pi's single-event form
would need the flag added.

Divergence, documented: the aborted attempt's partial text is not
persisted (LCA never persists failed attempts) — the compaction
record marks the boundary. Recovery respects `compaction.enabled`:
opted out means no strategy to compact with, so the error surfaces.

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

## What the interface shows, live and on replay (gh #496, #393)

Two views of the same compaction, and they say different things on purpose. Both
render as the same **collapsed card** (gh #393): a summary is prose the reader has
already watched happen, so printing it in full every time filled the transcript
with walls of text nobody re-reads.

### The card (collapsed)

```
 [compaction]
 summarised via llm · 00000002-430c..00000009-430c (ctrl+o to expand)
```

The label is its own row and the key hint rides inline, which is pi's shape
(`compaction-summary-message.ts`): `[compaction]` over
`Compacted from 48,213 tokens (ctrl+o to expand)`.

**What differs from pi, and why.** pi prints a token count because its message
carries `tokensBefore`. LCA's `compaction` record has no pre-compaction token count
— its `usage` field is the *summarization call's* own cost — and the display view
suppresses the records the compaction replaced, so a count is not derivable at
render time either. The card therefore names the **record range**, which a reader can
grep in the session log, instead of a figure it would have to invent.

The card names its strategy and the record range it replaced, so **collapsed is
not gone**: a reader who never expands still knows their context was rewritten and
roughly what was cut. Hiding that would be a worse bug than the wall.

`ctrl+o` (the existing `app.tools.expand` action - no second keybinding) expands
this card to the full summary, and collapses it again. Expansion is **per card**:
opening one does not open the others, it survives a streaming delta on the same
transcript, and a *new* compaction always arrives collapsed.

A branch summary (`Record::BranchSummary`) gets the same card, labelled
`branch summary` - pi's wording. Before gh #393 it rendered nowhere at all - the
replay matched a catch-all and dropped it - so a resumed fork silently lost the text
explaining it.

The stored record is untouched: the summary text, strategy, range and file lists
all stay in the session log. Only the view is collapsed.

**Live.** When a compaction finishes while the user is in the session, the
transcript gains the card at the point in the turn's entries where it happened:

```
 [compaction]
 summarised (threshold) (ctrl+o to expand)

 [compaction failed]
 compaction failed (manual) - the context is unchanged
```

It names the reason (`manual` / `threshold` / `overflow`, the same words the
separator uses while it runs) and whether the summary record was written. A
failure is **not** rendered as a neutral marker: `success: false` means no record
was written and the context is exactly as it was, so a neutral line would tell the
user the opposite of the truth about their session.

The live card carries no summary text. `TurnEvent::CompactionEnded` is
`{ reason, success }`; the summary lives in the `Compaction` *record*, and
`lca-ui` has no `lca-session` dependency to read it. Adding one to fetch text the
user sees again on replay is a seam bought for cosmetics, so the live view states
the event and the replay states the content.

Consequently a live card offers **no expand key**: there is nothing to reveal, and
advertising a key that does nothing would be a trap. Live and replay therefore
agree on *shape* (one band card, one label, one collapsed line) and differ on
*content*, which is the split #496 recorded.

`CompactionStarted` deliberately adds no row. The separator already reads
`Compacting (reason)` while it runs (#388 gap 2), and a second live indicator in
the scrollback would be two views of one fact - with the transcript's being a
spinner in a place that is supposed to be a record.

**Replay.** `Transcript::replay_records` places the card at the record's own
position, holding the record's summary behind the same expand key.

So a user who leaves and resumes gets the content (one key away, in the right
chronological slot), and a user who stays gets the fact when it happens. One compaction produces one
row in each view however many end events arrive (a retried attempt does not
duplicate the marker), and a `CompactionStarted` clears that dedupe so a genuine
second compaction in the same session gets its own row.
