# ADR-0001: WebAssembly runtime selection

Status: proposed. Phase 0 confirms or overturns it.

Date: 2026-09-20.

## Context

LCA embeds a WebAssembly runtime to load extensions. The runtime ships inside the binary, so its size counts against NFR-1 and NFR-2. The runtime has to support the Component Model, because the extension ABI is defined in WIT rather than as a flat byte-blob interface. It has to run on six native targets. It also has to work on targets where a host cannot create executable memory pages, because a just-in-time compiler cannot run there.

The size constraint and the Component Model constraint pull against each other. A runtime with a compiler backend is large. A runtime small enough to ignore is usually an interpreter with thinner Component Model support.

## Decision

Use Wasmtime. Enable the Cranelift backend by default on native targets. Enable Pulley, the portable interpreter that ships inside Wasmtime, for targets with no Cranelift backend and for any environment that forbids executable memory. Put both behind Cargo features so a minimal build can drop Cranelift entirely.

The decision is provisional. Phase 0 builds a minimal host with each candidate and measures binary size, cold start, instantiation time, and per-call overhead. If Wasmtime with Cranelift pushes the default build far past the size target, the default build switches to the interpreter and the compiler backend moves to a separate full build.

## Alternatives considered

Wasmi is a WebAssembly interpreter written in Rust with no unsafe code. It is smaller than Wasmtime and it needs no executable memory anywhere, which removes a whole class of platform problem. Its Component Model support is less developed than Wasmtime's, and the extension ABI depends on the Component Model rather than on core modules. Phase 0 measures it as the fallback for the default build.

WAMR is a small runtime written in C, built for embedding. Its interpreter and ahead-of-time runtime are both small enough to ignore against the size budget. It costs a C foreign function interface boundary in a project that otherwise has none, and its Component Model tooling is further from the Bytecode Alliance reference path than Wasmtime's.

A runtime written in Zig was considered and rejected on language grounds. The host is Rust. A Rust host embedding a Rust runtime has no boundary at all, and Wasmtime is a crate.

Extism is a plugin framework that solves this problem at a higher level. It works and it would be faster to adopt. It uses a blob-passing interface rather than typed WIT interfaces, which gives up the typed ABI that the extension design depends on.

Writing a runtime is not an alternative. It is a multi-year project with a security surface that this team cannot maintain.

## Consequences

The Wasmtime version is pinned. Upgrades happen in their own pull request with size and startup measurements attached, because a runtime upgrade can move both numbers.

The project tracks Wasmtime security advisories. A sandbox escape in the runtime is a host compromise, so advisories get the same treatment as a defect in LCA's own capability enforcement.

The build has two backend paths to test. The conformance extension runs under both, and the pipeline checks that results match. A difference between backends is a defect.

Precompilation is per digest: installing or updating an extension compiles its component to a cached ahead-of-time artifact keyed by that digest, stored under the extension tree, and ordinary loads execute the cached artifact; a digest change discards it. Phase 0 measures whether ahead-of-time-by-default beats just-in-time for the instantiation budget in NFR-4.

Pulley makes the web target simpler than it would otherwise be, but it is not the mechanism used there. The web build transpiles components to ES modules and runs them on the host engine as peers, which needs no embedded runtime at all. Pulley covers native targets without a Cranelift backend.

## Revisit conditions

Phase 0 measurements showing the Cranelift build more than double the size target. A Component Model release in wasmi that covers the worlds in `lca:ext`. A Wasmtime advisory pattern that makes the pinning burden unreasonable. Any of these reopens the choice.
