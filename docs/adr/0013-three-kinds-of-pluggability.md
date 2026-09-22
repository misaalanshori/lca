# ADR-0013: Three kinds of pluggability

Status: accepted.

Date: 2026-09-20.

## Context

Several design questions kept resolving to the same underlying confusion: whether something being swappable meant it belonged in the capability-gated WASM extension system. Read, write, and shell tools are swappable across build targets but are not third-party extensions. Skills-handling and the default compaction strategy are meant to be replaceable but are also meant to ship enabled by default. Without a named distinction, every one of these cases had to be argued from scratch.

The confusion has a specific shape. "Pluggable" was being treated as one property, when it is really answering two different questions: does this vary, and if it does, does the thing supplying the variation need to be untrusted. Those two questions have three honest answers, not two.

## Decision

Name three categories, and place every major piece of the system into one of them.

Fixed core is code that does not vary and carries no extension point. The agent loop's overall shape, the session log's record framing, the permission enforcement path.

A build-time backend is code with more than one implementation, selected when the binary or web bundle is produced, with no runtime installation, no manifest, and no consent screen, because the party supplying the implementation is the project itself, not a third party. Read, write, and shell tools are the clearest case: they are core in the sense that no capability gates them, since they are what defines the model's access to the workspace rather than something requesting it, but their implementation differs by build target behind a Rust trait, a native backend for desktop and a host-delegated backend for the web target that defers to whatever the embedding JavaScript application supplies, per FR-WEB-3.

A runtime extension is code installed by the user, potentially from a party the project does not control, gated by the capability model, shown a consent screen naming exactly what it can reach, and running either sandboxed as a WASM component or native-linked and labeled unsandboxed. Providers, skills-handling, compaction, context transforms, and anything a user installs from a registry all live here, regardless of whether a particular one ships enabled by default. Shipping enabled by default is a packaging decision; it does not move something out of this category.

The test for which category something belongs in is not "could this be swapped" but "who supplies the thing it is swapped for, and does that party need to be treated as untrusted." Fixed core has one supplier: the project itself, and there is only one implementation. A build-time backend has more than one implementation, all supplied by the project, chosen once at build time. A runtime extension's implementation is supplied by whoever the user chooses to install, which is why it needs the consent and capability machinery the other two categories do not.

## Alternatives considered

Treat everything that varies as an extension, including read, write, and shell tools, gating even the build-target backend choice through the capability system. This was the source of the original confusion, and reasoning it through is what motivated this record: it would mean the workspace tools need a manifest and a grant to access the workspace, which is circular, and it would mean an ordinary build configuration change requires the same ceremony as installing an untrusted third-party component. Rejected.

Treat nothing as pluggable unless it is a runtime extension, hardcoding build-target differences with conditional compilation scattered through the tool implementations rather than a named backend trait. This avoids inventing a category and it loses the property that made the backend trait worth designing: a single, tested seam where the native and web implementations diverge, rather than an unbounded number of small conditionals.

Two categories instead of three, merging fixed core and build-time backends since neither goes through the capability system. This loses the distinction that matters for a reader asking "can I swap this," which fixed core answers no to and a build-time backend answers yes to, just not at runtime and not by a third party.

## Consequences

Every new feature proposal should be placed into one of the three categories explicitly, as part of deciding whether it needs a WIT world, a Rust trait, or nothing at all. This record is the reference for that placement.

| Feature | Category |
|---|---|
| Agent loop, session log framing, permission enforcement | Fixed core |
| Read, write, shell tools | Build-time backend, native and web-delegated |
| Skills-handling | Runtime extension, native-linked by default |
| Compaction | Runtime extension, native-linked by default |
| Context transforms | Runtime extension; skills-handling is one and ships bundled, no others by default |
| OpenAI-compatible provider | Runtime extension, native-linked by default |
| Antigravity, Codex providers | Runtime extension, WASM only by default |
| MCP-bridging, external tool wrapping | Runtime extension |

A feature that seems to need capability-style consent but has only one possible supplier, the project itself, is misclassified; it belongs in the build-time backend category instead, and adding a manifest for it would be adding ceremony with no corresponding trust boundary to justify it.

## Revisit conditions

A case that does not fit any of the three categories cleanly, which would mean the taxonomy is incomplete rather than that the case is unusual.
