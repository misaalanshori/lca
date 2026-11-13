//! gh #553 - a `Manifest` cannot be built without passing its own validator, and
//! a startup-fatal `expect` is now a `LoadError`.
//!
//! `Manifest` is the capability trust boundary: `Manifest::parse` enforces that
//! the credential namespace equals the name (FR-PERM-6), that `oauth` declares
//! `net`, that no bare `net` wildcard or `env` wildcard slips through
//! (FR-PERM-15), that `net-local` ranges are canonical (FR-PERM-14), and that
//! resource kinds and limits are allowlisted and clamped. With 21 `pub` fields
//! and no `#[non_exhaustive]`, any crate could construct one and skip all of it.
//!
//! Two claims in the issue were measured and one was **refuted**, which is the
//! receipt this row keeps:
//!
//! * `#[non_exhaustive]` - confirmed needed. Measured: **zero** struct literals
//!   outside `manifest.rs` (`grep -rn "Manifest {" crates/ extensions/ tests/
//!   fuzz/` finds only a struct declaration, an `impl`, and the `Ok(Manifest
//!   { .. })` inside `parse`), so closing it breaks no caller and field reads are
//!   unaffected - no accessor set was needed.
//! * the six `expect`s - **partly refuted as a lint claim and confirmed as a
//!   behaviour one**. They were *not* in `#[cfg(test)]`; they were in production
//!   paths already carrying `#[allow(clippy::expect_used)]` with a written
//!   "startup-fatal" reason. So the lint was satisfied and the real defect was a
//!   `panic = "abort"` in release builds. Fixed by propagating.
//!
//! Verifies: gh #553 acceptance 1-2.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::path::PathBuf;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(path: &str) -> String {
    std::fs::read_to_string(repo().join(path)).unwrap_or_else(|err| panic!("read {path}: {err}"))
}

// Verifies: gh #553 - construction is closed to outside literals (**E0639**),
// which is the stronger receipt: the defect cannot be built any more.
#[test]
fn a_manifest_cannot_skip_its_parse() {
    let manifest = read("crates/lca-ext-host/src/manifest.rs");
    assert!(
        manifest.contains("#[non_exhaustive]\npub struct Manifest"),
        "`Manifest` is `#[non_exhaustive]`, so an outside `Manifest {{ .. }}` is E0639 \
         and every instance came through `parse`"
    );
    // Exactly one literal exists, inside `parse` itself.
    let literals = manifest
        .lines()
        .filter(|line| line.trim() == "Ok(Manifest {")
        .count();
    assert_eq!(
        literals, 1,
        "the only construction site is `parse` itself; found {literals}"
    );
    // And none outside the file - the measurement that makes this small.
    let mut offenders = Vec::new();
    for (path, text) in crate::module_sources("crates/lca-ext-host/src/lib.rs") {
        if path.ends_with("manifest.rs") {
            continue;
        }
        if text.contains("Manifest {")
            && !text.contains("pub struct Manifest")
            && !text.contains("Ok(Manifest {")
        {
            offenders.push(path);
        }
    }
    assert!(
        offenders.is_empty(),
        "no other file of the crate builds a `Manifest`: {offenders:?}"
    );

    // The invariants `parse` owns are still its own: the capability allowlist,
    // the credential-namespace field, and the two wildcard refusals all live in
    // `parse`, not in a constructor somewhere else.
    for rule in ["unknown capability", "namespace", "bare `*`", "net-local"] {
        let position = manifest.find("pub fn parse(").expect("`parse` is declared");
        assert!(
            manifest[position..].contains(rule),
            "`parse` still enforces {rule:?} (the invariants a struct literal would \
             have skipped)"
        );
    }
}

// Verifies: gh #553 - the startup-fatal `expect`s are a `Result` now, so a
// refused wasmtime engineer or a linker collision fails one load instead of
// aborting the process (`panic = "abort"` in release).
#[test]
fn a_refused_engine_or_linker_is_an_error_not_an_abort() {
    let host = read("crates/lca-ext-host/src/lib.rs");
    assert!(
        !host.contains("allow(clippy::expect_used)"),
        "no `expect_used` allowance is left in the host's production paths"
    );
    assert!(
        host.contains("fn build_engine() -> Result<Engine, LoadError>"),
        "the engine builder reports its failure instead of expecting one"
    );
    assert!(
        host.contains("wasmtime engine setup failed"),
        "and names what failed, so the operator can read the refusal"
    );
    for lens in ["wasi imports", "world imports", "completion imports"] {
        assert!(
            host.contains(&format!("LoadError::Link(format!(\"{lens}")),
            "`{lens}` is propagated through `LoadError::Link`, not expected"
        );
    }
    assert!(
        !host.contains(".expect(\"engine\")"),
        "the engine `expect` is gone"
    );
}
