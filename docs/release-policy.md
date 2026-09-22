# Release and versioning policy

Version 0.1, 2026-09-20.

This covers how LCA is versioned, branched, built, signed, and published. It is separate from the ABI versioning policy, which governs the extension interface and has a different support window and a different audience.

## Three version numbers

Three things version independently, and conflating them causes confusion that is expensive to undo later.

The agent version is what `lca --version` reports first and what a user names in a bug report. It covers the binary and everything in it.

The ABI version governs the extension interface. It moves slowly and freezes at 1.0 in Phase 8. See the ABI versioning policy.

Crate versions govern the workspace crates. Only `lca-ext-abi` is published to a registry, so only it needs a version an outsider reads. The others carry the workspace version and move together.

`lca --version` prints all three plus the build target, because a load failure needs all four to diagnose.

## Agent versioning

Semantic versioning, applied to what a user can observe.

A major change breaks the command line, the configuration format, the session format in a way that needs migration, or the supported ABI window in a way that disables working extensions.

A minor change adds a command, a configuration option, a built-in tool, or a capability, and keeps everything existing working.

A patch change fixes defects and changes nothing observable except the defect.

Two surfaces get explicit compatibility promises, because scripts depend on them. The headless output format with `--json` is stable within a major version: fields may be added, never removed or retyped. Exit codes are stable within a major version.

The terminal interface is not a compatibility surface. Layout, colors, and keybindings change in minor releases.

## Pre-1.0

Before 1.0, minor versions may break things. This is the normal semver convention and it is the honest description of a project in Phase 1 through Phase 6.

The changelog says plainly which minor releases break what. A user pinning a version is a supported choice.

1.0 ships at the end of Phase 8, at the same time the ABI freezes. Neither happens without the other, because an unfrozen ABI under a 1.0 binary is a promise the project cannot keep.

## Branching

`main` is always releasable. Every merge to `main` passes the full pipeline on all three operating systems.

Work happens on short-lived branches off `main` and merges back through a pull request. Long-lived feature branches are avoided, because the workspace has fifteen crates and a long branch turns into a rebase problem.

Release branches are cut only for a patch release against an older minor version, which happens for a security fix. Otherwise a release is a tag on `main`.

## Release cadence

There is no calendar cadence during Phase 0 through Phase 6. A release happens when a phase exit test passes.

After 1.0, minor releases are cut when there is something worth shipping, and patch releases are cut as needed. A security fix ships as soon as it is ready and does not wait for anything else.

## Artifact matrix

Every release builds six native targets and one web target.

| Target | Notes |
|---|---|
| `x86_64-unknown-linux-musl` | Fully static |
| `aarch64-unknown-linux-musl` | Fully static |
| `x86_64-apple-darwin` | Signed and notarized |
| `aarch64-apple-darwin` | Signed and notarized |
| `x86_64-pc-windows-msvc` | Signed |
| `aarch64-pc-windows-msvc` | Signed |
| `wasm32-wasip2` | Published as an npm package after jco transpilation |

Linux and Windows targets cross-compile from Linux runners, using `cargo-zigbuild` and `cargo-xwin`. macOS targets cross-compile the same way and then move to a macOS runner for signing and notarization, because notarization needs Apple tooling and a real macOS host.

Each artifact ships with a SHA-256 checksum. The checksum file for the whole release is published alongside the artifacts.

## Reproducibility

A tagged commit produces byte-identical binaries for a given target. This is a requirement, not an aspiration, and the pipeline checks it by building twice and comparing.

What this needs: a pinned Rust toolchain in `rust-toolchain.toml`, a committed `Cargo.lock`, no build-time timestamps, no embedded absolute paths, and a pinned Wasmtime version.

Reproducibility is what lets someone verify that a published binary matches the source. Signing proves who built it. Reproducibility proves what they built.

## Supply chain

`cargo-deny` runs on every merge and on a weekly schedule. It checks licenses against an allow list and dependencies against the advisory database.

A new dependency needs a written justification in the pull request: what it does, why writing it is worse, and what it pulls in transitively. The reviewer checks the size delta against the binary size budget.

The dependency count is a tracked number. A release that adds five dependencies without adding a feature is a signal worth discussing.

Dependency updates land in their own pull requests, not bundled with feature work, so a regression can be bisected to a single change.

## Size and startup gates

The pipeline measures binary size and cold start on every merge to `main`. A threshold breach fails the build.

The thresholds start at the values in the requirements and are ratcheted down when a release comes in well under, so the budget does not drift upward over time.

A pull request that needs a threshold raised states why in its description and needs explicit approval. This is the main defense against the core growing past the point of the project.

## Changelog

`CHANGELOG.md` at the repository root, written for users. Grouped as added, changed, fixed, and security. Each entry says what changed from the user's point of view, not which function was edited.

Breaking changes get their own section at the top of a release entry with a migration note.

The ABI changelog is separate, lives at `wit/CHANGELOG.md`, and is written for extension authors.

## Security advisories

A security fix ships as a patch release on the current minor version, and on the previous minor version if that version is still within the support window.

The advisory is published with the release. It names the affected versions, the impact, and the fix version. It does not include a working exploit.

A reporter gets acknowledgment within a few days and a coordinated disclosure date. The security contact and the reporting process live in `SECURITY.md`.

A vulnerability in the capability enforcement path or the WASM host is treated at the highest urgency, because those are the boundaries the whole design rests on.

## Deprecation

A command line flag, a configuration option, or a built-in tool that will be removed is marked deprecated in the release that decides it, not the release that removes it.

A deprecated item keeps working for the rest of the major version and warns when used. The warning names the replacement.

Removal happens only at a major boundary.

## Yanking

A published release that is actively harmful, meaning it corrupts sessions, leaks credentials, or bricks an install, is pulled from the distribution channels and replaced. The advisory explains what happened.

A release that is merely broken is fixed forward with a patch release. Yanking is for harm, not for defects.

## Installation channels

The primary channel is a shell installer that fetches the right artifact for the platform, verifies the checksum, and places the binary. A PowerShell equivalent covers Windows, because a bash installer is not a Windows installation story.

Package manager distribution follows once the release process is stable. Packaging is not a Phase 8 deliverable, and shipping to a package manager before the release process settles creates a support burden with stale versions.

The web build publishes as an npm package containing the transpiled module and its type definitions.

## Update checks

The agent checks for a newer version at most once per day, in the background, and never blocks startup on it. It reports a newer version in the status line and does not install anything.

The check is on by default in interactive mode and is the only outbound request the agent makes without the user asking for one (FR-CFG-6). It can be disabled with a configuration option, and it is disabled by default in headless mode, because a CI run should make no network request the user did not ask for.
