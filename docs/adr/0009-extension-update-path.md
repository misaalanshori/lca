# ADR-0009: Extension update path across ABI versions

Status: accepted.

Date: 2026-09-20.

## Context

The host supports the current ABI minor version and the one before it. An author has one minor cycle to publish a rebuilt component. When a user upgrades the agent, some installed extensions may target an ABI version the new host no longer supports.

The agent loads extensions by digest, which is what makes an install reproducible. A digest also pins an extension to one exact build, so something has to move it forward.

## Decision

Record a digest in a lockfile, resolve a moving tag at explicit update time, and always load by digest.

The registry tag scheme does most of the work. Each release is published under an immutable version tag, and also under a moving tag for its ABI line, such as `abi-0.1`. The resolver has one job: resolve the moving tag to a digest. The digest goes in the lockfile and every later load goes by digest, so a mutable tag never becomes a supply chain hole.

Updates are explicit. `lca ext update <name>` and `lca ext update --all` resolve and apply. An update that widens the capability set prompts before it applies, because a capability nobody approved defeats the consent flow.

The failure path is specified, because this is where a user gets stuck. When the host loads an extension whose ABI version it no longer supports, it refuses the load, disables that extension for the session, reports whether a compatible version exists in the registry, and names the command that fixes it. The session continues without the extension. A stale extension should not cost a user their agent.

## Alternatives considered

Manual reinstall with no update command. Smallest implementation. It gives the user no way to find out that a compatible version exists, which turns a one-command fix into a support question.

Automatic update when the host upgrades. It removes the stuck state entirely. It also applies capability changes nobody approved, and any update that prompts is not automatic. The consent model and automatic updates are incompatible, and the consent model wins.

Resolve the moving tag at every load instead of at update time. It keeps extensions current with no user action. It also means a registry can change what runs on a user's machine between two launches, which is the supply chain property the digest exists to prevent. Rejected.

Load by tag and verify a signature instead of a digest. Signatures solve a different problem well and need a key distribution story the project does not have in 1.0. The digest is enough for reproducibility, which is the goal here.

## Consequences

The lockfile becomes part of the extension state and needs a defined format, location, and recovery behavior when it is missing or corrupt.

Extension authors have to publish two tags per release. The authoring guide covers it and the publishing example shows it.

The one-minor deprecation window needs a test. The pipeline builds the conformance extension against the previous ABI minor version and asserts the current host loads it.

Three requirements follow. The agent SHALL record the resolved digest and the source reference of each installed extension. WHEN the user runs the update command, the agent SHALL resolve the ABI line tag to a digest and SHALL prompt before it applies any capability the current grant does not cover. IF an installed extension targets an unsupported ABI version at load time, THEN the agent SHALL disable it, report whether a compatible version exists, and continue the session.

## Revisit conditions

A signing story landing in the wider WASM registry tooling, which would make signature verification cheap enough to add alongside the digest. Evidence that users never run the update command, which would argue for a prompt at launch rather than automatic updates.
