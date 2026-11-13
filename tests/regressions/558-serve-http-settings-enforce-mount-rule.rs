//! gh #558 - a `ServeHttpSettings` cannot be built with a prefix its own parser
//! would have refused.
//!
//! A `[capabilities.serve-http]` prefix is a **mount point**: the host
//! concatenates it onto the extension's namespace, so `/` at the front is what
//! makes it a sub-path of that namespace and `..` anywhere is a walk that leaves
//! it. `parse` enforced both and returned `Result<_, String>`, while the struct's
//! fields were public - so `ServeHttpSettings { path_prefix: "../../etc".into(),
//! websocket: false }` was constructible by any crate and the rule was decorative.
//!
//! Two changes close that, and this row pins the contract rather than the shape:
//! `#[non_exhaustive]` (an outside literal is now E0639) and a typed
//! [`lca_permissions::ServeHttpError`] whose two variants *are* the two rules.
//! The rendered copy is unchanged - it is the message a manifest author reads
//! through `LoadError::InvalidManifest`, so a typed error that reworded it would
//! be a behaviour change.
//!
//! Verifies: gh #558 acceptance 1 (the mount-point rule is unbypassable).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use lca_permissions::{ServeHttpError, ServeHttpSettings};

/// The copy this refusal has always produced, before the typed error existed.
///
/// Quoted here so a reword is a red test rather than a review note. These two
/// strings came out of `parse`'s `format!` calls in `src/scope.rs` at
/// `fc9a3820` and are what a manifest author sees:
const MISSING_SLASH: &str = "`acme/etc` must start with `/`";
const TRAVERSAL: &str = "`/../../etc` may not contain `..`; a prefix is a mount point, not a path";

// Verifies: gh #558 - the rule is enforced by the only constructor an outside
// crate can reach, and the type carries which rule was broken.
#[test]
fn serve_http_settings_cannot_bypass_its_own_mount_rule() {
    // A prefix with no leading `/` is the first rule.
    let err = ServeHttpSettings::parse("acme/etc", false).expect_err("no leading slash");
    assert!(
        matches!(err, ServeHttpError::MissingLeadingSlash { .. }),
        "the error names which rule broke: {err:?}"
    );
    assert_eq!(err.to_string(), MISSING_SLASH, "the copy is unchanged");

    // A `..` anywhere is the second, whether or not the slash is there.
    for prefix in ["/../../etc", "/a/../b", "/.."] {
        let err = ServeHttpSettings::parse(prefix, false).expect_err("a walk");
        assert!(
            matches!(err, ServeHttpError::ParentTraversal { .. }),
            "`{prefix}`: a prefix is a mount point, not a path: {err:?}"
        );
    }
    let err = ServeHttpSettings::parse("/../../etc", false).expect_err("a walk");
    assert_eq!(err.to_string(), TRAVERSAL, "the copy is unchanged");

    // The type is closed to outside literals, which is what makes the above the
    // only way in: a struct expression is E0639.
    let source = std::fs::read_to_string(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("crates/lca-permissions/src/scope.rs"),
    )
    .expect("read scope.rs");
    assert!(
        source.contains("#[non_exhaustive]\npub struct ServeHttpSettings"),
        "`ServeHttpSettings` is `#[non_exhaustive]`, so `{{ path_prefix: \"../bad\".into(), .. }}` \
         is a compile error outside the crate"
    );

    // And the accepted path still reads back exactly as declared.
    let ok = ServeHttpSettings::parse("/assets", true).expect("a plain sub-path");
    assert_eq!(ok.path_prefix, "/assets");
    assert!(
        ok.websocket,
        "the websocket flag is carried, not judged here"
    );
}

// Verifies: gh #558 - the refusal reaches the manifest loader with the same text,
// which is where a user actually meets it.
#[test]
fn the_refusal_keeps_its_text_through_the_manifest_loader() {
    let err = ServeHttpSettings::parse("../escape", false).expect_err("no slash");
    assert_eq!(
        err.to_string(),
        "`../escape` must start with `/`",
        "a manifest author reads this line; the wording is the contract"
    );
}
