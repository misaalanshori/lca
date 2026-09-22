# ADR-0018: Web-embedded extension hosting through sibling instantiation, not a nested runtime

Status: accepted.

Date: 2026-09-20.

## Context

The `wasm32-wasip2` build of the agent, per FR-WEB-1, is meant to run inside a JavaScript host: a browser tab or a Node process. That build still needs to host extensions the same way the native build does. The question is how, and it was worked through at length before this record existed to capture it, which is the gap this record closes.

The naive answer is to embed a WASM runtime inside the agent's own WASM build, the same way the native build embeds Wasmtime, so the agent-as-WASM-module hosts extension-WASM-modules from within itself. This runs into a hard wall: a JIT-based runtime, Wasmtime's default Cranelift backend included, needs to mark memory pages executable to run the native code it generates, and a WASM guest has no way to request that from inside its own sandbox. A JIT cannot run nested inside a WASM guest, full stop; this is not a Wasmtime limitation, it is what WASM's sandboxing model is for.

Wasmtime's own answer to environments without a JIT backend is Pulley, a portable bytecode interpreter, already the fallback chosen in ADR-0001 for native targets with no Cranelift backend. Pulley needs no executable memory and could run nested inside the agent's own WASM sandbox, interpreting an extension's bytecode the same way any other computation the agent's WASM build performs. This works, but it means the browser's own WASM engine, which is a real, fully JIT-compiled engine, is left idle for the extension's execution while a slower interpreter inside another WASM sandbox does the work instead, a genuinely worse position than the native build is ever in.

## Decision

Do not nest a WASM runtime inside the web build at all. Let the JavaScript host instantiate the agent module and each extension module as siblings, both running directly on the browser or Node's own WASM engine, and use `jco`, the Bytecode Alliance's JavaScript Component tooling, to make the typed WIT contract between them speak plain JavaScript at that boundary rather than hand-rolling marshaling code.

Concretely: `jco transpile` turns both the agent's Component and each extension's Component into an ES module wrapping a core WASM module, with all Canonical ABI marshaling, records, strings, resources, already generated. The JS orchestrator, per FR-WEB-2, imports both modules and wires the agent's expected imports to each extension's exports directly in JavaScript. Neither module is nested inside the other from the WASM engine's perspective; both are first-class guests of the same JIT-compiled host engine, and the extension pays no bytecode-interpretation tax at all in the browser, where the naive nested-runtime answer would have imposed exactly that.

Pulley is not eliminated by this decision; it remains the correct answer for a genuinely bare embedding target with no JavaScript layer to mediate, which is a narrower and more exotic case than the browser or Node embedding this project actually ships. Where that narrower case arises, this record's reasoning does not apply and the native-target fallback in ADR-0001 does.

## Alternatives considered

Nested Pulley, as described above. Technically correct and meaningfully slower than necessary, since it declines to use the fast engine already sitting right there in the host. Rejected once the sibling-instantiation option was identified, on pure performance grounds; there was no capability nested Pulley offered that sibling instantiation does not.

A hand-written JS marshaling layer instead of `jco`, manually serializing values across the boundary between the agent module's imports and an extension module's exports. This is exactly the kind of mechanical, canonical-ABI-defined work `jco` already does correctly and is maintained by the same organization that defines the ABI it implements. Writing it by hand duplicates that work and is a second, independent place for a marshaling bug to live. Rejected.

Requiring the web target to accept a reduced extension surface, native-linked equivalents only, with no true third-party WASM extensions loadable in the browser at all. This would have sidestepped the whole problem by not solving it. Rejected because it would make the web target meaningfully less capable than the native build for no reason connected to the browser's actual limitations, which, once `jco` is in the picture, turn out not to be a real constraint here.

## Consequences

The web build's extension-loading code and the native build's extension-loading code are genuinely different implementations behind the same conceptual contract, not the same code path with a runtime switch. This is worth stating plainly rather than letting the "same ABI everywhere" framing imply more code sharing across the two builds than actually exists: the WIT contract is shared, the mechanism that satisfies it on each side is not.

`jco`'s own maturity for the browser target specifically, as opposed to Node, is the dependency this decision rests most heavily on; if a specific Component Model feature the agent or a reference extension needs turns out not to be supported yet in `jco`'s browser output, that surfaces as a Phase 7 finding, not as a defect in this record's reasoning.

Extension authors do not need to know or care which hosting mechanism is in play. A component built once against the WIT worlds runs unmodified under Wasmtime natively and under `jco` in the browser; the dual-mode native-linked-versus-WASM distinction from ADR-0013 is a separate axis entirely from native-versus-web hosting, and an extension author reasons about the former, never the latter.

## Revisit conditions

A genuinely bare embedding target surfacing with real demand, no JavaScript layer at all, would need this record's sibling-instantiation approach replaced with the nested-Pulley fallback for that specific target, without disturbing the browser and Node cases this record actually governs. Evidence that `jco`'s browser output cannot keep pace with Wasmtime's native performance closely enough to matter for the interactive use cases this project targets, which would be a reason to revisit the whole web-target scope rather than this record specifically.
