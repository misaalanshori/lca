//! gh #546 - `parse_personas` reports a typed error, and the copy is unchanged.
//!
//! A public library function returning `Result<_, String>` makes every caller
//! parse prose to tell refusals apart, and `docs/lca-srdd.md`'s coding standards
//! ask for typed errors in library crates. The message, though, is
//! **user-facing**: `lca config` prints it, and the last cycle's pane pass showed
//! `expected a table of personas, got a string` coming out of a real load. A typed
//! error that reworded it would be a behaviour change wearing hygiene clothes, so
//! this row pins the type *and* the text.
//!
//! Two parts, deliberately:
//!
//! 1. **The type is there and is closed**: `PersonaError` is declared with
//!    `#[error("{0}")]` and a `From<String>`, `parse_personas` documents its
//!    `# Errors`, and both domain structs are `#[non_exhaustive]` with a
//!    constructor. These read source - `PersonaError`'s *shape* is a source fact,
//!    and the alternative (a `compile_fail` harness) is out of scope.
//! 2. **The copy survived end to end** through `Config::load`, which is the
//!    pipeline that actually printed it to a user. That assertion is the one that
//!    would have caught a reword, and it holds on the old tree as well by design -
//!    it is the receipt that this refactor changed no message.
//!
//! The typed error's own `Display`/`Error` behaviour is a unit row beside the
//! definition (`crates/lca-config/src/persona.rs`), where `toml` is in scope; this
//! crate cannot name a `toml::Value` without adding a dependency, which the cycle
//! does not.
//!
//! Verifies: gh #546 acceptance 1-2.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::path::PathBuf;

use lca_config::{Config, CustomCatalog, Persona};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(path: &str) -> String {
    std::fs::read_to_string(repo().join(path)).unwrap_or_else(|err| panic!("read {path}: {err}"))
}

/// Load one user file holding `body`, returning the refusal if it is refused.
fn load_user(name: &str, body: &str) -> Result<Config, String> {
    let dir = lca_testkit::scratch_path(&format!("lca-hyg546-{name}"));
    std::fs::create_dir_all(&dir).expect("fixture dir");
    let path = dir.join("user.toml");
    std::fs::write(&path, body).expect("fixture file");
    Config::load(&lca_config::LoadInput {
        user_file: Some(path),
        ..Default::default()
    })
    .map_err(|err| err.to_string())
}

// Verifies: gh #546 - the public error is typed, and both domain structs are
// closed to outside literals with a constructor that replaces them.
#[test]
fn parse_personas_reports_a_typed_error_with_the_same_copy() {
    let persona = read("crates/lca-config/src/persona.rs");
    assert!(
        persona.contains("pub struct PersonaError"),
        "`parse_personas` returns a typed error, not a `String`: a library crate's \
         public API (docs/lca-srdd.md coding standards)"
    );
    assert!(
        persona.contains("#[error(\"{0}\")]"),
        "the typed error renders the message it was built from, unchanged"
    );
    assert!(
        persona.contains("impl From<String> for PersonaError"),
        "the refusal paths keep their exact `format!` text: they convert, they do \
         not re-describe"
    );
    assert!(
        persona.contains("pub fn parse_personas(value: &toml::Value) -> Result<BTreeMap<String, Persona>, PersonaError>"),
        "the signature is the typed one"
    );
    assert!(
        persona.contains("/// # Errors") || persona.contains("///\n/// # Errors"),
        "`parse_personas` documents its `# Errors`, as a public fallible fn must"
    );
    assert!(
        persona.contains("#[non_exhaustive]\npub struct Persona"),
        "`Persona` is `#[non_exhaustive]`: a field added later breaks no caller"
    );
    let keys = read("crates/lca-config/src/keys.rs");
    assert!(
        keys.contains("#[non_exhaustive]\npub struct CustomCatalog"),
        "`CustomCatalog` is `#[non_exhaustive]` too"
    );

    // The constructors carry every field, so a caller loses nothing but writing
    // the literal by hand.
    let built = Persona::new(
        "opus",
        Some(Some("high".to_string())),
        Some(vec!["read".to_string()]),
        Some("be brief".to_string()),
    );
    assert_eq!(built.model, "opus");
    assert_eq!(
        built.summary(),
        "opus + high + tools read + instructions",
        "the bundle the constructor builds is the one `/persona` lists"
    );
    let catalog = CustomCatalog::new("acme", "Acme Corp", "https://example.invalid/catalog.toml");
    assert_eq!(
        (&catalog.id[..], &catalog.name[..], &catalog.url[..]),
        ("acme", "Acme Corp", "https://example.invalid/catalog.toml")
    );
}

// Verifies: gh #546 - the copy a user reads did not change. This is the receipt
// the cycle exists to keep: `lca config` printed exactly this line before the
// typed error and must print exactly this line after it.
#[test]
fn the_persona_refusal_copy_is_unchanged_end_to_end() {
    let err =
        load_user("not-a-table", "personas = \"plan\"\n").expect_err("a string is not a table");
    assert!(
        err.contains("expected a table of personas, got a string"),
        "the refusal text is the one the pane printed last cycle: {err}"
    );
    assert!(
        err.contains("user file"),
        "and it still names the layer it came from (FR-CFG-1): {err}"
    );

    // A malformed persona keeps its whole sentence: the persona, the field, and
    // the remedy. A typed wrapper that only carried a category would lose this.
    let err = load_user("unknown-field", "[personas.plan]\nmodle = \"opus\"\n")
        .expect_err("a misspelled field is refused");
    for fragment in [
        "persona `plan`",
        "unknown field `modle`",
        "model, thinking, tools, instructions",
    ] {
        assert!(
            err.contains(fragment),
            "the refusal keeps {fragment:?}: {err}"
        );
    }
}
