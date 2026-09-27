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

**Three submit modes** (`SubmitMode`): `Steer`, `FollowUp`, and `New`.
While a turn runs, `Steer` appends the message to the turn's input at the
**next model-call boundary** — after the current tool or model step
completes. `FollowUp` queues the message and auto-submits it when the
turn ends. Queued messages keep their submission order regardless of
mode, and each remembers its mode. `New` is the no-turn case.

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
against follow-up; the status area shows the queue count.

## Consequences

- ADR-0004's stream shape is untouched: steering is message queueing at
  the turn-loop level (`lca-core`), not stream manipulation.
- Injection latency is bounded by the duration of the current step; a
  long tool run delays a steer exactly as long as it delays the next
  model call. This is inherent to boundary injection and accepted.
- The queue is ordinary input to the model at the next call, so prompt
  caching is preserved: steered messages extend the message list the way
  any user message does (ADR-0017's stable prefix keeps its value).
