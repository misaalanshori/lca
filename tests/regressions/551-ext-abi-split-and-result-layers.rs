//! gh #551 - `lib.rs` is a contract root (the two modules are real files), and
//! the nested `Result<Result<.., String>>` is **kept on purpose** — with the
//! measurement that decided it.
//!
//! The issue asks for two things and this row answers one of them by declining:
//!
//! 1. **The split: taken.** `pub mod dispatch` was 524 inline lines and
//!    `pub mod host` 197; they are now `src/dispatch.rs` and `src/host.rs`, and
//!    `lib.rs` is the contract root. Every public path is unchanged
//!    (`lca_ext_abi::dispatch::ExtensionDispatch`, `lca_ext_abi::host::tool`).
//! 2. **`TransformVerdict`: declined, with the reason.** The nesting is the
//!    `with:`-mapped ABI's own shape, not an accident of the trait:
//!    `wit/world-context-transform.wit` exports
//!    `transform(messages) -> result<list<message>, string>`, and every host import
//!    is reached through a `Result` too. The **outer** `DispatchError` is a host
//!    transport fact (trap, disabled, missing world); the **inner** is the
//!    extension's own FR-CTX-3 verdict. **Measured**: adding
//!    `TransformVerdict` to `lca-protocol` and switching the trait compiles cleanly
//!    and then **eleven crates in `extensions/**` fail with `E0053 incompatible
//!    type for trait`** — i.e. it is a public-trait change in a *published* crate
//!    that every context-transform extension implements, which is an ABI-line
//!    decision (0.7), not this cycle's hygiene brief. `docs/extension-authoring.md`
//!    now documents the layering so the cycle that owns the ABI line can make the
//!    change with its own receipt.
//!
//! Verifies: gh #551 acceptance 1-2.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::path::PathBuf;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(path: &str) -> String {
    std::fs::read_to_string(repo().join(path)).unwrap_or_else(|err| panic!("read {path}: {err}"))
}

// Verifies: gh #551 (part 1) - the two inline modules are files, the crate root
// is a contract root, and no public path moved.
#[test]
fn the_inline_modules_are_files_and_the_paths_are_unchanged() {
    let module = crate::module_sources("crates/lca-ext-abi/src/lib.rs");
    let dispatch = module
        .iter()
        .find(|(path, _)| path.ends_with("dispatch.rs"))
        .unwrap_or_else(|| {
            panic!(
                "dispatch.rs exists; the walk returned {:?}",
                module.iter().map(|(p, _)| p.clone()).collect::<Vec<_>>()
            )
        })
        .1
        .clone();
    let host = module
        .iter()
        .find(|(path, _)| path.ends_with("host.rs"))
        .expect("host.rs exists")
        .1
        .clone();
    let root = &module[0].1;

    assert!(
        dispatch.contains("pub trait ExtensionDispatch"),
        "the dispatch contract lives in its own file"
    );
    assert!(
        host.contains("bindgen!"),
        "the host bindings module is its own file"
    );
    assert!(
        !root.contains("pub mod dispatch {") && !host.contains("pub mod host {"),
        "neither module is declared inline any more"
    );
    assert!(
        root.contains("pub mod dispatch;") && root.contains("pub mod host;"),
        "the crate root declares both as plain modules, so every path is the same"
    );
    assert!(
        root.lines().count() < 340,
        "`lib.rs` is a contract root now ({} lines)",
        root.lines().count()
    );
    assert!(
        dispatch.lines().count() < 600 && host.lines().count() < 260,
        "each half is well clear of gate 11 ({} / {})",
        dispatch.lines().count(),
        host.lines().count()
    );
    // The one `#[allow(missing_docs)]` the issue asks to retain is retained, as an
    // inner attribute of the generated module rather than on a declaration.
    assert!(
        host.contains("#![allow(missing_docs)]"),
        "generated bindings keep their missing-docs allowance"
    );
}

// Verifies: gh #551 (part 2) - the nested shape is documented as deliberate, in
// the code and in the authoring guide, because a future reader must not
// "simplify" it into a single enum without owning the ABI decision.
#[test]
fn the_two_result_layers_are_documented_as_distinct() {
    let dispatch = read("crates/lca-ext-abi/src/dispatch.rs");
    let position = dispatch
        .find("fn transform_messages(")
        .expect("`transform_messages` exists");
    // The doc block above the signature: every contiguous `///` line, walking up.
    // Attribute lines between the doc and the fn are skipped, and the walk stops
    // at the first line that is neither.
    let before = &dispatch[..position];
    let mut collected: Vec<&str> = Vec::new();
    for line in before.lines().rev() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("///") {
            collected.push(line);
            continue;
        }
        if trimmed.starts_with("#[") || trimmed.is_empty() {
            if collected.is_empty() {
                continue; // attributes/blanks between the doc and the signature
            }
            // A blank *after* we started collecting means the block ended, unless
            // it is a `///`-style blank (which starts with `///` and never gets here).
            break;
        }
        break;
    }
    // Flattened: rustfmt wraps doc prose, so "host transport" can straddle a line.
    let doc = collected
        .into_iter()
        .rev()
        .map(|line| line.trim().trim_start_matches("///").trim())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        doc.contains("host transport") || doc.contains("host-level"),
        "the doc says what the OUTER error means: {doc}"
    );
    assert!(
        doc.contains("verdict") && doc.contains("FR-CTX-3"),
        "and what the INNER one means: {doc}"
    );
    assert!(
        dispatch.contains("Result<Result<Vec<ChatMessage>, String>, DispatchError>"),
        "the signature is unchanged - the nesting is the ABI's shape, not a style choice"
    );

    let guide = read("docs/extension-authoring.md");
    assert!(
        guide.contains("two `Result` layers"),
        "the authoring guide records the layering for extension authors"
    );
    assert!(
        guide.contains("host transport") && guide.contains("FR-CTX-3"),
        "and names both layers' meanings"
    );
    // The enum the issue proposed is deliberately absent from the published crate.
    let protocol = read("crates/lca-protocol/src/dispatch.rs");
    assert!(
        !protocol.contains("enum TransformVerdict"),
        "`TransformVerdict` was declined: it is a 0.7 ABI decision, not a hygiene edit"
    );
}
