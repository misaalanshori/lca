//! Configuration merge tests: precedence, typed keys, inventories, refusals.
//!
//! Precedence per FR-CFG-1: flags > environment > project file > user file >
//! built-ins, and the project file only participates once the user trusts the
//! project (FR-PERM-9).
//!
//! Gh #545 split this file (it was 1,196 lines, four from gate 11) by subject.
//! The rows now live in `merge/`: `precedence`, `values`, `inventory`, and
//! `refusals`, with the two shared fixtures in `merge/support.rs`. A test
//! binary root cannot `mod` a subdirectory by name, so each child is declared
//! with `#[path]` - the same shape `tests/provider.rs` uses (gh #552) - and the
//! rows report as `merge::precedence::<row>` where they used to be
//! `merge::<row>`. `include!` would have kept the old names but hides the rows
//! from `crate::module_sources`, which is the failure mode gh #574 was filed
//! about; the test-set comparison in the cycle report states the rename.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

#[path = "merge/inventory.rs"]
mod inventory;
#[path = "merge/precedence.rs"]
mod precedence;
#[path = "merge/refusals.rs"]
mod refusals;
#[path = "merge/support.rs"]
mod support;
#[path = "merge/values.rs"]
mod values;
