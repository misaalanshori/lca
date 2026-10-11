//! Split from `lib.rs` for the workspace file ceiling (gate 11,
//! #158). Behaviour unchanged.

/// [`version_text`], leaked to the `'static` lifetime clap's derive wants.
/// Computed at most once (gh #154): the `OnceLock` holds the one leak,
/// repeated calls return the same address.
pub fn version_static() -> &'static str {
    static CACHED: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();
    CACHED.get_or_init(|| Box::leak(version_text().into_boxed_str()))
}

/// `lca --version`: agent version, ABI version, crate version, build target
/// (release policy, ABI policy host version reporting).
///
/// The first line carries the *product* version: the crate version on the
/// stable line, `X.Y.Z.b<sha7>` on an unstable build (ADR-0043). The
/// `crate` line keeps the crate version, so the other three lines hold
/// their shape on both lines.
pub fn version_text() -> String {
    format!(
        "{}\nabi {}\ncrate {}\ntarget {}",
        env!("PRODUCT_VERSION"),
        lca_session::ABI_VERSION,
        env!("CARGO_PKG_VERSION"),
        env!("LCA_BUILD_TARGET"),
    )
}

/// Split a product version into its `X.Y.Z` base and its optional
/// `.b<sha7>` unstable suffix (ADR-0043's scheme). `--version`'s shape
/// test runs it over both forms.
pub fn split_product_version(product: &str) -> (&str, Option<&str>) {
    match product.split_once(".b") {
        Some((base, sha)) => (base, Some(sha)),
        None => (product, None),
    }
}

mod version_static_tests {
    // Verifies: gh #154 - repeated calls share one allocation.
    #[test]
    fn repeated_calls_return_the_same_address() {
        let first = super::version_static() as *const str;
        for _ in 0..100 {
            assert_eq!(super::version_static() as *const str, first);
        }
    }
}
