//! Pi-conformance harness (RM-001, #93): pi-documented observable behavior,
//! pinned as LCA acceptance cases.
//!
//! Every test carries a `// Verifies: pi:<anchor>` comment naming the exact
//! pi source (pi tree per `docs/parity-baseline.md`: pin
//! `v1.0.0-25-ga276dabe5`, live oracle `~/gits/pi` at record time).
//! Anchors point at pi 1.0.0-era docs and tests; the harness pins
//! OBSERVABLE behavior (bytes, exit codes, event shapes), never pi
//! internals. Settled divergences (yolo prompts, provider profiles, theme
//! roles, append-only fork-dirs) are documented in `docs/pi-parity.md`,
//! never asserted against.
//!
//! Red witnesses for future milestones are `#[ignore]`-gated (named owners
//! in `docs/pi-parity.md`) so main stays green; run them with
//! `cargo nextest run -E 'test(pi_parity)' --run-ignored`.
//!
//! The `pi_parity_` prefix is the selection contract:
//! `cargo nextest run -E 'test(pi_parity)'` runs exactly this target.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

mod agent;
mod cache;
mod config;
mod headless;
mod session;
mod thinking;
mod tools;
mod witnesses;
