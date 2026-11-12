//! The two fixtures every merge row shares (`write`, `scratch`).
//!
//! Gh #545 split `tests/merge.rs` (1,196 lines, 4 from gate 11) by subject, and
//! these two were the only shared code in the whole file: `write` creates a
//! parent directory and drops a TOML file, `scratch` hands out a per-row temp
//! path. Keeping them in one place is why the split could not have copied
//! either of them four times.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::path::Path;

/// Write `content` to `path`, creating its parent directory.
pub(super) fn write(path: &Path, content: &str) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(path, content).expect("write");
}

/// A fresh scratch directory for one row.
pub(super) fn scratch(name: &str) -> std::path::PathBuf {
    lca_testkit::scratch_path(name)
}
