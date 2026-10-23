//! The plan-mode badge (gh #409's first slice), tested against the extension
//! that owns it.
//!
//! # What this proves
//!
//! #409 is three features and carries an `abi-0.7-breakage` label. This slice
//! takes the badge alone, over the regions that already exist, because that is
//! the smallest thing that proves the *pattern*: extension-held state, a pull of
//! `render(region)`, one line in a width-limited region, no WIT change. The todo
//! widget and the fleet capsules stay open.
//!
//! # Where the state lives
//!
//! In the extension, per ADR-0013's taxonomy. A `plan_mode` flag in the host
//! would be the core growing a feature that belongs to an extension, and the
//! `ui` world exists precisely so an extension can hold state and be pulled.
//!
//! That has one consequence worth stating, because it decides the design: the
//! host's `RegionRenderer` calls `render(region)` **per frame** (a pull model -
//! `wit/world-ui.wit` says so and `crates/lca-cli/src/tui/mod.rs`'s
//! `region_renderer` implements exactly that). So the badge needs no push at
//! all: the extension mutates its own state on a key and the next pull sees it.
//! `lca:host/ui.request-redraw()` remains the documented push for a guest whose
//! async work lands outside a frame, and these rows pin that a *state change* is
//! what alters the render - never a per-frame request.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use lca_protocol::Widget;
use lca_ui::ext_widgets::WidgetCtx;

/// Render a tree the way the host does: through the real `widget_lines`, at the
/// real region width. A string-length guess here would miss wrapping and the
/// theme's own reshaping.
fn render(nodes: &[Widget], width: usize, region: &str) -> Vec<String> {
    let theme = lca_ui::Theme::default();
    let offsets = std::collections::HashMap::new();
    let ctx = WidgetCtx {
        theme: &theme,
        width,
        region,
        offsets: &offsets,
    };
    lca_ui::widget_lines(nodes, &ctx)
}

/// One badge's rendered text.
fn badge_text(badge: &ui_example::PlanBadge) -> String {
    render(&ui_example::plan_badge_nodes(badge), 80, "status-line").join("\n")
}

// Verifies: gh #409 - the badge names the mode while plan mode is active, and
// nothing at all while it is not. "Nothing" matters as much as the badge: the
// status line belongs to the host and other extensions, so an inactive plan mode
// must contribute no segment rather than an empty one that still costs a
// separator.
#[test]
fn the_badge_appears_only_while_plan_mode_is_active() {
    let off = ui_example::PlanBadge {
        active: false,
        step: None,
    };
    let on = ui_example::PlanBadge {
        active: true,
        step: None,
    };
    assert!(
        badge_text(&off).trim().is_empty(),
        "an inactive badge renders nothing: {:?}",
        badge_text(&off)
    );
    assert!(
        badge_text(&on).to_lowercase().contains("plan"),
        "an active badge names the mode: {:?}",
        badge_text(&on)
    );
}

// Verifies: gh #409 design point 2 - **the badge names the current step when the
// extension has one**, in `n/m` form, so a user in a five-step plan can see
// where they are without opening anything. The step is optional because the
// mode can be entered before any plan is parsed.
#[test]
fn the_badge_shows_the_current_step_when_there_is_one() {
    let with_step = ui_example::PlanBadge {
        active: true,
        step: Some((2, 5)),
    };
    let text = badge_text(&with_step);
    assert!(text.contains("2/5"), "the step is shown as n/m: {text:?}");

    let without = ui_example::PlanBadge {
        active: true,
        step: None,
    };
    let text = badge_text(&without);
    assert!(
        !text.contains('/'),
        "no step, no half-formed fraction: {text:?}"
    );
}

