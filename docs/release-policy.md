# Release and versioning policy

Version 0.1, 2026-09-20.

This covers how LCA is versioned, branched, built, signed, and published. It is separate from the ABI versioning policy, which governs the extension interface and has a different support window and a different audience.

## Three version numbers

Three things version independently, and conflating them causes confusion that is expensive to undo later.

The agent version is what `lca --version` reports first and what a user names in a bug report. It covers the binary and everything in it.

The ABI version governs the extension interface. It moves slowly. It froze at 1.0 in Phase 8, that freeze was reopened by the ADR-0028 development window, and it freezes for good at product 1.0. On the 0.x line its label tracks the product minor (`0.x.y` ships `abi 0.x`); the numbers join at 1.0 and stay joint for majors afterward. See the ABI versioning policy.

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

*Amended 2026-09-25 (ADR-0028):* the joint-ship rule stands, but "the ABI freezes" now means the ADR-0028 re-freeze — the freeze-for-good on the owner's judgment that the interface is mature. Phase 8's initial freeze was reopened for a development window while the product is still in active development, so product 1.0 waits for the re-freeze. Neither ships without the other.

## Branching

`main` is always releasable. Every merge to `main` passes the full pipeline on all three operating systems.

Work happens on short-lived branches off `main` and merges back through a pull request. Long-lived feature branches are avoided, because the workspace has fifteen crates and a long branch turns into a rebase problem.

Release branches are cut only for a patch release against an older minor version, which happens for a security fix. Otherwise a release is a tag on `main`.

## Release cadence

There is no calendar cadence during Phase 0 through Phase 6. A release happens when a phase exit test passes.

After 1.0, minor releases are cut when there is something worth shipping, and patch releases are cut as needed. A security fix ships as soon as it is ready and does not wait for anything else.

## Two release lines

**Stable** is every rule in this document: a tag on `main`, a changelog entry, six assets, checksums, attestations. **Unstable** (ADR-0043) is one rolling pre-release for every commit that passed the pipeline: a GitHub release tagged `unstable`, marked `prerelease`, whose assets are replaced after each green commit and whose binaries report `X.Y.Z.b<sha7>` as their first `--version` line. It guarantees three things: it was built from a green `main` commit, its checksums come from the same directory as its binaries, and it is provenance-attested exactly as a stable artifact is. It guarantees nothing else — it may break, it gets no changelog entry (the history is git and the CI run artifacts, kept per build for 90 days), and the next green commit replaces it.

Annotation, 2026-10-02 (ADR-0043), on the tag rule above: `unstable` is the exception. It is a channel pointer, not a release announcement. The tag is created once and never moved; what rolls is the release it heads — its assets are replaced — so the tag's target SHA is explicitly *not* the provenance of the binaries currently behind it, which is what the attestation and the baked version are for. `releases/latest` never resolves to it, because it is marked `prerelease` and `--latest=false`, which is exactly what keeps the stable installers' redirect path true. Every other tag stays as immutable as it was.

## Artifact matrix

Every release builds six native targets. The seventh row is deferred with NFR-11 (`scripts/deferred-requirements.txt`): until the web target ships, a release publishes no npm package.

| Target | Notes |
|---|---|
| `x86_64-unknown-linux-musl` | Fully static |
| `aarch64-unknown-linux-musl` | Fully static |
| `x86_64-apple-darwin` | Unsigned; provenance-attested |
| `aarch64-apple-darwin` | Unsigned; provenance-attested |
| `x86_64-pc-windows-msvc` | Unsigned; provenance-attested |
| `aarch64-pc-windows-msvc` | Unsigned; provenance-attested |
| `wasm32-wasip2` | Deferred with NFR-11: no npm package exists until the web target ships |

Linux and Windows targets cross-compile from Linux runners, using `cargo-zigbuild` and `cargo-xwin`. macOS targets build natively on macOS runners, because Apple's toolchain wants a real macOS host. Nothing is code-signed or notarized in 0.1: trust rests on reproducibility plus the provenance attestation below.

Each artifact ships with a SHA-256 checksum. The checksum file for the whole release is published alongside the artifacts.

## Reproducibility

A tagged commit produces byte-identical binaries for a given target. This is a requirement, not an aspiration, and the pipeline checks it by building twice and comparing.

What this needs: a pinned Rust toolchain in `rust-toolchain.toml`, a committed `Cargo.lock`, no build-time timestamps, no embedded absolute paths, and a pinned Wasmtime version.

Reproducibility is what lets someone verify that a published binary matches the source. Signing proves who built it, and where no code signature exists, the provenance attestation carries that proof instead.

## Supply chain

`cargo-deny` runs on every merge and on a weekly schedule. It checks licenses against an allow list and dependencies against the advisory database.

Every artifact on a release also carries a GitHub artifact attestation (in-toto provenance, produced by the pipeline's `Sign the artifacts (provenance attestation)` step and verifiable through the repository's attestations endpoint). That attestation proves who built an artifact; reproducibility proves what they built.

