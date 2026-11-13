//! gh #550 - the four claims, measured, and two of them **refuted**.
//!
//! | claim | verdict |
//! |---|---|
//! | `#[inline(always)]` forces `tracing` into every caller on-feature builds | **confirmed and fixed**: the attribute is now `cfg_attr`-scoped so the on-feature arm asks for a plain `inline` and `emit_inner` keeps the six-arm dispatch in one function |
//! | the `.1` spare path is duplicated between `DebugPaths::spare_for` and `RotatingWriter::with_cap` | **confirmed and fixed**: both were building `path.push(".1")` separately; there is now one `spare_path()` and both call it |
//! | `spare_for` takes `&self` without reading it | **confirmed**: it is an associated-shaped method on a struct it does not touch; it stays a method (tests call it that way) but delegates, and the free function is the real home |
//! | "pedantic lints" (`# Errors` sections, `map_or`, redundant closures, mid-block `const`s, `#[must_use]`) | **partly refuted**: the workspace enables **no** pedantic lint set (`[workspace.lints.clippy]` is `unwrap_used`/`expect_used`/`panic` only), so those edits cannot fail a gate. The ones that are *contracts* (`#[must_use]` on the redaction functions - AGENTS.md Rule 6 - and the mid-block `const` move) were taken; the rest are named as declined |
//!
//! What was **refuted outright**: the duplication claim named a "second copy of
//! the spare logic" in general; measured, there were exactly two sites, both in
//! `logfiles.rs`, and no third (`grep -rn '\.1"'` finds only the formatter and the
//! two builders).
//!
//! Verifies: gh #550 acceptance 1-3.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::path::PathBuf;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(path: &str) -> String {
    std::fs::read_to_string(repo().join(path)).unwrap_or_else(|err| panic!("read {path}: {err}"))
}

// Verifies: gh #550 - no emitter carries an unconditional `inline(always)`; the
// off-feature build still gets it (that is what makes it a no-op).
#[test]
fn the_inline_attribute_is_scoped_to_the_off_feature_build() {
    for file in [
        "crates/lca-debug/src/lib.rs",
        "crates/lca-debug/src/core.rs",
        "crates/lca-debug/src/net.rs",
        "crates/lca-debug/src/permissions.rs",
        "crates/lca-debug/src/session.rs",
        "crates/lca-debug/src/wasm.rs",
    ] {
        let source = read(file);
        // Every `inline(always)` must be reached by a `cfg_attr`, or sit inside a
        // `#[cfg(not(feature = "debug-trace"))]` item - the empty `emit_inner`
        // body, which only exists when there is nothing to inline into.
        let lines: Vec<&str> = source.lines().collect();
        for (index, line) in lines.iter().enumerate() {
            let trimmed = line.trim_start();
            // Doc prose that *names* the attribute is not an attribute.
            if !line.contains("inline(always)")
                || trimmed.starts_with("///")
                || trimmed.starts_with("//")
            {
                continue;
            }
            let scoped = trimmed.starts_with("#[cfg_attr(");
            let empty_body = index > 0
                && lines[index - 1]
                    .trim_start()
                    .starts_with("#[cfg(not(feature = \"debug-trace\"))]");
            assert!(
                scoped || empty_body,
                "{file}: `inline(always)` must be scoped, otherwise the on-feature build \
                 force-inlines the `tracing` dispatch at every call site (NFR-1): {line}"
            );
        }
        assert!(
            source.contains("#[cfg_attr(not(feature = \"debug-trace\"), inline(always))]"),
            "{file}: the off-feature arm still asks for `inline(always)` - that is what \
             collapses the call to nothing"
        );
    }
    // And the dispatch itself is one function, not inlined into every caller.
    let root = read("crates/lca-debug/src/lib.rs");
    assert!(
        root.contains("#[cfg_attr(feature = \"debug-trace\", inline)]"),
        "the on-feature arm is a plain `inline`"
    );
    assert!(
        root.contains("fn emit_inner("),
        "the six-arm `tracing` dispatch lives in one function"
    );
}

// Verifies: gh #550 - one spare-path computation, and both callers use it. The
// defect class is a `.1` suffix built twice, so the row counts the builders.
#[test]
fn there_is_one_spare_path_builder() {
    let logfiles = read("crates/lca-debug/src/logfiles.rs");
    let builders = logfiles.matches("spare.push(\".1\")").count();
    assert_eq!(
        builders, 1,
        "exactly one place pushes the `.1` suffix (found {builders}); two is the \
         gh #550 defect"
    );
    assert!(
        logfiles.contains("pub fn spare_path("),
        "the one builder is a named function both callers can reach"
    );
    assert!(
        logfiles.contains("DebugPaths::spare_for") || logfiles.contains("spare_path(path)"),
        "`spare_for` delegates instead of re-implementing"
    );

    // Behaviour: the method and the free function agree, and the writer's spare
    // is the file the method names.
    let paths = lca_debug::DebugPaths::in_dir(std::path::Path::new("/tmp/logs"));
    assert_eq!(
        paths.spare_for(&paths.log),
        lca_debug::logfiles::spare_path(&paths.log)
    );
    assert_eq!(
        paths.spare_for(&paths.log).to_str(),
        Some("/tmp/logs/debug.log.1")
    );
    assert_eq!(
        paths.spare_for(&paths.wire).to_str(),
        Some("/tmp/logs/debug-wire.log.1")
    );
}

// Verifies: gh #550 (AGENTS.md Rule 6) - a redaction that nobody bound is a
// secret kept in plain sight, so `#[must_use]` is on every `redact::*` function.
// This is the one part of the issue's "pedantic" half that is a real contract.
#[test]
fn every_redaction_is_must_use() {
    let redact = read("crates/lca-debug/src/redact.rs");
    for name in [
        "redact_secret",
        "redact_header",
        "redact_url",
        "redact_json",
        "redact_credential",
    ] {
        let position = redact
            .find(&format!("pub fn {name}"))
            .unwrap_or_else(|| panic!("`{name}` exists"));
        let before = &redact[..position];
        let attr_line = before.lines().rev().find(|line| !line.trim().is_empty());
        assert_eq!(
            attr_line.map(str::trim),
            Some("#[must_use]"),
            "`{name}` is `#[must_use]`: discarding a redacted value must warn"
        );
    }
    // And the query helpers on the domain vocabulary too.
    let root = read("crates/lca-debug/src/lib.rs");
    for name in ["target", "is_wire"] {
        let position = root
            .find(&format!("pub fn {name}("))
            .unwrap_or_else(|| panic!("`Domain::{name}` exists"));
        let before = &root[..position];
        let attr_line = before.lines().rev().find(|line| !line.trim().is_empty());
        assert_eq!(
            attr_line.map(str::trim),
            Some("#[must_use]"),
            "`Domain::{name}` is `#[must_use]`"
        );
    }
    // The redaction still behaves: the prefix stays, the secret does not.
    assert_eq!(
        lca_debug::redact::redact_secret("abcd1234efgh5678"),
        "abcd1234...[REDACTED]"
    );
    // Under two chars there is nothing safe to keep.
    assert_eq!(lca_debug::redact::redact_secret("a"), "[REDACTED]");
}
