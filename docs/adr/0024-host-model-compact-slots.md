# ADR-0024: The `/model` and `/compact` slots are host-side; no new effect, world, or WIT export

Status: accepted.

Date: 2026-09-24.

## Context

The requirements document's interface section names six built-in slash commands, and `/model` and `/compact` were among them. The Phase 4 log recorded `/compact` as unwired precisely because the trigger needs an interface: a manual compact must reach the session's record pipeline, and a model switch must reach the code that builds every request - and *which* interface those live behind is a design decision the log deferred to an ADR. The final audit then confirmed both slots were still unclaimed in the binary, and that no numbered requirement exists for either (their neighbors are tagged FR-PROV-2 for the picker's listing half and FR-SESS-5 for the compaction mechanism).

## Decision

Both slots are host-side, routed through surfaces that already exist, so neither `CommandEffect` nor the frozen `lca:ext` world changes.

`/compact` is intercepted by the CLI's command invoker - the same route `/login` and friends use - and drives `lca_core::compact_now`: the host picks the candidate range (every compactable record, no threshold), the compaction world's own strategy produces the summary (FR-SESS-5's rule holds - there is no built-in summarizing path), and the host appends the durable record. The work runs through `drive_blocking`, which gives a future a thread with its own runtime, because the interface thread is already driven by `main`'s runtime where any nested `block_on` panics.

`/model` with no argument renders the model picker's text from the provider world's existing `list_models` (FR-PROV-2 at the interface); with an argument it validates against that same listing and rewrites the session's model everywhere it is read: the runner's per-turn `AgentConfig`, the status-line label cell, and the completion backend's model via `ProviderBackend::set_model`. The override lives in process memory - "/model overrides it for a session" means this session, this run; configuration files are untouched.

## Alternatives considered

A new `CommandEffect` variant for each (`OpenPicker`, `RunCompaction`). Rejected: `CommandEffect` is the protocol-layer type every extension command returns, so a variant is an interface change for behavior only the host performs - exactly the kind of coupling ADR-0019's dispatch split exists to avoid.

A `command`-world export on the compaction extension (the requirements say a compaction extension "typically also implements `command`, for a manual trigger"). Rejected as the *built-in's* route: the slot must answer even when no such extension is installed or enabled ("no compaction extension is enabled" is the honest reply), and adding an export would move the frozen ABI's minor. Extensions remain free to register their own command - worlds compose; the host route does not preclude it.

A modal model picker. Rejected for 0.1: `CommandEffect` has no modal-open for built-ins, and the notice-area listing matches the `/login` picker's idiom exactly, which is already how the generic identity commands present a choice.

## Consequences

`BUILTIN_SLOTS` (asserted by test) claims the five host-owned slots; `/stats` still arrives from the native hooks extension. The manual compaction path is proven at the core with the automatic window disabled, so every invocation in that test is the manual one's doing. The model switch is proven from the picker text down to the backend's next request. WIT, `abi-versioning.md`, and the ABI freeze at 1.0 are untouched.

## Revisit conditions

A want for arrow-key selection (a modal picker), or for `/compact` to stop blocking the interface thread for the length of one model call - the notice-cell pattern the update check already uses is the upgrade path. Either one changes the effect surface, and that is when this decision gets its successor.
