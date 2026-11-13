//! gh #560 - a `ProviderError` cannot be built in a state its own classification
//! contradicts.
//!
//! The struct carries three fields that describe **one** fact: the message, the
//! headless envelope's `class`, and whether a retry could help (FR-CORE-6). With
//! public fields and no `#[non_exhaustive]`, any crate could write
//! `ProviderError { class: "auth", retryable: true, .. }` - "permanent, do not
//! retry" and "retry me" in the same value - and every consumer downstream would
//! faithfully believe both, including the retry loop that then spends three
//! backoffs on an expired key.
//!
//! `#[non_exhaustive]` + `ProviderError::new` makes that unrepresentable outside
//! the crate, which is the better outcome than a mutant: **the defect cannot be
//! constructed any more**, and the compile error proves it:
//!
//! ```text
//! error[E0639]: cannot create non-exhaustive struct using struct expression
//! ```
//!
//! So this row pins what a *behaviour* test can see: the attribute is there, every
//! named constructor routes through `new` (so no second place can pair a class
//! with a different retryability), and the pairing each constructor promises holds
//! at runtime.
//!
//! Verifies: gh #560 acceptance 1-2.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::path::PathBuf;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(path: &str) -> String {
    std::fs::read_to_string(repo().join(path)).unwrap_or_else(|err| panic!("read {path}: {err}"))
}

// Verifies: gh #560 - the construction site is the one function, and the type is
// closed to outside literals.
//
// Reading source in a test is unusual and deliberate here (the same technique
// `272-273-config-drift-guard.rs` uses for private fields): "no struct literal
// outside `new`" is a property of the source, and the alternative - a
// `compile_fail` harness - is out of scope. If a future field is added and one
// constructor wanders off `new`, the class/retryability pairing has a second
// author and this row says so.
#[test]
fn a_provider_error_cannot_be_constructed_with_a_lying_class() {
    let source = read("crates/lca-provider/src/lib.rs");
    assert!(
        source.contains("#[non_exhaustive]\npub struct ProviderError"),
        "`ProviderError` is `#[non_exhaustive]`, so an outside struct literal is a \
         compile error (E0639) and the inconsistent combination cannot be built"
    );
    // A literal is a line that is *only* `ProviderError {` (the declaration and
    // the `impl` line both contain the text but end differently), so count those.
    let literals = source
        .lines()
        .filter(|line| line.trim() == "ProviderError {")
        .count();
    assert_eq!(
        literals, 1,
        "exactly one `ProviderError {{ .. }}` literal exists (inside `new`); found {literals}"
    );
    // Every named constructor goes through `new`, so each one's class/retryable
    // pairing is written once, beside the class it names.
    for constructor in ["transport", "auth", "invalid"] {
        let body = source
            .split(&format!("pub fn {constructor}("))
            .nth(1)
            .unwrap_or_else(|| panic!("`{constructor}` exists"));
        let body = &body[..body.find("\n    }").expect("the constructor closes")];
        assert!(
            body.contains("ProviderError::new("),
            "`{constructor}` delegates to `new`, so it cannot pair its class with \
             a different retryability: {body}"
        );
    }
    // The class vocabulary stays a string: `docs/headless.md` pins its values as
    // the envelope's contract, and a kit may report a class core has not enumerated.
    assert!(
        source.contains("pub class: &'static str"),
        "the class is a string, not an enum (docs/headless.md)"
    );
}

// Verifies: gh #560 (FR-CORE-6, docs/headless.md) - the pairing each constructor
// promises, asserted at runtime so the delegation above is proven, not just read.
#[test]
fn every_named_constructor_keeps_its_class_and_retryability_together() {
    let transport = lca_provider::ProviderError::transport("connection reset");
    assert_eq!(transport.class, "transport");
    assert!(
        transport.retryable,
        "a transport failure is the one class a retry can fix (FR-CORE-6)"
    );

    let auth = lca_provider::ProviderError::auth("no key");
    assert_eq!(auth.class, "auth", "the constructor names its class");
    assert!(
        !auth.retryable,
        "`auth` is a permanent refusal: retrying it burns the budget and ends on \
         the wrong code (the gh #356 lesson)"
    );
    let invalid = lca_provider::ProviderError::invalid("bad request");
    assert_eq!(invalid.class, "invalid");
    assert!(!invalid.retryable, "and so is `invalid`");

    // And `new` passes the caller's judgement through unchanged: that is what
    // makes it the single place the three fields are chosen together.
    let custom = lca_provider::ProviderError::new("rate limited", "rate-limit", true);
    assert_eq!(custom.class, "rate-limit");
    assert!(custom.retryable);
    assert_eq!(custom.to_string(), "rate limited");
}