// Verifies: NFR-26 / the region's contract - **one line, inside 80 columns**.
// `status-line` is a single-line region; a badge that wrapped would displace the
// footer, and the dock's layout arithmetic has no room for that. Widths of 80
// and 40 are both checked, because a narrow terminal is where a long badge
// breaks first.
#[test]
fn the_badge_is_one_line_at_eighty_and_forty_columns() {
    let wide = ui_example::PlanBadge {
        active: true,
        step: Some((12, 34)),
    };
    for width in [80usize, 40] {
        let lines = render(&ui_example::plan_badge_nodes(&wide), width, "status-line");
        assert_eq!(lines.len(), 1, "one line at {width} columns: {lines:?}");
        // The badge is ASCII by construction (`plan 12/34`), so a character
        // count is the display width here. A CJK or emoji badge would need
        // `lca_tui`'s width table, which this crate does not depend on - say so
        // rather than silently measuring the wrong thing.
        let measured = lines[0].chars().count();
        assert!(
            measured <= width,
            "the badge fits {width} columns (got {measured}): {:?}",
            lines[0]
        );
    }
}

// Verifies: gh #409 design point 4 and NFR-28 - **legible without colour**. A
// plain theme strips styling, so the cue has to be in the words: a badge that
// said only `\u{23f8}` in accent would reduce to unlabelled punctuation on a
// plain terminal, which is the failure NFR-28 exists to prevent.
#[test]
fn the_plain_theme_still_says_plan_in_words() {
    let badge = ui_example::PlanBadge {
        active: true,
        step: Some((1, 3)),
    };
    let plain = ui_example::plan_badge_nodes(&badge)
        .iter()
        .filter_map(|node| match node {
            Widget::Text { content, .. } | Widget::StyledText { content, .. } => {
                Some(content.as_str())
            }
            _ => None,
        })
        .collect::<String>();
    assert!(
        plain.to_lowercase().contains("plan"),
        "the words carry the meaning, not the colour: {plain:?}"
    );
    assert!(
        plain.contains("1/3"),
        "and the step survives a plain theme too: {plain:?}"
    );
}

// Verifies: gh #409 design point 3 - **the redraw is on change only**. The host
// pulls `render()` per frame, so the badge needs no push at all; what must never
// happen is the reverse - a badge that requests a redraw per frame, which is the
// #389 footer lesson and #369's. The row is structural because the extension has
// no redraw counter to count: `request_redraw()` is a host import with no
// observing implementation on this side of the boundary.
//
// What is testable here, and is the property that matters: **the extension never
// calls the push import at all**. The badge is state the host pulls per frame, so
// there is nothing to notify - and a `request_redraw()` anywhere in this source
// would be the #389 footer bug rebuilt (one notification per change is harmless;
// the habit is what slides into per-frame). Asserted on the source because the
// import has no host-side observer to count against.
#[test]
fn the_extension_never_requests_a_redraw() {
    let source = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/lib.rs"))
        .expect("the extension's source is readable");
    assert!(
        !source.contains("request_redraw"),
        "the badge is pulled per frame, so nothing calls the push import"
    );
}

// Verifies: gh #409 design point 3's other half - rendering twice with unchanged
// state produces identical bytes, so the host's per-frame pull costs nothing
// through this extension. The pair with the row above is the complete rule: no
// push, and a repeat pull is free.
#[test]
fn rendering_an_unchanged_badge_is_idempotent() {
    let badge = ui_example::PlanBadge {
        active: true,
        step: Some((2, 4)),
    };
    let first = badge_text(&badge);
    let second = badge_text(&badge);
    assert_eq!(
        first, second,
        "the same state renders the same bytes, so a per-frame pull costs nothing"
    );
}

// Verifies: gh #409 - **a state change is what alters the render**. This is the
// other half of the redraw rule: the badge follows the extension's state exactly,
// so the host's per-frame pull is sufficient and no notification is needed. A
// badge that cached its first render would fail here.
#[test]
fn a_state_change_alters_the_next_render() {
    let off = ui_example::PlanBadge {
        active: false,
        step: None,
    };
    let on = ui_example::PlanBadge {
        active: true,
        step: Some((1, 2)),
    };
    assert_ne!(
        badge_text(&off),
        badge_text(&on),
        "toggling plan mode changes what the next pull sees"
    );
}

