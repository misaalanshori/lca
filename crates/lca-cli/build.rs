//! Capture the build target triple and the product version for `lca --version`.

#[allow(clippy::expect_used)] // cargo always sets these for a build script.
fn main() {
    // The build target triple for `lca --version` (release policy).
    let target = std::env::var("TARGET").expect("cargo sets TARGET for build scripts");
    println!("cargo:rustc-env=LCA_BUILD_TARGET={target}");

    // The product version (ADR-0043): `LCA_BUILD_VERSION` when the
    // unstable workflow bakes `X.Y.Z.b<sha7>`, else the crate version, so
    // a stable build is byte-identical to what it is today.
    let version = std::env::var("LCA_BUILD_VERSION").unwrap_or_else(|_| {
        std::env::var("CARGO_PKG_VERSION").expect("cargo sets CARGO_PKG_VERSION for build scripts")
    });
    println!("cargo:rustc-env=PRODUCT_VERSION={version}");
    println!("cargo:rerun-if-env-changed=LCA_BUILD_VERSION");
}
