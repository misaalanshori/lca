//! Session storage tests: the append-only log, truncation recovery, listing,
//! forking, the compaction view, export, and attachment reachability.
//!
//! Every test runs against a scratch directory; the sandboxed-`HOME` fixture in
//! `lca-testkit` covers environment isolation for tests that touch process state,
//! and the three fixtures these rows share (`scratch`, `store`, `user_record`)
//! live in `tests/common/mod.rs` (gh #564 phase 2: nine targets had each grown a
//! byte-identical private copy).
//!
//! Gh #564 phase 2 also split the 36 rows by subject, because the file was 1,177
//! lines, 23 from gate 11, and a new record type adds a row here. A test binary
//! root cannot `mod` a subdirectory by name, so each child is a `#[path]` sibling
//! (the `tests/provider.rs` shape from gh #552) and the rows report as
//! `log::<subject>::<row>` where they used to be `log::<row>`. `include!` would
//! have kept the names but hides the rows from `crate::module_sources` - the
//! failure mode gh #574 was filed about - so the names gave way, with the
//! comparison in the cycle report.
//!
//! FR-SESS-6 (a corrupt line keeps the prefix) stays one file: it is one rule seen
//! from five angles, and a reader that gets it half-right loses a user's history.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

mod common;

#[path = "log/append.rs"]
mod append;
#[path = "log/attachments.rs"]
mod attachments;
#[path = "log/forks.rs"]
mod forks;
#[path = "log/listing.rs"]
mod listing;
#[path = "log/meta.rs"]
mod meta;
#[path = "log/truncation.rs"]
mod truncation;
#[path = "log/view.rs"]
mod view;
