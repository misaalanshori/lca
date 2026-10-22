//! gh #279 - the extension's advertised description agrees with what its code
//! actually offers.
//!
//! The defect: `extensions/antigravity/extension.toml` and
//! `extensions/catalog.toml` both described the extension as *"Google Antigravity
//! models, **including image generation**, via subscription login"*, while
//! `is_selectable` (`extensions/antigravity/src/models.rs`) refused every model
//! whose id contains `image`. A user reads the manifest description before
//! installing, and the catalog row is what an extension hub presents, so the claim
//! propagated to every surface a person can see.
//!
//! This is a consent-surface accuracy problem, not cosmetics: describing a
//! capability the code then refuses is the same class as an under-declared grant,
//! just in the other direction.
//!
//! # Why the description moved and not the filter
//!
//! The brief allowed either side to change and asked for evidence. The evidence
//! said the filter is correct and the claim is not:
//!
//! 1. `lca_protocol::StreamEvent` has **no image variant** - text, reasoning, tool
//!    calls, usage, error, vendor. So there is no place for an image *result* to
//!    arrive.
//! 2. `ContentBlock::Image` (ADR-0029) exists, but it is the **request** side: the
//!    extension turns an attached image into Gemini's `inlineData` part. That is
//!    image *input*, a different capability from image generation, and it is not
//!    what the description said.
//! 3. The extension's only `VendorEvent` is `THINKING_SIGNATURE_KIND`. Nothing
//!    emits an image event, so the doc's claim that `vendor-event` "carries image
//!    generation results" described a channel that is not used that way.
//! 4. The vendor reference `~/gits/pi-antigravity/` - the standing ground truth for
//!    this provider, and the reason this cycle made no live call - refuses
//!    `/image/i` too, in its own `isSelectableRuntimeModelId`, *and* implements
//!    generation through a **separate direct image endpoint** plus a
//!    `generate_image` tool. LCA implements neither. So the capability is real in
//!    the reference and absent here: enabling it is a feature with no path in this
//!    cycle, and the honest fix is to stop advertising it.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(path: &str) -> String {
    std::fs::read_to_string(root().join(path)).unwrap_or_else(|err| panic!("read {path}: {err}"))
}

/// The description string the manifest advertises.
fn manifest_description() -> String {
    let manifest = read("extensions/antigravity/extension.toml");
    for line in manifest.lines() {
        if let Some(rest) = line.strip_prefix("description = ") {
            return rest.trim().trim_matches('"').to_string();
        }
    }
    panic!("antigravity's manifest has no description");
}

/// The description string this extension's catalog row advertises.
fn catalog_description() -> String {
    let catalog = read("extensions/catalog.toml");
    let mut in_row = false;
    for line in catalog.lines() {
        let trimmed = line.trim();
        if trimmed == "[[extension]]" || trimmed.starts_with("[[extension]]") {
            in_row = false;
            continue;
        }
        if trimmed == "name = \"antigravity\"" {
            in_row = true;
            continue;
        }
        if in_row && let Some(rest) = trimmed.strip_prefix("description = ") {
            return rest.trim().trim_matches('"').to_string();
        }
    }
    panic!("the catalog has no antigravity row with a description");
}

