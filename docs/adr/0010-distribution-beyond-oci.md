# ADR-0010: Distribution beyond OCI registries

Status: accepted.

Date: 2026-09-20.

## Context

OCI is the primary extension distribution channel. The question is whether it should be the only one, given that git, plain HTTPS hosting, and operating system package managers are all real ways software gets distributed today.

Each alternative fails for a different reason, and the reasons matter because they decide what, if anything, replaces OCI as a channel rather than sitting alongside it.

Git works well for interpreted code because the runtime and the source are the same artifact. It does not work the same way here, because a WebAssembly component is a build output, not source. Distributing a repository means distributing something the host cannot run without a `cargo component build` step, which reintroduces the toolchain dependency this design exists to remove from the consuming side.

Operating system packages are OS-specific by construction, which rules out macOS and Windows from a channel meant to serve all three. They also bypass the manifest consent screen: a postinstall script that drops a `.wasm` file onto disk has no way to show what it is asking for, which conflicts directly with the model where the manifest is the consent surface.

Plain HTTPS hosting has neither problem. A component and a manifest, zipped, served from any HTTPS URL, is functionally the same payload an OCI artifact carries, just fetched a different way.

## Decision

Add exactly one more source kind for 1.0: a plain HTTPS-fetched archive. Treat it as behaviorally identical to an OCI moving tag rather than inventing separate update machinery for it.

The archive is a zip containing `extension.toml` and the component file, nothing else. No custom container format. The resolver fetches it, verifies the manifest, and hands the result to the same consent and lockfile machinery an OCI-sourced extension goes through.

Update semantics come from treating the URL itself as a moving pointer. `lca ext update` re-fetches the URL, compares the resulting digest against the lockfile, and prompts if it changed and the capability set widened, the same rule an OCI ABI-line tag resolves under. An author publishing a "latest" asset on a release page gets update behavior with no protocol beyond what already exists.

Local path installation already exists as its own source kind, specified in FR-DIST-5, and is unaffected by this record.

Git remains a documented development workflow, not a distribution channel: an author builds locally and installs from the resulting path, or, for someone who wants to verify a published binary matches its source by building it themselves, clones and builds with their own toolchain. Neither needs a distribution mechanism from the host beyond the local path source that already exists.

Operating system packages remain out of scope for 1.0. A future managed-fleet feature, where an organization pre-stages approved extensions for a team, is a distinct problem involving organizational trust rather than public distribution, and does not change how an individual user installs something.

## Alternatives considered

An open-ended set of source kinds, resolved by URL scheme sniffing. It would accept git URLs, arbitrary HTTP endpoints, and anything else that looks pluggable. Every new kind is a new resolver, a new update story, and a new thing the consent screen has to describe accurately. Rejected in favor of a small, enumerable set, matching the discipline already applied to the capability catalog.

A custom archive format with its own extension, distinct from a plain zip. It would look more like a first-party product and it would need its own tooling to produce and inspect. A zip needs none, and every platform already has tools that open one.

Requiring OCI for every install, with local path as the only alternative. Smallest possible surface. It also means an author who does not want to run a registry, or a user on a network that blocks registry traffic but allows plain HTTPS, has no path at all. The additional source kind costs one resolver function and removes a real barrier.

## Consequences

`lca-registry` gains a second resolver alongside the OCI one. Both produce the same pair, verified bytes and a parsed manifest, so nothing downstream of resolution needs to know which kind supplied them.

The lockfile records a source reference regardless of kind. For an HTTPS archive that reference is the URL rather than an OCI reference string; the digest verification and update-prompt logic are identical either way.

The extension authoring guide gains a short section on publishing to a plain HTTPS host as an alternative to a registry, with the same two-reference convention OCI uses: a versioned URL for a fixed install, and a stable "latest" URL for the one the update path re-resolves.

## Revisit conditions

Evidence that a third source kind is needed for a real, motivated case, following the same bar the capability catalog uses for adding a capability. Evidence that the archive format needs more structure than a manifest plus a component, which would argue for reconsidering the zip choice rather than adding a fourth kind on top of it.
