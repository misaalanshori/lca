//! The provider world (Phase 3) in both delivery modes: identical
//! scripted results, case-per-case event mapping, FR-PROV-7/8 shapes,
//! and ADR-0012's three identity exports (NFR-25's Phase 3 half).
//!
//! Gh #552: this file was **at** gate 11's 1,200-line ceiling, which blocked the
//! next provider-parity row (and `docs/abi-versioning.md` wants one per ABI
//! addition). It is a valve now: the rows live in `provider/` under the subjects
//! the file already had - `parity` (the conformance script and its event
//! mapping), `identity` (ADR-0012's exports, the OAuth/credentials flows and
//! their refusals), `shipped` (the first-party components' own data paths) - and
//! `support` holds the one `Fixture` every row shares.
//!
//! The row names that result are `provider::parity::<row>` where they used to be
//! `provider::<row>`: a sibling module adds its segment to the path. The
//! alternative (`include!`) would paste the items into `provider` and keep the old
//! names, but it also hides them from `crate::module_sources`, which is how a
//! structural guard reads them (gh #574/#543) - and a guard that cannot see a row
//! is the failure mode those two issues were filed about. The test set was
//! therefore compared with the new segment stripped, and the comparison is in the
//! cycle report.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

// `#[path]` because this file is a test **binary root**, not a module: a bare
// `mod parity;` would make rustc look for `tests/parity.rs`. The paths are
// relative to the directory this file lives in (`tests/`), which is the rule
// `crate::module_sources` follows too (gh #552 fixed it there).
#[path = "provider/identity.rs"]
mod identity;
#[path = "provider/parity.rs"]
mod parity;
#[path = "provider/shipped.rs"]
mod shipped;
#[path = "provider/support.rs"]
mod support;
