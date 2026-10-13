//! Subscriber wiring (gh #258 §5): domains reach their file, the
//! explicit filter overrides defaults, garbage falls back. Scoped
//! subscribers (`with_default`) so tests never fight over the global.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code
#![cfg(feature = "debug-trace")]

use lca_debug::{DebugPaths, Domain};

// Verifies: gh #258 - internal domains land in debug.log, wire
// domains in debug-wire.log, through the real subscriber.
#[test]
fn domains_reach_their_file() {
    let root = lca_testkit::scratch_path("debug-files");
    let paths = DebugPaths::in_dir(&root);
    let config = lca_debug::DebugConfig {
        paths: paths.clone(),
        filter: None,
    };
    let subscriber = lca_debug::subscriber(&config).expect("subscriber");
    tracing::subscriber::with_default(subscriber, || {
        lca_debug::core::event("turn.start", "id=1");
        lca_debug::net::event("http.request", "GET https://x.test/");
    });
    let log = std::fs::read_to_string(&paths.log).expect("log");
    let wire = std::fs::read_to_string(&paths.wire).expect("wire");
    assert!(
        log.contains("turn.start"),
        "core lands in debug.log:\n{log}"
    );
    assert!(!log.contains("http.request"), "wire stays out:\n{log}");
    assert!(
        wire.contains("http.request"),
        "net lands in debug-wire.log:\n{wire}"
    );
    assert!(!wire.contains("turn.start"), "core stays out:\n{wire}");
}

// Verifies: gh #258 - an explicit filter (a flag or LCA_LOG) takes
// control of both files; below-threshold events vanish.
#[test]
fn explicit_filter_overrides_and_silences() {
    let root = lca_testkit::scratch_path("debug-filter");
    let paths = DebugPaths::in_dir(&root);
    let config = lca_debug::DebugConfig {
        paths: paths.clone(),
        filter: Some("lca_core=off,lca_net=trace".to_string()),
    };
    let subscriber = lca_debug::subscriber(&config).expect("subscriber");
    tracing::subscriber::with_default(subscriber, || {
        lca_debug::core::event("turn.start", "id=1");
        lca_debug::net::event("http.request", "GET https://x.test/");
    });
    let log = std::fs::read_to_string(&paths.log).expect("log");
    let wire = std::fs::read_to_string(&paths.wire).expect("wire");
    assert!(!log.contains("turn.start"), "off wins:\n{log}");
    assert!(wire.contains("http.request"), "trace passes:\n{wire}");
}

// Verifies: gh #258 - the DebugConfig type and file routing agree.
#[test]
fn file_for_matches_the_layers() {
    let root = lca_testkit::scratch_path("debug-routing");
    let paths = DebugPaths::in_dir(&root);
    assert_eq!(paths.file_for(Domain::Core), &paths.log);
    assert_eq!(paths.file_for(Domain::Net), &paths.wire);
}
