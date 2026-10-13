//! Registry emission (gh #258 §3F): reference parses and
//! resolutions emit counts and digest prefixes — never content.
//! Feature-gated.

#![cfg(feature = "debug-trace")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code

fn capture(filter: &str, run: impl FnOnce()) -> String {
    let root = lca_testkit::scratch_path("debug-registry-emit");
    let paths = lca_debug::DebugPaths::in_dir(&root);
    let config = lca_debug::DebugConfig {
        paths: paths.clone(),
        filter: Some(filter.to_string()),
    };
    let subscriber = lca_debug::subscriber(&config).expect("subscriber");
    tracing::subscriber::with_default(subscriber, run);
    std::fs::read_to_string(&paths.log).unwrap_or_default()
}

// Verifies: gh #258 - catalog parses and local resolutions emit.
#[test]
fn parses_and_resolutions_emit() {
    let log = capture("lca_registry=trace", || {
        let root = lca_testkit::scratch_path("debug-registry-store");
        let component = root.join("word-count");
        std::fs::create_dir_all(&component).expect("mkdir");
        std::fs::write(
            component.join("extension.toml"),
            "name = \"word-count\"\nversion = \"1.0.0\"\nabi = \"0.6\"\nworlds = [\"tool\"]\n",
        )
        .expect("manifest");
        lca_registry::parse_catalog("[version]\nformat = 1\n[[entries]]\nname = \"word-count\"\nversion = \"1.0.0\"\nabi = \"0.6\"\ncategory = \"tool\"\ndescription = \"Counts words.\"\noci_ref = \"ghcr.io/example/word-count:abi-0.6\"\n")
            .expect("parse");
        lca_registry::resolve_local(&component, None).expect("resolve");
    });
    for event in ["catalog.parse", "resolve.local"] {
        assert!(log.contains(event), "registry emits {event}:\n{log}");
    }
}
