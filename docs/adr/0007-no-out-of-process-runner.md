# ADR-0007: No out-of-process extension runner

Status: accepted for 1.0.

Date: 2026-09-20.

## Context

The WASM sandbox cannot give an extension everything. Some capabilities are outside what the host is willing to expose through a typed import. The question is whether LCA should ship a second binary that runs a component with full operating system privileges, talking to the main process over a pipe.

Listing the actual gaps makes the answer clearer than arguing from principle. The sandbox today cannot give raw socket access for protocols other than HTTPS, filesystem watching, platform keychain access, long-lived background processes, or native graphical windows.

Three of those five are better solved as narrow host capabilities. Keychain access belongs behind the `credentials` capability anyway, because the host should prefer the platform keychain over a file. Filesystem watching is a small capability with an obvious shape. A narrow outbound TCP capability with a host-enforced allow list covers most socket cases without opening a general one.

## Decision

No out-of-process runner in 1.0. The native-linked path stays the only unsandboxed tier, and it is reserved for first-party code that ships in the binary.

Gaps get filled by adding narrow capabilities as evidence arrives. Phase 3 collects the cases. Each case becomes either a named capability or a written argument for the runner. The argument has to be written down, because the failure mode here is adding a tier because one extension author asked, not because the design needed it.

For the genuinely unbounded case, point at an external tool protocol. An extension holding the `process` and `net` capabilities can speak a protocol such as MCP to a server the user already installed and trusts. The privilege lives in that server, which the user manages through their own package manager, rather than in a new tier of this agent. An MCP bridge is itself an extension implementing the `tool` world, which is a good test of whether the ABI is expressive enough.

## Alternatives considered

Ship the runner as a second binary. It breaks the single file property that the project exists to protect. It needs process lifecycle management, crash recovery, and the whole ABI serialized over inter-process communication, which is most of the work of the WASM path with none of the isolation.

The stronger objection is not engineering cost. A user reading an install prompt can hold two trust tiers in their head: this runs sandboxed, or this runs with full access. Three tiers is noise, and noise in a consent flow means people stop reading. The capability model's value comes from people reading it.

Allow any extension to request an escape capability that disables the sandbox. It is the runner without the second binary, and it has the same problem: an escape hatch that any extension can request is not an escape hatch, it is an opt-out.

## Consequences

Some extensions cannot be written for LCA. That is the intended outcome of a capability model, and saying so plainly is better than pretending the sandbox is universal.

The capability set grows over time and each addition needs justification. Growth by evidence is slower than growth by request, which is the point.

The risk register entry about a restrictive capability model names the external tool protocol as its first fallback, before a broad capability.

Phase 3 carries a deliverable beyond its exit test: a written list of the capability gaps that real extensions hit. That list is the input to this decision being revisited.

## Revisit conditions

A case from Phase 3 that a narrow capability cannot cover and an external tool protocol cannot serve. One such case is not enough. Three would mean the model has a structural gap rather than a missing feature.
