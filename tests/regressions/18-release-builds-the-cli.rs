//! Latent release-pipeline defect (found in cycle 3, fixed in `c9eb924`):
//! the regression-suite package at the workspace root made a bare
//! `cargo build --release` build only that package, so the publish legs
//! found no `lca` binary to package and the 0.2.0 publish failed. The fix
//! set `default-members` to the crates and extensions and passed
//! `-p lca-cli` explicitly in the pipeline.
//!
//! This encodes the shape that broke: the workspace default build must
//! reach the crate that produces the `lca` binary. It fails against the
//! pre-fix tree (no `default-members`, so the root regression package was
//! the only default).
//!
//! Verifies: NFR-23 (release pipeline).

#[test]
fn a_default_release_build_reaches_the_lca_binary() {
    let workspace = include_str!("../../Cargo.toml");
    // Only the `[workspace]` table: the root package below `[package]` is
    // the regression target, not a build default.
    let workspace_table = workspace.split("[package]").next().unwrap_or(workspace);
    assert!(
        workspace_table.contains("default-members"),
        "the workspace must declare default-members, or a bare `cargo build --release` \
         builds the root regression package and nothing else"
    );
    assert!(
        workspace_table.contains("\"crates/*\""),
        "and the default members must include the crates (where `lca-cli` lives)"
    );
    let cli = include_str!("../../crates/lca-cli/Cargo.toml");
    assert!(
        cli.contains("name = \"lca\""),
        "`crates/lca-cli` is the package that builds the `lca` binary the pipeline packages"
    );
}
