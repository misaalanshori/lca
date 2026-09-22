//! Interactive mode, provided by `lca-tui`. This module is the wiring seam
//! for the Phase 1 CLI; the real entry point replaces it in the same phase.
use std::path::Path;

/// Enter the interactive interface for `cwd`, optionally resuming `resume`.
pub fn run(_cwd: &Path, _resume: Option<&str>) -> anyhow::Result<i32> {
    anyhow::bail!("interactive mode is being wired to lca-tui in this phase")
}
