//! Gh #548 phase 1: `crates/lca-core/src/registry.rs` (1,159 lines, 41 from gate
//! 11's ceiling) became the module directory `crates/lca-core/src/registry/`.
//!
//! The split itself is a move - every method body is byte-identical to the
//! pre-split file (checked by script, and by `cargo nextest list -p lca-core`
//! being byte-identical), and `src/lib.rs` did not change. What these rows pin is
//! the part a *later* change could quietly undo:
//!
//! 1. **The module is a directory, and its children are declared by `mod`.** A
//!    plain `registry.rs` reached its sibling through `#[path = "registry_manage.rs"]
//!    mod manage;`; now there is no `#[path]` anywhere in the module, so a file
//!    added beside `mod.rs` without a declaration is a compile error rather than
//!    an invisible module. This row **fails on the pre-split tree**: the path it
//!    reads (`registry/mod.rs`) did not exist, and the file it replaces carried
//!    the `#[path]`.
//! 2. **Every name `lib.rs` re-exports from the module is still declared there.**
//!    `lib.rs` must not change in this split, which means the module has to keep
//!    producing exactly those names; a move that buried one behind a private
//!    child would surface as a missing re-export, and a `pub use` reaching a
//!    child module would surface here as a path the split renamed.
//!
//! Both read the module through the shared walk (gh #574/#543), so a second split
//! does not need this file edited.
//!
//! Verifies: gh #548, FR-EXT-11 (the dispatch table and its collision report stay
//! reachable at the paths the front ends name).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failing assertion.

/// The registry module's files, in declaration order (the walk is recursive).
fn registry_module() -> Vec<(String, String)> {
    crate::module_sources("crates/lca-core/src/registry/mod.rs")
}

// Verifies: gh #548 - **the registry is a module directory whose children are
// plain `mod` declarations.**
//
// Mutation: adding a `#[path = "…"]` attribute in front of any `mod` line
// reddens this row (that is the shape a plain `registry.rs` needed, and keeping
// one now would mean a child could be declared from somewhere other than the
// module directory).
#[test]
fn the_registry_module_declares_its_children_by_mod_without_path_attributes() {
    let files = registry_module();
    assert!(
        files.len() >= 5,
        "the module is the partition it claims to be (mod, tools, hooks, plan, \
         manage); found {} file(s): {:?}",
        files.len(),
        files.iter().map(|(p, _)| p).collect::<Vec<_>>()
    );
    for (path, text) in &files {
        assert!(
            !text.contains("#[path = \""),
            "{path} carries a `#[path]` declaration; a module directory declares \
             its children by `mod`, and the attribute is how the plain-file shape \
             used to reach a sibling (gh #548)"
        );
    }
    // And the two halves this cycle created are reachable from the walk, which
    // is the only proof that a `mod` declaration names a real file.
    for expected in [
        "crates/lca-core/src/registry/tools.rs",
        "crates/lca-core/src/registry/hooks.rs",
    ] {
        assert!(
            files.iter().any(|(path, _)| path == expected),
            "{expected} is not declared from `registry/mod.rs`: {:?}",
            files.iter().map(|(p, _)| p).collect::<Vec<_>>()
        );
    }
}

// Verifies: gh #548 - **every name `lib.rs` re-exports from the registry module
// is still declared inside it.**
//
// `lib.rs` is not allowed to change in this split (a change there means the split
// moved a public path), so the module owes it exactly these names. A later move
// that renamed `CollisionReport` or made `BUILTIN_TOOLS` private would leave the
// re-export pointing at nothing - which is a compile error in the crate, but only
// if the re-export is exercised; this row names the obligation directly.
#[test]
fn the_reexports_lib_rs_names_are_still_declared_by_the_module() {
    let lib = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/lca-core/src/lib.rs"),
    )
    .expect("read lca-core/src/lib.rs");
    let start = lib
        .find("pub use registry::{")
        .expect("`lib.rs` still re-exports the registry module's names");
    let list = &lib[start..start + lib[start..].find('}').expect("the re-export list closes")];
    let names: Vec<&str> = list
        .trim_start_matches("pub use registry::{")
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .collect();
    assert!(
        names.len() >= 5,
        "the re-export list is the one this cycle preserved: {names:?}"
    );

    let module = registry_module();
    for name in names {
        // A declaration of the name in the module: a `pub` item (struct, enum,
        // const, fn, type) or a re-export. Matching on the item keyword keeps the
        // row from passing on a mention inside a doc comment.
        let declared = module.iter().any(|(_, text)| {
            [
                "pub struct ",
                "pub enum ",
                "pub const ",
                "pub fn ",
                "pub type ",
                "pub use ",
            ]
            .iter()
            .any(|prefix| text.contains(&format!("{prefix}{name}")))
        });
        assert!(
            declared,
            "`lib.rs` re-exports `{name}` from the registry module, but no file in \
             it declares that name any more: {:?}",
            module.iter().map(|(p, _)| p).collect::<Vec<_>>()
        );
    }
}
