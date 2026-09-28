# 0038. Steering: prompts submitted while a turn runs

Status: accepted (2026-09-28).

## Context

While a turn runs, the input used to accept nothing: a submit during a
running turn was dropped, and the only interaction was cancel. Every
agent that is pleasant to work with lets the user talk to the model while
it works — correct a course, add context, or queue the next task. pi's
queue model is the design source (steer versus follow-up queues with a
compaction-queue sibling). The owner named steering as an essential
feature of the TUI renovation.

## Decision

**Two submit modes** (`SubmitMode`): `Steer` and `FollowUp`. (An
ordinary submit with no turn running is the no-queue path, not a mode.)
While a turn runs, `Steer` appends the message to the turn's input at the
**next model-call boundary** — after the current tool or model step
completes. `FollowUp` queues the message and auto-submits it when the
turn ends. Queued messages keep their submission order regardless of
mode, and each remembers its mode.

**A streaming response is never mutated.** Steering queues messages; it
does not rewrite an in-flight stream. The ceiling is that a steer takes
effect at the next boundary, not mid-step, and that ceiling is the
documented behavior, not a limitation to hide.

**Queue lifecycle.** During compaction the queue holds and flushes after
(the compaction-queue sibling). Aborting a turn returns every queued
message to the editor, in order; an edit-all-queued action does the
same while the turn runs. The session log records each queued message
and the boundary where it was injected, in the same record vocabulary as
ordinary user messages with a queue marker (`docs/session-log-format.md`
is extended in the change that implements this).

**The interface makes the queue visible.** The input stays editable
during a turn; a pending band lists queued messages, marking steer
against follow-up; the status area shows the queue count. While a turn
runs, Enter steers and Alt+Enter queues a follow-up; an aborted turn
returns the queue to the editor in order.

**Extension visibility (decided).** The submit-mode marker travels on
the session record (`Record::User.queue`) and is copied into the
`message.extras` map of the resolved list, so a `context-transform`
extension reads it as `extras["queue"]` with no ABI change (the `extras`
map is reserved for exactly this, `wit/types.wit`). A dedicated
`pre-prompt` hook was the alternative; it was rejected because it would
add a hook point and a signature for data that already has a home.

## Consequences

- ADR-0004's stream shape is untouched: steering is message queueing at
  the turn-loop level (`lca-core`), not stream manipulation.
- Injection latency is bounded by the duration of the current step; a
  long tool run delays a steer exactly as long as it delays the next
  model call. This is inherent to boundary injection and accepted.
- The queue is ordinary input to the model at the next call, so prompt
  caching is preserved: steered messages extend the message list the way
  any user message does (ADR-0017's stable prefix keeps its value).
- A queued message's mode is visible to extensions through
  `message.extras["queue"]`, so a transform can treat a steer differently
  from a follow-up without a new hook or ABI change.

## Annotation — 2026-09-28 (cycle 3): the decision text above was rewritten
in place, and that was the wrong house procedure

**The record, honestly.** The decision originally named **three** submit
modes — `Steer`, `FollowUp`, and `New` — with `New` standing for "the
no-turn case". During cycle 3's pre-review audit the `New` variant was
correctly recognised as not a mode at all (the no-turn path is simply the
absence of a queue) and deleted from `SubmitMode`. The decision text above
was then edited from "Three submit modes" to "Two submit modes", and the
`New` sentence was deleted, in place — the audit commit `e93a049`.

That edit changed **decision text**, which the house rule (this ADR
family's annotation rule) forbids: an ADR is annotated or superseded,
never silently rewritten. The resulting text is correct — `SubmitMode`
really does have two variants and `New` never was a mode — so the fix is
this annotation, not a revert: the original three-mode wording stands on
the record here, the current decision text is the corrected one, and this
annotation is the trail between them. Future edits to a decision follow
the rule the cycle-3 audit broke: append an annotation, never rewrite the
paragraph.
