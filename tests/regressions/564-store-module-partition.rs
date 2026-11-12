//! Gate-11 guard for gh #564 phase 1: the store is a module directory, and the
//! append path's write/flush order did not move with the code.
//!
//! ## Why this is a guard and not just the compiler
//!
//! `rustc` proves every path today (see the cycle report's compile probe). What
//! nothing in the tree stops is the two shapes this split could regress into:
//!
//! 1. **A `#[path]` sibling** - a declaration a structural guard cannot see from
//!    the module name, and the shape gh #557 spent a cycle removing.
//! 2. **The append path drifting.** `docs/session-log-format.md` says an append
//!    writes one line and flushes it. The mutation in the report (dropping
//!    `flush()`) came back **green**, because `File::flush` is a no-op on Unix -
//!    so this is the one place a structural row is the only pin available: it
//!    asserts the three calls still exist, in that order, in one function.
//!
//! ## What this pins
//!
//! 1. the module walk reaches `rows`, `lines`, `read`, `write` and `forks`, and
//!    no file in the module declares a child with `#[path]`;
//! 2. `append` still writes the line, writes the newline, and flushes - in that
//!    order, inside `write.rs`;
//! 3. `mod.rs` keeps the headroom the split bought (it is 264 lines of 1,200).

use crate::module_sources;

/// The store module's own files, as `(path, source)`, through the shared walk
/// (which skips `#[cfg(test)]` children, so this reads production code only).
fn store_module() -> Vec<(String, String)> {
    module_sources("crates/lca-session/src/store/mod.rs")
}

fn source_of<'a>(module: &'a [(String, String)], suffix: &str) -> &'a str {
    module
        .iter()
        .find(|(path, _)| path.ends_with(suffix))
        .map(|(_, text)| text.as_str())
        .unwrap_or_else(|| panic!("the store module must still hold {suffix}"))
}

// Verifies: gh #564 - the store's readers, writers and tree live in sibling files,
// reached by the ordinary `mod` declarations a structural guard walks.
#[test]
fn the_store_module_reaches_its_read_write_and_fork_siblings() {
    let module = store_module();
    let paths: Vec<&str> = module.iter().map(|(path, _)| path.as_str()).collect();
    for expected in [
        "crates/lca-session/src/store/rows.rs",
        "crates/lca-session/src/store/lines.rs",
        "crates/lca-session/src/store/read.rs",
        "crates/lca-session/src/store/write.rs",
        "crates/lca-session/src/store/forks.rs",
    ] {
        assert!(
            paths.contains(&expected),
            "the walk from `store/mod.rs` must reach {expected}: a guard reading the \
             module otherwise stops seeing the code the split moved there \
             (docs/testing-plan.md's module-walk rule, gh #574). Found: {paths:?}"
        );
    }
    // Gh #557's rule: a module directory declares its children by plain `mod`.
    for (path, text) in &module {
        assert!(
            !text.contains("#[path = \""),
            "{path} declares a child with `#[path]`: the store's children are ordinary \
             files, and a `#[path]` sibling is a declaration a future guard cannot see \
             from the module name"
        );
    }
    // And the partition still reads as it was measured: one file per job.
    let read = source_of(&module, "store/read.rs");
    let write = source_of(&module, "store/write.rs");
    assert!(read.contains("pub fn read("), "`read` is the reader's file");
    assert!(
        write.contains("pub fn append("),
        "`append` is the writer's file"
    );
}

// Verifies: gh #564 + `docs/session-log-format.md`'s Durability section - an
// append is one line, then a flush, in that order, in one function. This is the
// row that survives the mutation the behavioural suite cannot see (`File::flush`
// is a no-op on Unix, so dropping it changes nothing observable there).
#[test]
fn an_append_writes_one_line_then_flushes() {
    let module = store_module();
    let write = source_of(&module, "store/write.rs");
    let append = write
        .split("pub fn append(")
        .nth(1)
        .expect("`append` is in the writer's file");
    let body = &append[..append.find("\n    }\n").unwrap_or(append.len())];
    let line_write = body
        .find("write_all(line.as_bytes())")
        .expect("append writes the record's line");
    let newline = body
        .find("write_all(b\"\\n\")")
        .expect("append terminates the line");
    let flush = body.find("flush()").expect(
        "append flushes: docs/session-log-format.md says the write is flushed, and a \
         document that promises a flush which the code no longer performs is drift",
    );
    assert!(
        line_write < newline && newline < flush,
        "the order is write, newline, flush (found at {line_write}, {newline}, {flush}): \
         a flush before the newline would report a half-written record as durable"
    );
    // And no fsync crept in: the doc is explicit that a crash may lose the tail.
    assert!(
        !write.contains("sync_all(") && !write.contains("sync_data("),
        "the store does not fsync per record (docs/session-log-format.md, Durability): \
         a synchronous write per token is the cost that section refuses"
    );
}

// Verifies: gh #564 - the file keeps the headroom the split bought (1,177 lines
// before, 264 after), so a boundary creeping back in fails here first.
#[test]
fn the_store_file_keeps_the_headroom_the_split_bought() {
    let module = store_module();
    let mod_rs = source_of(&module, "store/mod.rs").lines().count();
    assert!(
        mod_rs < 600,
        "store/mod.rs is {mod_rs} lines: the split left it at 264, and growth past ~600 \
         means an owner has crept back in - that wants its own cycle before gate 11 does"
    );
}