A new dependency needs a written justification in the pull request: what it does, why writing it is worse, and what it pulls in transitively. The reviewer checks the size delta against the binary size budget.

The dependency count is a tracked number. A release that adds five dependencies without adding a feature is a signal worth discussing.

Dependency updates land in their own pull requests, not bundled with feature work, so a regression can be bisected to a single change.

## Gate list

Eleven gates run on the pipeline; a change that turns any of them red does not land. Gate 10 joined with the installers (ADR-0040) and is registered here as well as in `docs/testing-plan.md` section 15 and the README, so this table is the canonical list.

| # | Gate | What runs | Where |
|---|---|---|---|
| 1 | Format and lint | `cargo fmt --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo doc` with `-D warnings` | `ci.yml` (fmt in ubuntu gates; clippy/doc in the windows job and the core group per OS) |
| 2 | Test suite | `cargo nextest run --workspace` on Linux, macOS, and Windows, including the real-terminal suites and the regression tests | `ci.yml` test jobs |
| 3 | NFR timing | The five tests the serial release filter selects on a release build, on the machine to itself: the NFR-4, NFR-5, and NFR-29 timing rows, the epoch functional row, and the conformance row the `hook` name-filter catches | `ci.yml` ubuntu gates |
| 4 | Dependency audit and license check | `cargo-deny` against `deny.toml`, on every merge and on the weekly schedule | `ci.yml` ubuntu gates; `deny.yml` schedule |
| 5 | Requirements traceability | `scripts/traceability.sh`: every FR/NFR carries at least one verifying test (NFR-30) | `ci.yml` ubuntu gates |
| 6 | Fuzz targets build | `scripts/fuzz-check.sh`: every fuzz target still compiles (testing plan section 13) | `ci.yml` |
| 7 | Extension components build | `scripts/wasm-check.sh`: every extension compiles for `wasm32-wasip2` | `ci.yml` |
| 8 | Release targets build | `scripts/release-targets-check.sh`: all six release targets compile with the publish toolchain | `ci.yml` |
| 9 | Size and startup | `scripts/perf-gate.sh` against the ratcheted thresholds, plus the cache-hit-ratio benchmark (NFR-7, NFR-31) | `ci.yml` |
| 10 | Installers | `scripts/install-check.sh`: `tests/install/test_install_sh.sh` plus `shellcheck --shell=sh install.sh`; the PowerShell suite runs in the same `install` job on Windows | `ci.yml` |
| 11 | File-size ceiling | `scripts/ceiling-check.sh`: every tracked `*.rs` file stays at or under the workspace's 1,200-line ceiling; the offenders are printed | `ci.yml` ubuntu gates |

The manual release gate (below) is separate: it is a human on a real terminal before a tag, not a pipeline step, and no count of the eleven includes it.

## Size and startup gates

The pipeline measures binary size and cold start on every merge to `main`. A threshold breach fails the build.

The thresholds start at the values in the requirements and are ratcheted down when a release comes in well under, so the budget does not drift upward over time.

A pull request that needs a threshold raised states why in its description and needs explicit approval. This is the main defense against the core growing past the point of the project.

## Manual release gate

Before a release tag, a human runs the release candidate's TUI for a couple of minutes on a real terminal and checks the transcript, a resize, and each modal (`/login`, the permission prompt, the model picker). Snapshot tests cover rendering logic; they do not catch a terminal-version or platform-specific break, which is what this gate is for.

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

The primary channel is a shell installer at the repository root, `install.sh`, fetched from `raw.githubusercontent.com` and piped into `sh`: it resolves the right artifact for the platform, verifies the checksum, places the binary, and puts it on PATH. Running the same one-liner again is the update path, and `--uninstall` reverses it. `install.ps1` covers Windows with the same semantics, because a bash installer is not a Windows installation story. The full specification, the flags, and the manual alternative are `docs/installation.md`; the decision is ADR-0040; the scripts are gated by gate 10 in the table above. Both release lines install through those same scripts: no flag resolves the stable line exactly as before, `--unstable`/`-Unstable` resolves the rolling line's `download/unstable` directory with the same mandatory verification, and the flag is latest-only, so it is a usage error next to `--version` (FR-INSTALL-10, ADR-0043).

Package manager distribution follows once the release process is stable. Packaging is not a Phase 8 deliverable, and shipping to a package manager before the release process settles creates a support burden with stale versions.

The web build publishes as an npm package containing the transpiled module and its type definitions.

## Update checks

The agent checks for a newer version at most once per day, in the background, and never blocks startup on it. It reports a newer version in the status line and does not install anything.

The check is on by default in interactive mode and is the only outbound request the agent makes without the user asking for one (FR-CFG-6). It can be disabled with a configuration option, and it is disabled by default in headless mode, because a CI run should make no network request the user did not ask for.