/// Whether `extensions/antigravity/src/models.rs` refuses ids containing `image`.
///
/// Read as a fact about the code rather than reimplemented: this guard's job is to
/// notice that the *description* and the *filter* disagree, so the filter's
/// existence is what it consults.
fn the_filter_refuses_image_models() -> bool {
    let models = read("extensions/antigravity/src/models.rs");
    models.contains(r#"lower.contains("image")"#)
}

// Verifies: gh #279 - the three advertised surfaces say the same thing.
//
// `extension.toml` is the consent surface (what a user reads before installing),
// `catalog.toml` is the hub listing (how the claim propagates), and
// `docs/providers/antigravity.md` is the reference. A fix that corrected two of
// three would have moved the drift rather than removed it, which is exactly what
// this row exists to prevent.
#[test]
fn the_three_surfaces_agree() {
    let manifest = manifest_description();
    let catalog = catalog_description();
    let doc = read("docs/providers/antigravity.md");
    assert!(
        doc.contains(&manifest),
        "the provider doc must quote the manifest's description verbatim; it does \
         not contain {manifest:?}"
    );
    assert_eq!(
        manifest, catalog,
        "the manifest's description and the catalog row's must match: a user sees \
         one before installing and the other in the hub"
    );
}

// Verifies: gh #279 - and none of them claims image generation, because the code
// refuses it.
//
// This is the row that would have caught the original drift. It reads the claim out
// of the *data* and the behaviour out of the *code* and requires them to agree, so
// re-adding "including image generation" to any surface turns this red while the
// filter still refuses.
#[test]
fn no_surface_claims_image_generation() {
    let refuses = the_filter_refuses_image_models();
    assert!(
        refuses,
        "`is_selectable` no longer refuses image model ids; this guard's premise is \
         gone and the description decision needs re-deciding before it is relaxed"
    );
    for (surface, text) in [
        (
            "extensions/antigravity/extension.toml",
            manifest_description(),
        ),
        ("extensions/catalog.toml", catalog_description()),
    ] {
        assert!(
            !text.to_ascii_lowercase().contains("image"),
            "{surface} still advertises {text:?}, while `is_selectable` refuses every \
             model whose id contains `image`: a claim the code will not honour"
        );
    }
    // The provider doc lists the manifest verbatim, so the same text check covers
    // it; asserted separately so the failure names the file.
    let doc = read("docs/providers/antigravity.md");
    assert!(
        !doc.to_ascii_lowercase()
            .contains("including image generation"),
        "docs/providers/antigravity.md still carries the image-generation claim"
    );
}

// Verifies: gh #279 - the doc does not claim that `vendor-event` carries image
// generation results.
//
// A second, subtler instance of the same drift, found while reading the doc: it
// asserted that the reserved vendor event exists *for* image results. The
// extension's only `VendorEvent` is the thinking signature, and `StreamEvent` has
// no image variant, so that sentence described a channel nobody uses that way.
#[test]
fn the_vendor_event_doc_matches_what_is_emitted() {
    let doc = read("docs/providers/antigravity.md");
    let stream = read("extensions/antigravity/src/stream.rs");
    let emits_image_event = stream.contains("VendorEvent")
        && stream
            .lines()
            .filter(|line| line.contains("VendorEvent"))
            .any(|line| line.to_ascii_lowercase().contains("image"));
    assert!(
        !emits_image_event,
        "the extension now emits an image vendor event; the doc and the description \
         need revisiting together"
    );
    assert!(
        !doc.contains("Carries image generation results"),
        "the vendor-event section still says it carries image generation results, \
         but the only vendor event emitted is the thinking signature"
    );
}

// Verifies: gh #279 - the pair is on the record: `is_selectable` refuses an image
// id **and** nothing advertises one.
//
// The brief asks for both halves pinned "so the pair cannot disagree silently
// again". The filter row is separate from the description row on purpose: if a
// future cycle enables image models, the filter row is the one that must change
// first and deliberately.
#[test]
fn the_filter_refuses_and_the_description_is_silent() {
    let models = read("extensions/antigravity/src/models.rs");
    // The refusal is unconditional on `image`, before the accept at the end.
    let image_check = models
        .find(r#"lower.contains("image")"#)
        .expect("the image refusal is present");
    let accept = models
        .rfind("    true\n}")
        .expect("the accept arm is present");
    assert!(
        image_check < accept,
        "the image refusal must run before the accepting `true`"
    );
    // And the behavioural half: a row that actually calls the filter, so this
    // guard is not satisfied by a source string. It lives beside the filter in
    // `models.rs` (`cargo nextest -p antigravity` runs it).
    assert!(
        models.contains("fn an_image_model_is_not_selectable"),
        "a row pins that the filter actually refuses an image id, not just that the \
         source contains the check"
    );
    // ...and that it still accepts the families the extension advertises, or a
    // filter that refused everything would satisfy the row above.
    assert!(
        models.contains("fn the_offered_families_are_still_selectable"),
        "and a row pins that the offered families stay selectable"
    );
}
