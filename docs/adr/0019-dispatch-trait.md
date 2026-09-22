# ADR-0019: One dispatch trait for both delivery modes

Status: accepted.

Date: 2026-09-23.

## Context

The SRDD says the core holds a list of extension handles, that a handle
is either native (Rust trait object) or WASM (component instance), that
call sites never branch on the mode (FR-EXT-6), and that `lca-ext-native`
"exposes the same handle type as `lca-ext-host`". It does not say where
that type lives. Candidates: `lca-core` (wrong direction: only
`lca-sdk`/`lca-cli` may depend on core, and core must not depend on the
hosts), `lca-ext-host` (would drag Wasmtime into every native
registration), or `lca-ext-abi`.

## Decision

The trait lives in `lca-ext-abi`, the contract crate: `dispatch::ExtensionDispatch`.
It is a plain `Send + Sync` trait covering the worlds written so far
(tool, command, hooks), returning `lca-protocol` types (`ToolSpec`,
`CommandSpec`, `CommandEffect`, `HookAction`, `DispatchError`).
`WasmExtension` implements it by mapping its generated bindings;
`lca-ext-native` registers objects that already implement it; `lca-core`
holds `Vec<Arc<dyn ExtensionDispatch>>` and never learns which side any
handle came from.

Command names auto-namespace as `<extension>.<command>` (the ADR-0012
mechanism, which the SRDD already states for provider identity
commands), so third-party commands cannot collide with the six built-in
slash names. First-party *native* extensions may claim a reserved
built-in slot directly — they are unsandboxed first-party code, a review
rule like the rest of the native path (ADR-0013) — which is how a
behavior can move out of the core implementation while the user-facing
built-in name stays put.

## Alternatives considered

The trait in `lca-core`: rejected, inverts the dependency direction the
crate decomposition fixes (ADR-0002).

The trait in `lca-ext-host`: rejected; every native registration would
link Wasmtime even when no component ever loads, and `lca-ext-native`
would depend on the sandbox it exists to parallel.

An enum `Handle { Native(..), Wasm(..) }`: rejected outright — FR-EXT-6
forbids call sites branching on the mode; an enum invites exactly that.

## Consequences

`lca-ext-abi` gains a small Rust surface beyond the WIT (it already
carries `ABI_VERSION` and, behind a feature, host bindings), and gains a
dependency on `lca-protocol`. Adding a world to the dispatch trait is
an additive Rust change; the WIT world it mirrors follows the
`docs/abi-versioning.md` table as before.

Name collisions (FR-EXT-11) are enforced once, in core's registry:
bare tool names against the reserved built-in set and against earlier
registrations, and command leaf names within their namespace.

## Revisit conditions

A third delivery mode (the web target, ADR-0018) that cannot implement
this trait cheaply, which would argue for splitting the trait per world
or moving it behind per-world adapter crates.
