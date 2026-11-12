//! Session-install rows: the store, the archive, the resolvers, the catalog, and
//! git sources.
//!
//! Gh #561 split this file (1,140 lines) by the subjects both it and the crate
//! already had, so a row now sits beside the module it exercises:
//! `registry/store.rs`, `registry/archive.rs`, `registry/resolve.rs`,
//! `registry/catalog.rs`, `registry/git.rs`, with the shared fixtures in
//! `registry/support.rs`.
//!
//! A test binary root cannot `mod` a subdirectory by bare name, so each child is a
//! `#[path]` sibling (the `tests/provider.rs` shape from gh #552) and rows report
//! as `registry::<subject>::<row>` where they used to be `registry::<row>`;
//! `include!` would have kept the names and hidden the rows from
//! `crate::module_sources` (gh #574's failure mode), so the names gave way and the
//! comparison is in the cycle report.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

#[path = "registry/archive.rs"]
mod archive;
#[path = "registry/catalog.rs"]
mod catalog;
#[path = "registry/git.rs"]
mod git;
#[path = "registry/resolve.rs"]
mod resolve;
#[path = "registry/store.rs"]
mod store;
#[path = "registry/support.rs"]
mod support;
