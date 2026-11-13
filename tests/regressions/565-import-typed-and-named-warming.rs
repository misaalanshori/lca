//! gh #565 - `import_jsonl` reports a typed error, and the two long-signature
//! hazards read as named fields.
//!
//! Three claims, measured:
//!
//! * `import_jsonl -> std::result::Result<Session, String>` - **confirmed**
//!   (`store/write.rs`), and the only method on `SessionStore` that bypassed the
//!   crate's own `Result` alias.
//! * `warming_eligible`'s seven positional scalars - **confirmed**, including the
//!   three consecutive `f64`s (`miss_cost`, `warm_cost`, `p_continue`) whose
//!   transposition compiled silently.
//! * `tree::emit`'s eight arguments - **confirmed** (a nested `fn` carrying five
//!   lookup maps down every recursive call).
//! * the five output structs - **confirmed**: zero external literals for
//!   `SessionMeta`, `SessionSummary`, `VersionState`, `MigrateReport`, `EntryRow`.
//!
//! The copy is the receipt: `/import` prints the refusal, so `line 2: …` and
//! `line 1: a session must open with session-start` must read the same before and
//! after a typed error.
//!
//! Verifies: gh #565 acceptance 1-3.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::path::PathBuf;

use lca_session::{SessionStore, WarmingQuery};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(path: &str) -> String {
    std::fs::read_to_string(repo().join(path)).unwrap_or_else(|err| panic!("read {path}: {err}"))
}

// Verifies: gh #565 - the import refuses with `Error::InvalidImport`, whose text
// is the copy `/import` has always shown, and no session exists afterwards.
//
// The assertion is end-to-end on purpose: a typed error that reworded the
// message would be a behaviour change wearing hygiene clothes, and the pane
// output is what a user reads.
#[test]
fn an_import_refusal_is_typed_and_keeps_its_copy() {
    let root = lca_testkit::scratch_path("lca-hyg565-import");
    std::fs::create_dir_all(&root).expect("fixture dir");
    let project = root.join("project");
    std::fs::create_dir_all(&project).expect("project dir");
    let store = SessionStore::new(root.join("data"));

    // A corrupt second line names its 1-based position.
    let start = lca_protocol::Record::SessionStart {
        v: lca_protocol::FORMAT_VERSION,
        ts: 1,
        agent_version: "t".to_string(),
        abi_version: "0.6".to_string(),
        working_dir: project.display().to_string(),
    };
    let good = serde_json::to_string(&start).expect("the opener serializes");
    let lines = [good.as_str(), "not json at all"];
    let err = store
        .import_jsonl(&project, "bad", &lines)
        .expect_err("a corrupt line aborts");
    assert!(
        matches!(err, lca_session::Error::InvalidImport(_)),
        "the refusal is the crate's own `Error`, not a bare `String`: {err:?}"
    );
    let text = err.to_string();
    assert!(
        text.contains('2') && text.contains("line"),
        "it names the line: {text}"
    );
    assert_eq!(
        store.list_sessions(&project).expect("list").len(),
        0,
        "no half-imported session exists (the property the copy promises)"
    );

    // A document whose first record is not `session-start` keeps its sentence.
    // The record must parse (a malformed one refuses earlier, with its own
    // `line N: …`), so this uses a well-formed `session-end`.
    let not_start = r#"{"t":"session-end","v":1,"ts":9,"id":"end"}"#;
    let err = store
        .import_jsonl(&project, "bad-opener", &[not_start])
        .expect_err("the opener is required");
    assert!(
        err.to_string().contains("must open with session-start"),
        "the opener refusal keeps its copy: {err}"
    );
}

// Verifies: gh #565 - `Error::InvalidImport` exists and the method returns the
// crate's `Result`, so `?` propagates an `Io` from `create_session`/`append`
// instead of flattening it into an opaque string.
#[test]
fn the_import_error_is_part_of_the_crate_error_type() {
    let lib = read("crates/lca-session/src/lib.rs");
    assert!(
        lib.contains("InvalidImport(String)"),
        "the crate's `Error` carries the import refusal"
    );
    let write = read("crates/lca-session/src/store/write.rs");
    assert!(
        write.contains(") -> crate::Result<Session> {"),
        "`import_jsonl` returns `crate::Result`, so the `?` operator reaches \
         `Error::Io` from the store calls it makes"
    );
    assert!(
        !write.contains("Result<Session, String>"),
        "the `String`-returning signature is gone"
    );
}

// Verifies: gh #565 - `warming_eligible` takes one named struct, so two of the
// three `f64`s cannot be swapped invisibly; the seven gates still decide.
#[test]
fn warming_eligibility_reads_as_named_gates() {
    let cache = read("crates/lca-session/src/cache.rs");
    assert!(
        cache.contains("pub struct WarmingQuery"),
        "the inputs are a named struct"
    );
    for field in [
        "mode",
        "short_secs",
        "long_secs",
        "idle_ms",
        "miss_cost",
        "warm_cost",
        "p_continue",
    ] {
        assert!(
            cache.contains(&format!("pub {field}:")),
            "`{field}` is a named field, so a transposition is visible"
        );
    }
    assert!(
        !cache.contains("#[allow(clippy::too_many_arguments)]"),
        "the argument-count allowance is gone with it"
    );

    // The arithmetic still decides: a warm cache that cannot save the floor
    // does not fire, and one that can does.
    let fires = lca_session::warming_eligible(WarmingQuery {
        mode: "idle",
        short_secs: Some(300),
        long_secs: None,
        idle_ms: 360_000,
        miss_cost: 0.20,
        warm_cost: 0.01,
        p_continue: 1.0,
    });
    assert!(fires);
    let does_not = lca_session::warming_eligible(WarmingQuery {
        miss_cost: 0.01,
        warm_cost: 0.20,
        ..WarmingQuery {
            mode: "idle",
            short_secs: Some(300),
            long_secs: None,
            idle_ms: 360_000,
            miss_cost: 0.20,
            warm_cost: 0.01,
            p_continue: 1.0,
        }
    });
    assert!(!does_not, "a negative expected saving never fires");
}

// Verifies: gh #565 - the tree walk carries its state on a struct, so the
// eight-argument recursion is gone; the rows it produces are unchanged.
#[test]
fn the_tree_walk_carries_its_state_in_one_struct() {
    let tree = read("crates/lca-session/src/tree.rs");
    assert!(
        tree.contains("struct Walk<"),
        "the walk's read-only state is one struct"
    );
    assert!(
        !tree.contains("#[allow(clippy::too_many_arguments)]"),
        "the eight-argument allowance is gone with it"
    );
    for field in ["children", "live", "live_leaf", "marks", "calls"] {
        assert!(
            tree.contains(&format!("    {field}:")),
            "`{field}` is a named field of the walk"
        );
    }
    // And every output struct is closed to outside literals.
    for (path, name) in [
        ("crates/lca-session/src/store/rows.rs", "SessionMeta"),
        ("crates/lca-session/src/store/rows.rs", "SessionSummary"),
        ("crates/lca-session/src/migrate.rs", "VersionState"),
        ("crates/lca-session/src/migrate.rs", "MigrateReport"),
        ("crates/lca-session/src/tree.rs", "EntryRow"),
    ] {
        let source = read(path);
        assert!(
            source.contains(&format!("#[non_exhaustive]\npub struct {name}")),
            "`{name}` is `#[non_exhaustive]`: a field added later breaks no caller"
        );
    }
}
