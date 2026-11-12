//! Gate-11 guard for gh #548 phase 2: the turn module's boundaries are siblings,
//! and no method changed its path.
//!
//! ## What the split was
//!
//! `crates/lca-core/src/turn/mod.rs` was 1,142 lines, 58 from gate 11's ceiling,
//! with 1,018 of them inside one function (`turn_body`) and the rest split between
//! two boundaries: request assembly (FR-CTX-2/3/4, FR-CACHE-5/6, FR-SESS-4) and
//! persistence/compaction (FR-SESS-4, `docs/session-log-format.md`). This cycle
//! moved those two boundaries to `turn/request.rs` and `turn/persist.rs` and left
//! the loop alone; `mod.rs` is 511 lines.
//!
//! ## Why this needs a guard rather than a compiler
//!
//! `rustc` proves every path today - that is what the compile probe in the cycle
//! report does (`lca_core::turn::{effective_context_window, catalog_context_window}`
//! and the `lib.rs` re-exports still resolve). What nothing in the tree stops is the
//! *next* move: `turn/` is a directory whose children are declared by plain `mod`
//! lines in `mod.rs`, and two shapes of drift are silent.
//!
//! 1. **A `#[path]` sibling**, which would change where the file lives without
//!    changing the module path - the shape gh #557 spent a cycle removing.
//!    Worse here: a `#[path]` child is where a future split's guards stop walking,
//!    because `crate::module_sources` follows `mod name;` and `#[path = "…"]` but
//!    not a declaration it cannot see.
//! 2. **`turn_body` growing again.** The whole reason this cycle stopped where it
//!    did is that the loop stays one function until a cycle takes it on with its
//!    own rows and mutations. A `mod.rs` that climbs past its headroom quietly is
//!    how the next worker inherits a 1,200-line file again.
//!
//! ## What this pins
//!
//! 1. the module walk reaches `turn/request.rs` and `turn/persist.rs`, and every
//!    file in the module is declared by `mod` (no `#[path]` in the module);
//! 2. `turn_body` is still one function, in `mod.rs`, and the two boundaries are
//!    not in the loop's file (the partition this cycle landed, not the reverse);
//! 3. `mod.rs` keeps headroom under gate 11's ceiling.
//!
//! The first row fails on the pre-split tree (no `turn/request.rs` exists); the
//! second fails on any tree where the boundaries were moved *into* `mod.rs` again.

use std::path::Path;

/// The module's own files, as `(path, source)`, via the shared walk (which skips
/// `#[cfg(test)]` children and so sees production code only).
fn module() -> Vec<(String, String)> {
    crate::module_sources("crates/lca-core/src/turn/mod.rs")
}

fn lines_of(sources: &[(String, String)], suffix: &str) -> usize {
    sources
        .iter()
        .find(|(path, _)| path.ends_with(suffix))
        .map_or(0, |(_, text)| text.lines().count())
}

// Verifies: gh #548 phase 2 - the request and persist boundaries are module
// files beside the loop, reached by the module walk and declared by `mod`.
#[test]
fn the_turn_module_reaches_its_request_and_persist_siblings() {
    let module = module();
    let paths: Vec<&str> = module.iter().map(|(path, _)| path.as_str()).collect();
    for expected in [
        "crates/lca-core/src/turn/request.rs",
        "crates/lca-core/src/turn/persist.rs",
    ] {
        assert!(
            paths.contains(&expected),
            "the walk from `turn/mod.rs` must reach {expected}: a structural guard \
             reading the module otherwise stops seeing the code the split moved there \
             (docs/testing-plan.md's module-walk rule, gh #574). Found: {paths:?}"
        );
    }
    // Gh #557's rule: a module directory declares its children by plain `mod`.
    for (path, text) in &module {
        assert!(
            !text.contains("#[path = \""),
            "{path} declares a child with `#[path]`: the turn module's children are \
             ordinary files, and a `#[path]` sibling is a declaration a future guard \
             cannot see from the module name"
        );
    }
}

// Verifies: gh #548 phase 2 - `turn_body` stays the loop, alone in `mod.rs`, and
// the two boundaries stayed out of it. Each half of the partition is asserted on
// both sides, so a future cycle that moves code back fails here rather than
// silently re-inflating the file.
#[test]
fn the_loop_stays_in_mod_rs_and_the_boundaries_stay_out_of_it() {
    let module = module();
    let mod_rs = module
        .iter()
        .find(|(path, _)| path.ends_with("turn/mod.rs"))
        .map(|(_, text)| text.clone())
        .expect("the module's own file");

    assert_eq!(
        mod_rs.matches("async fn turn_body").count(),
        1,
        "`turn_body` is exactly one function: it is the loop, and its phases share \
         long-lived state across `await` points, so splitting it is a separate change \
         with its own rows (see the module docs)"
    );
    for boundary in [
        "async fn assemble_request",
        "fn narrow_stable_prefix",
        "async fn persist_response",
        "async fn compact_before_turn",
    ] {
        assert!(
            !mod_rs.contains(boundary),
            "`{boundary}` is back in `turn/mod.rs`: the split moved the request and \
             persist boundaries to `request.rs`/`persist.rs`, and a move *back* would \
             put the file near the ceiling again"
        );
    }
    for (moved, suffix) in [
        ("async fn assemble_request", "turn/request.rs"),
        ("fn narrow_stable_prefix", "turn/request.rs"),
        ("async fn persist_response", "turn/persist.rs"),
        ("async fn compact_before_turn", "turn/persist.rs"),
    ] {
        let source = module
            .iter()
            .find(|(path, _)| path.ends_with(suffix))
            .map(|(_, text)| text)
            .expect("the sibling the split created");
        assert!(
            source.contains(moved),
            "`{moved}` lives in {suffix}: the seam is the one this cycle measured, not \
             whichever file happens to hold it later"
        );
    }
}

// Verifies: gh #548 phase 2 - the file keeps the headroom the split bought. 1,142
// lines before, 511 after: this row fails long before a reviewer notices the
// ceiling approaching again.
#[test]
fn the_loop_file_keeps_the_headroom_the_split_bought() {
    let module = module();
    let mod_rs = lines_of(&module, "turn/mod.rs");
    assert!(
        mod_rs < 700,
        "turn/mod.rs is {mod_rs} lines: the split left it at 511, and growth past ~700 \
         means a boundary has crept back in or the loop grew - either way it needs its \
         own cycle before gate 11 does"
    );
    assert!(
        Path::new("crates/lca-core/src/turn/mod.rs").is_file(),
        "and it is a real file on disk, not an empty module"
    );
}
