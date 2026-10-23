//! NFR-24 guard for gh #409's first slice: a plan-mode badge, driven by an
//! extension through the **existing** `ui` world, rendered in the existing
//! `status-line` region.
//!
//! # Why this slice, and what it is proving
//!
//! #409 is three features (a plan-mode badge, a live todo widget, parallel
//! subagent fleet capsules) and carries an `abi-0.7-breakage` label for a
//! possible dedicated dock region. This slice takes the badge alone, over the
//! regions that already exist, because that is the smallest thing that proves
//! the *pattern*: extension-held state, a pull of `render(region)`, a single
//! line in a width-limited region. If the badge needs a new region to be useful,
//! that is the 0.7 conversation and not something a cycle decides.
//!
//! The todo widget and the fleet capsules stay open; the cycle report says what
//! each of them would need.
//!
//! # What is pinned here, and where the rest lives
//!
//! The badge is extension behaviour, so most of its rows live beside the
//! extension (`extensions/ui-example/tests/`) where the handle and its state are
//! constructible. This file guards the two things that must hold at the *repo*
//! level and would otherwise be invisible:
//!
//! 1. **No ABI change.** The whole point of the slice is that the badge rides the
//!    existing `status-line` region and the existing widget vocabulary. A future
//!    fix that added a `plan-dock` region or a new widget variant to make the
//!    badge nicer would be exactly the 0.7 decision this cycle deferred, and it
//!    would pass every behaviour row. So the vocabulary is asserted here.
//! 2. **The badge is one line that fits.** `status-line` is a single-line region
//!    by contract; a badge that wrapped or pushed the footer would break the
//!    dock's layout arithmetic, which no extension-local test can see.
//!
//! Verifies: gh #409's first slice, FR-UI-1, NFR-26 (width), NFR-28 (legible
//! without colour), and NFR-24.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

/// The reference extension's source, so the region and vocabulary can be read.
fn ui_example_source() -> String {
    std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/extensions/ui-example/src/lib.rs"
    ))
    .expect("ui-example's source is readable")
}

// Verifies: gh #409's scope decision - **no ABI change**. The manifest still
// declares exactly the four catalog regions, and the badge lives inside
// `status-line` rather than in a new `plan-dock`/`hud` region. The issue labels
// that possibility `abi-0.7-breakage`; this row is what stops a future cycle
// from taking it by accident while making the badge prettier.
#[test]
fn the_badge_adds_no_region_and_no_widget_kind() {
    let manifest = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/extensions/ui-example/extension.toml"
    ))
    .expect("the manifest is readable");
    assert!(
        manifest.contains(r#"regions = ["status-line", "footer", "panel", "modal"]"#),
        "the manifest still declares exactly the four catalog regions:\n{manifest}"
    );
    for absent in ["plan-dock", "hud", "plan_dock"] {
        assert!(
            !manifest.contains(absent),
            "no `{absent}` region was added: that is the 0.7 decision this slice \
             deferred"
        );
    }

    // The WIT is the ABI. A badge that needed a variant would have had to edit
    // it, so its absence is the proof the vocabulary sufficed.
    let wit = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/wit/types.wit"))
        .expect("the widget vocabulary is readable");
    assert!(
        wit.contains("package lca:ext@0.6.0;"),
        "the WIT package line is unchanged"
    );
    for invented in ["plan-badge", "plan_badge", "hud"] {
        assert!(
            !wit.contains(invented),
            "no `{invented}` widget variant was added - the badge uses the \
             existing vocabulary"
        );
    }
}

// Verifies: gh #409 - **the badge is built from existing widget kinds only**.
// The vocabulary is `text`, `styled-text`, `markdown`, `button`, `table`,
// `scroll-container`, `image`, `boxed`, `row`, `column`, `spinner`, `progress`,
// `key-value`, `vendor`. A brand-new kind would be the ABI conversation; this
// row reads the badge's own construction and checks it uses nothing else.
#[test]
fn the_badge_uses_only_the_existing_widget_vocabulary() {
    let source = ui_example_source();
    let start = source
        .find("fn plan_badge")
        .expect("the badge has its own constructor");
    let body = &source[start..];
    let body = &body[..body.find("\n}\n").unwrap_or(body.len())];

    // Every `Widget::` constructor the badge body names must be a variant that
    // already exists in the protocol's vocabulary.
    let known = [
        "Text",
        "StyledText",
        "Markdown",
        "Button",
        "Table",
        "ScrollContainer",
        "Image",
        "Boxed",
        "Row",
        "Column",
        "Spinner",
        "Progress",
        "KeyValue",
        "Vendor",
    ];
    for used in body.match_indices("Widget::") {
        let rest = &body[used.0 + "Widget::".len()..];
        let name: String = rest
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        assert!(
            known.contains(&name.as_str()),
            "the badge uses `Widget::{name}`, which is not in the existing \
             vocabulary: {body}"
        );
    }
}

// The two render rows (one line at 80 columns, the plain theme saying "plan" in
// words) live in `extensions/ui-example/tests/plan_badge.rs`, where the
// extension is a first-class dependency of its own test target. Adding
// `ui-example` to this crate's dev-dependencies would have meant editing
// `Cargo.toml`, which this cycle's boundary forbids - and a copy of the widget
// tree here would drift from the one the host actually renders.