// Verifies: gh #409 - **removing the extension removes the badge**, which is the
// host's `ui_regions` filter doing its job: an extension that does not declare
// `status-line` is never asked. Asserted on the extension's own region list, so
// this is the extension half of that contract; the host half is `region_renderer`
// (`crates/lca-cli`), which filters on exactly this list.
#[test]
fn the_badge_lives_in_the_status_line_region_only() {
    assert!(
        ui_example::REGIONS.contains(&"status-line"),
        "the extension declares status-line, so the host will ask for it"
    );
    // The badge must not leak into the other regions: `render_region` builds a
    // different tree per region, and the status badge belongs to one.
    let badge = ui_example::PlanBadge {
        active: true,
        step: Some((1, 2)),
    };
    let footer = render(
        &ui_example::render_region("footer", None).expect("footer renders"),
        80,
        "footer",
    )
    .join("\n");
    assert!(
        !footer.contains("1/2"),
        "the footer does not carry the badge: {footer:?}"
    );
    assert!(!badge_text(&badge).is_empty(), "and the status line does");
}

// Verifies: gh #409 end to end through the native handle - **the toggle is what
// changes the badge, and the badge lives in a region the host pulls**. This is
// the whole lifecycle in one row: `render` shows no badge, a `p` in the panel
// turns plan mode on, the next `render("status-line")` shows it, and a second `p`
// turns it back off.
//
// `p` is handled in the panel rather than the status line because the host makes
// `footer` and `status-line` **display-only** (`chat_mouse.rs`: their rows pack
// native and extension statuses together, so a cell cannot name one owner). The
// badge is therefore rendered where the contract allows it and toggled where
// input is actually delivered - a fact worth this row existing, because it is the
// one place this slice's shape differs from the "one segment, one line" picture.
#[test]
fn the_panel_toggles_plan_mode_and_the_status_line_shows_it() {
    use lca_ext_abi::ExtensionDispatch as _;

    let cap = sandbox("plan-toggle");
    let handle = ui_example::UiExample::new(cap);

    let status = |handle: &ui_example::UiExample| -> String {
        handle
            .render("status-line")
            .expect("renders")
            .map(|tree| render(&tree.nodes, 80, "status-line").join("\n"))
            .unwrap_or_default()
    };

    let before = status(&handle);
    assert!(
        !before.to_lowercase().contains("plan"),
        "plan mode starts off: {before:?}"
    );

    let effect = handle
        .on_ui_event("panel", &lca_protocol::UiInput::Key { key: "p".into() })
        .expect("the key is accepted");
    assert_eq!(
        effect,
        lca_protocol::UiEffect::None,
        "a mode toggle is not a host effect"
    );

    let during = status(&handle);
    assert!(
        during.to_lowercase().contains("plan"),
        "the next pull shows the badge: {during:?}"
    );

    handle
        .on_ui_event("panel", &lca_protocol::UiInput::Key { key: "p".into() })
        .expect("the key is accepted");
    let after = status(&handle);
    assert_eq!(
        before, after,
        "toggling off restores the prior status line exactly"
    );
}

/// A capability engine granting what the manifest declares, so the handle can be
/// built the way the host builds it. Duplicated from `panel.rs` deliberately: two
/// test files sharing a fixture module would need a `mod common` in a crate whose
/// test target is not a library, and this is fifteen lines.
fn sandbox(name: &str) -> std::sync::Arc<lca_tools::Capabilities> {
    use lca_permissions::{Decision, GrantStore, PermissionPrompt, ProposalDiff, ScopeRoots};
    use std::sync::{Arc, Mutex};

    struct Always;
    impl PermissionPrompt for Always {
        fn ask(&mut self, _action: &lca_permissions::Action) -> Decision {
            Decision::Always
        }
        fn review_proposals(&mut self, _diff: &ProposalDiff) -> bool {
            false
        }
    }

    let root = lca_testkit::scratch_path(&format!("plan-badge-{name}"));
    for dir in ["project", "private", "config", "data", "tmp"] {
        std::fs::create_dir_all(root.join(dir)).expect("mkdir");
    }
    Arc::new(lca_tools::Capabilities::new(
        "ui-example",
        ui_example::manifest_grants(),
        ScopeRoots {
            workspace: root.join("project"),
            private: root.join("private"),
            home_config: root.join("config"),
            temp: root.join("tmp"),
            state_dir: root.join("data"),
        },
        Arc::new(Mutex::new(Always)),
        Arc::new(Mutex::new(
            GrantStore::open(&root.join("grants.json")).expect("store"),
        )),
        root.join("project"),
        None,
    ))
}
