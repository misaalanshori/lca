# Contributing to LCA

LCA is a terminal coding agent that ships as one static Rust binary:
a small trusted core, a capability-sandboxed WebAssembly extension
system, append-only sessions, and a pi-inspired terminal UI. This
guide orients a new contributor. `AGENTS.md` (repo root) is the
terse working-rules companion; the `docs/` tree is normative.

## What to read first

1. `README.md` — what the product is and how it presents itself.
2. `docs/lca-srdd.md` — the requirements and architecture document;
   it indexes every other doc.
3. `docs/adr/` — the decision records (start with `README.md`, then
   0001, 0002, 0013). A record is never edited to change its decision;
   a changed decision gets a dated annotation or a superseding record.
4. `docs/testing-plan.md` — how work is verified (test-driven, with a
   fake provider so the suite runs offline).
5. `docs/extension-authoring.md` — if you are building an extension.

## Philosophy (what reviews enforce)

- **Small trusted core, wide extension surface.** The core owns the
  agent loop, built-in tools, renderer, permissions, and the extension
  host. Everything else belongs in an extension. A new core feature
  needs a written argument for why it cannot be one (ADR-0013).
- **Capabilities, not promises.** Extensions get exactly what their
  manifest declares; deny by default. See `docs/capabilities.md`.
- **Tests pin behavior, never implementation.** Failing tests are
  expected on new work; unexpected failures are the signal. See
  `AGENTS.md` ("Testing culture").
- **Docs are the law.** Behavior changes ship with doc updates in the
  same change; documentation drift is a defect.
- **Minimal diffs.** The shortest working change wins; no speculative
  abstractions, no scaffolding "for later".

## Architecture (one paragraph per layer)

Sixteen crates in one Cargo workspace (`crates/`), plus first-party
extensions under `extensions/` (same tree third-party authors use).
Dependencies flow downward only; only `lca-cli` depends on everything:

- `lca-cli` — the `lca` binary (dispatch, headless, UI runner).
- `lca-core` — the Tokio agent loop (one turn at a time).
- `lca-protocol` — shared types, no I/O. `lca-provider` — the
  provider trait. `lca-session` — append-only logs, fork/resume.
- `lca-tools` — built-in tools (`read`, `write`, `edit`, `list`,
  `glob`, `grep`, `shell`) behind a swappable backend trait.
- `lca-permissions` — grants, rules, filesystem scopes.
- `lca-ext-abi` — the `lca:ext` WIT contract. `lca-ext-host` — the
  Wasmtime host + capability enforcement. `lca-ext-native` — in-binary
  extensions through the same dispatch trait.
- `lca-registry` — OCI/HTTPS resolvers + lockfile.
- `lca-config`, `lca-sdk` (embedding API), `lca-testkit` (the scripted
  fake provider every test uses).
- `lca-tui` — terminal engine + widgets (zero agent imports).
  `lca-ui` — the agent interface on top.

The extension contract lives in `wit/` (WIT package `lca:ext`,
currently ABI `0.6`); the crates are internal structure.

## Build instructions

- Toolchain: stable Rust pinned by `rust-toolchain.toml`
  (`rustup` picks it up automatically). Edition 2024.
- `cargo build --release -p lca-cli` → `target/release/lca`.
- Test runner: `cargo nextest run --workspace`
  (`cargo test` works too). Tests run offline by default; live
  provider tests skip without keys.
- Quality bar on every change: `cargo fmt --check`,
  `cargo clippy --workspace --all-targets --all-features -- -D
  warnings`, `RUSTDOCFLAGS="-D warnings" cargo doc --workspace
  --no-deps`. `unsafe` is forbidden except with a documented
  exemption; `unwrap`/`expect`/`panic` are denied outside tests.
- No file over 1,200 lines (`scripts/ceiling-check.sh`) — split as
  you grow. Every public item documented.
- New dependencies need a written justification (what it does, why
  writing it is worse, transitive footprint) plus a row in the SRDD
  dependency table.

## Issues and pull requests

- Issues use EARS-style requirements language where they state
  behavior; bug reports should include the transcript, the platform,
  and `lca --version`. Only the maintainer assigns priorities —
  discussion in comments is welcome from anyone.
- Pull requests stay small and green: the full gate list is in
  `docs/release-policy.md` (eleven gates incl. traceability, fuzz,
  wasm, perf, installers). **Commits on `main` are green or reverted.**
- A fix for a shipped defect carries its regression test in
  `tests/regressions/`, named after the tracking issue.
- A change touching the ABI needs the WIT edit, regenerated bindings,
  a `wit/CHANGELOG.md` entry, and the conformance extension updated
  in the same change (see `docs/abi-versioning.md`).
- An ABI change also needs an ADR when it changes a design decision
  rather than filling in an agreed shape.
- Commit messages: short imperative subject, body explains why and
  cites the requirement/decision/issue.

## Security

Report vulnerabilities privately per `SECURITY.md`, never in a public
issue. The capability enforcement path and the WASM host get the
highest-urgency review. Secrets never touch logs, tests, or commits.
