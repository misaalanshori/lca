//! Capture the build target triple for `lca --version`.

fn main() {
    // The build target triple for `lca --version` (release policy).
    let target = std::env::var("TARGET").expect("cargo sets TARGET for build scripts");
    println!("cargo:rustc-env=LCA_BUILD_TARGET={target}");
}
