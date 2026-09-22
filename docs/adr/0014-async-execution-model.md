# ADR-0014: Async execution model

Status: accepted.

Date: 2026-09-20.

## Context

The agent has one process and needs to interleave several kinds of concurrent work on it: consuming a streaming provider response, running shell commands with streamed output, calling into WASM extensions that may themselves make network requests, and responding to cancellation immediately regardless of what else is in flight. The design needed to settle how WASM extension execution shares the process with everything else, and how cancellation reaches into a running extension call specifically.

## Decision

One Tokio runtime, multi-threaded, for the whole process. The provider stream is consumed by an async task pulling the WIT stream resource's next function repeatedly, as described in `docs/flows.md`. Shell and subprocess execution goes through `tokio::process`, with output streamed into the render pipeline as it arrives.

WASM extension calls share this runtime rather than each running on a dedicated blocking thread. Wasmtime's async host function support makes this possible: a host import such as a `net` request can genuinely await a real response without blocking an OS thread, and the guest's own execution yields back to the host at points Wasmtime controls, so several extension instances cooperate on the same runtime instead of each needing a thread of its own.

Two distinct Wasmtime mechanisms serve two distinct needs, and the design keeps them separate rather than overloading one for both jobs. Fuel is a resource budget: how much computation a single call is allowed before it is cut off, configured per extension from the manifest's `limits.fuel_per_call`, already specified in the manifest schema. Epoch interruption is a cancellation mechanism: a way to force-preempt a running instance from another thread on demand, independent of how much of its fuel budget remains. Cancellation, per FR-CORE-5, uses epoch interruption specifically: when the user cancels a turn, the host increments the epoch and every running instance traps at its next yield point, regardless of whether it was anywhere near its fuel limit. Using fuel exhaustion as the cancellation path would tie an unrelated concern, resource budgeting, to responsiveness, and would make cancellation latency depend on how generous an extension's budget happened to be rather than being immediate.

Tool call execution within a single turn is sequential for 1.0. A model response that requests several tool calls has them run one after another, not concurrently. This is a deliberate simplification, not a limitation the ABI forces: the provider stream already reports fully-formed tool calls in whatever order the model emitted them, and running them concurrently would require deciding an execution order for their results, interleaving their output in the terminal, and reasoning about two tool calls racing on the same file. None of that has a forcing case yet.

## Alternatives considered

A dedicated OS thread per active extension instance, with blocking calls inside it. Simpler to reason about in isolation and it does not scale the same way: thread count grows with extension count rather than staying bounded by the runtime's own worker pool, and cancellation becomes a matter of killing or signaling a thread rather than a cooperative yield, which is a cruder and less portable mechanism than epoch interruption.

Using fuel exhaustion as the sole cancellation mechanism, with no separate epoch interruption path. It would mean giving an extension call a very large fuel budget to leave room for legitimate work, in which case cancellation could take arbitrarily long to actually stop it, or a very small one to keep cancellation responsive, in which case legitimate long-running calls fail for an unrelated reason. Rejected because these are two different concerns wearing one mechanism.

Concurrent tool call execution by default, with an opt-out for tools that declare themselves unsafe to run in parallel. It is more work with no motivating case yet, and getting the interleaved terminal output right is a real design problem on its own that deserves to be solved when there is a concrete reason to, not speculatively.

## Consequences

Extension authors do not need to think about threads at all; the host's use of Wasmtime's async support is invisible from inside the component. What they do need to handle is that a call can be interrupted mid-execution through epoch interruption, so any state an extension mutates as a side effect of a call, most relevantly anything written through the `fs` capability, should be written in a way that a partial write is either harmless or detectable, since a cancellation can land between two writes the extension intended to be atomic.

Cancellation latency becomes a measurable, testable property rather than a qualitative one, since epoch interruption's granularity is bounded by how often Wasmtime checks the epoch during execution, which is itself a tunable the host configures.

Sequential tool execution is a decision the requirements section states explicitly, so that a future change to concurrent execution is a visible requirement change, not something that happens quietly as an optimization.

## Revisit conditions

A workload where sequential tool execution is the measured bottleneck, with data showing the tool calls in question are genuinely independent, would justify designing concurrent execution properly rather than defaulting away from it further out of caution alone.
