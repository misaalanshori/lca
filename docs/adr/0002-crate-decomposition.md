# ADR-0002: Crate decomposition

Status: accepted.

Date: 2026-09-20.

## Context

LCA is one repository. The question is how many crates it contains and where the boundaries fall.

Two forces push in opposite directions. A single crate compiles faster in a clean build, needs no dependency bookkeeping, and lets any code reach any other code. Many crates make the dependency direction explicit, let parts be published on their own, and stop a small change from rebuilding everything.

One requirement settles part of it. Extension authors need to depend on the ABI without pulling in the agent. That means the ABI has to be its own crate, published on its own schedule. Once one crate is separate for that reason, the argument for keeping the rest together weakens.

## Decision

Fifteen crates in one Cargo workspace. Dependencies flow in one direction. No crate depends on `lca-cli`, and only `lca-cli` depends on everything.

The layering, from the bottom up:

`lca-protocol` and `lca-config` sit at the bottom with no workspace dependencies. `lca-protocol` holds shared types and performs no input or output.

`lca-permissions`, `lca-session`, `lca-provider`, and `lca-ext-abi` sit above them. Each one owns a domain and depends on `lca-protocol`.

`lca-tools`, `lca-ext-host`, `lca-ext-native`, `lca-registry`, and `lca-tui` sit above those and hold the parts that touch the operating system, the network, and the terminal.

`lca-core` holds the agent loop and the dispatch table. `lca-sdk` wraps it for embedding. `lca-cli` is the binary.

`lca-testkit` is a dev dependency for every crate and depends on `lca-protocol` and `lca-provider`.

`lca-ext-abi` is published to a registry on its own. The others are internal unless a reason appears to publish them.

## Alternatives considered

One crate for everything, with modules instead of crates. It compiles faster from clean and it makes refactoring trivial. It also makes the dependency direction invisible, which is the thing most likely to rot. An extension author would have to depend on the whole agent to get the ABI types.

Three crates, splitting core, interface, and binary. This is the common shape for a terminal application and it would work. It puts the extension host, the registry client, and the session store in the same crate as the agent loop, which is where a minimal core stops being minimal without anyone noticing.

A crate per feature, with twenty-five or more crates. Boundaries get so fine that a normal change touches five manifests. The bookkeeping cost outgrows the benefit.

## Consequences

Incremental builds get faster and clean builds get slower. The pipeline caches the workspace, so the clean build cost lands mostly on new contributors.

Every new crate needs a manifest, a license header, and a place in the dependency graph. A pull request that adds a crate states which layer it belongs to.

Circular dependencies become a compile error instead of a design problem found later. This is the main benefit and it is worth the manifest churn on its own.

The extension ABI can version independently of the agent. This is required by the ABI policy and it is not possible without the split.

Crate boundaries make the minimalism goal enforceable. A reviewer can ask which crate a new feature belongs in. A feature that fits nowhere is usually an extension.

## Revisit conditions

Clean build times that stop new contributors from working. A pattern of changes that routinely touch more than five crates, which would mean the boundaries are in the wrong place.
