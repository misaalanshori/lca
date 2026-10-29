//! Gh #333: a tab switch must not leave a transient overlay pinned over the next tab.
//!
//! The defect was a **hand-maintained list**: `Chat::close_pickers` named ten pickers,
//! and the overlays #227 added (`extension_hub`, `extension_manager`,
//! `install_consent`) were never added to it — so `/extensions` in Tab 1 stayed pinned
//! over Tab 2's transcript. `mcp_manager` (#418 Phase G) had already leaked the same way
//! by the time the issue was filed, which is the argument for the structure over the
//! three-line fix.
//!
//! The behavioural rows live in `crates/lca-ui/src/overlay_registry_tests.rs`. This file
//! is the NFR-24 guard, and its job is the one a unit test cannot do: **enumerate the
//! overlay set from the source** and fail when `Chat` or `UiState` grows an overlay-shaped
//! field that the registry does not know about. A test that listed three names would
//! re-create the hand-maintained list in a second place and pass happily while a fourth
//! leaked.
//!
//! Three checks:
//!
//! 1. every overlay-shaped field declared on `Chat` is in `CHAT_OVERLAYS`;
//! 2. every one in `UiState` is in `WORLD_OVERLAYS`;
//! 3. the macro-generated clearing in `overlay_registry.rs` names exactly what the lists
//!    name — because Rust macros cannot iterate a `const`'s identifiers, the two are
//!    written separately and this row is what keeps them equal.

use std::collections::BTreeSet;
use std::path::PathBuf;

/// A source file from the workspace root.
fn source(rel: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {path:?}: {err}"))
}

/// The declared field names of `pub struct <name> { … }`, as `(name, type)`.
///
/// Brace-matched rather than regexed, so a field whose type spans lines still yields the
/// right name and a nested generic's `,` does not split it. Only `pub` fields: a private
/// field cannot be an overlay the interface shows.
fn declared_fields(text: &str, struct_name: &str) -> Vec<(String, String)> {
    let start = text
        .find(&format!("pub struct {struct_name} {{"))
        .unwrap_or_else(|| panic!("`{struct_name}` is declared"));
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    let mut seen = false;
    let mut end = start;
    for (offset, byte) in bytes[start..].iter().enumerate() {
        match byte {
            b'{' => {
                depth += 1;
                seen = true;
            }
            b'}' => {
                depth -= 1;
                if seen && depth == 0 {
                    end = start + offset;
                    break;
                }
            }
            _ => {}
        }
    }
    let body = &text[start..end];
    let mut fields = Vec::new();
    for line in body.lines() {
        let trimmed = line.trim();
        // A `pub name: Type,` line, at any visibility. Stripping the visibility
        // generically matters: matching only `pub ` silently *skipped* a `pub(crate)`
        // field, so this guard passed while a new overlay leaked - found by
        // mutation-testing the guard with exactly that field.
        let decl = trimmed
            .strip_prefix("pub(crate) ")
            .or_else(|| trimmed.strip_prefix("pub(super) "))
            .or_else(|| trimmed.strip_prefix("pub "));
        if let Some(rest) = decl
            && let Some((name, ty)) = rest.split_once(':')
        {
            fields.push((
                name.trim().to_string(),
                ty.trim().trim_end_matches(',').to_string(),
            ));
        }
    }
    fields
}

/// Whether a field's *type* makes it a transient overlay.
///
/// `Option<…>` is the shape; the exemptions below are the honest part, because several
/// `Option` fields are **not** overlays and must not be cleared on a switch:
///
/// * `submitted` / `submitted_queue` — a message already handed to the host;
/// * `turn_status` — a turn's live meter, which travels with the tab;
/// * `current_steer` — the steer queue, which travels with the tab;
/// * `scroll_drag` — a pointer gesture in flight, not a surface;
/// * `tooltip` / `hover_at` — hover chrome, cleared by the mouse paths;
/// * `shell` — a running `!` command, which is work in progress, not a question;
/// * the model caches and `theme_query` — long-lived state, not surfaces.
///
/// The exemptions are named rather than pattern-matched, so adding an overlay means
/// adding it to the registry (which the row then checks) and adding an *exempt* field
/// means writing down why.
const NON_OVERLAY_EXEMPTIONS: &[&str] = &[
    "turn_status",
    "current_steer",
    "submitted",
    "submitted_queue",
    "scroll_drag",
    "tooltip",
    "hover_at",
    "shell",
    "model_rows_cache",
    "model_refresh",
    "theme_query",
    "terminal_bg",
    "last_transcript_len",
    "jump_target",
    "last_escape",
    "picker_press",
    "turn_tab",
    "deferred_host_session",
    // The host's own modal (gh #124). **Exempt, and not by convenience**: the host's
    // worker blocks on `respond.recv()` (see `crates/lca-cli/src/tui/mod.rs`), so
    // dropping this field without answering would hang an extension thread forever.
    // A tab switch must *answer* it, not forget it - that is the host's lifecycle, and
    // it is a different question from "which tab is this surface on". Recorded here so
    // the exemption is a decision rather than an omission.
    "dialog",
    // Not surfaces: the loop's wake channel, a login URL already handed to the browser,
    // and a "copied" confirmation timestamp belonging to the previous step.
    "waker",
    "auth_url",
    "url_copied_at",
];

/// The overlay-shaped field names on one struct, minus the exemptions.
fn overlay_shaped(text: &str, struct_name: &str) -> BTreeSet<String> {
    declared_fields(text, struct_name)
        .into_iter()
        .filter(|(name, ty)| {
            ty.starts_with("Option<") && !NON_OVERLAY_EXEMPTIONS.contains(&name.as_str())
        })
        .map(|(name, _)| name)
        .collect()
}

/// The names a `close_overlays!` invocation clears, for the `$owner` given.
fn macro_names(text: &str, owner: &str) -> BTreeSet<String> {
    let marker = format!("close_overlays!(\n        {owner};");
    let start = text
        .find(&marker)
        .unwrap_or_else(|| panic!("a `close_overlays!` invocation for `{owner}` exists:\n{text}"));
    let body = &text[start + marker.len()..];
    let end = body
        .find(')')
        .unwrap_or_else(|| panic!("the invocation terminates"));
    body[..end]
        .split(',')
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .collect()
}

// Verifies: gh #333 design point 2 - **every overlay field on `Chat` is in the registry.**
//
// This is the row the brief asks for: driven by the set, not by three names. A fifth
// overlay added to `Chat` tomorrow fails *here* - with its own name in the message -
// rather than leaking onto the next tab and being discovered months later.
#[test]
fn every_overlay_field_on_chat_is_in_the_registry() {
    let chat = source("crates/lca-ui/src/chat.rs");
    let declared = overlay_shaped(&chat, "Chat");
    let registered: BTreeSet<String> = lca_ui::overlay_registry::CHAT_OVERLAYS
        .iter()
        .map(|name| (*name).to_string())
        .collect();
    let missing: Vec<&String> = declared.difference(&registered).collect();
    assert!(
        missing.is_empty(),
        "these overlay-shaped fields on `Chat` are not in `CHAT_OVERLAYS`, so a tab \
         switch leaks them: {missing:?}\nAdd them to the registry (and to the \
         `close_overlays!` invocation), or to `NON_OVERLAY_EXEMPTIONS` with a reason."
    );
    let stale: Vec<&String> = registered.difference(&declared).collect();
    assert!(
        stale.is_empty(),
        "`CHAT_OVERLAYS` names fields `Chat` does not declare (or that are exempt): \
         {stale:?}"
    );
}

// Verifies: gh #333 - the same question for the overlays that live on `UiState`.
//
// They are the same kind of thing (`permission`, `picker`, `login_waiting`, `wizard`…)
// and were the same kind of omission waiting to happen: `wizard` (#397) arrived after
// `close_pickers` was written.
#[test]
fn every_overlay_field_on_the_world_is_in_the_registry() {
    let state = source("crates/lca-ui/src/ui_state.rs");
    let declared = overlay_shaped(&state, "UiState");
    let registered: BTreeSet<String> = lca_ui::overlay_registry::WORLD_OVERLAYS
        .iter()
        .map(|name| (*name).to_string())
        .collect();
    let missing: Vec<&String> = declared.difference(&registered).collect();
    assert!(
        missing.is_empty(),
        "these overlay-shaped fields on `UiState` are not in `WORLD_OVERLAYS`: {missing:?}"
    );
    let stale: Vec<&String> = registered.difference(&declared).collect();
    assert!(
        stale.is_empty(),
        "`WORLD_OVERLAYS` names absent fields: {stale:?}"
    );
}

// Verifies: gh #333 design point 1 - the generated clearing names **exactly** what the
// registry lists.
//
// Rust macros cannot iterate a `const` slice's identifiers, so `overlay_registry.rs`
// necessarily names them twice: once as a list (which the rows above check against the
// source) and once in the `close_overlays!` invocation (which is what compiles). Two
// spellings that must agree is a drift risk, and this row is what makes it safe.
#[test]
fn the_generated_clearing_matches_the_registry_lists() {
    let registry = source("crates/lca-ui/src/overlay_registry.rs");
    let cleared_chat = macro_names(&registry, "chat");
    let listed_chat: BTreeSet<String> = lca_ui::overlay_registry::CHAT_OVERLAYS
        .iter()
        .map(|name| (*name).to_string())
        .collect();
    assert_eq!(
        cleared_chat, listed_chat,
        "the `close_overlays!(chat; …)` invocation and `CHAT_OVERLAYS` must name the same fields"
    );

    let cleared_world = macro_names(&registry, "world");
    let listed_world: BTreeSet<String> = lca_ui::overlay_registry::WORLD_OVERLAYS
        .iter()
        .map(|name| (*name).to_string())
        .collect();
    assert_eq!(
        cleared_world, listed_world,
        "the `close_overlays!(world; …)` invocation and `WORLD_OVERLAYS` must name the same fields"
    );
}

// Verifies: gh #333 - the **flags** that carry overlay meaning without being `Option`s
// are cleared too.
//
// `panel_open` and `modal_open` are booleans, so the type-shaped rule above cannot see
// them; they are set rather than generated. A row rather than a comment, because a flag
// that stops being cleared is exactly as invisible as the modal it represents.
#[test]
fn the_overlay_flags_are_cleared_with_the_options() {
    let registry = source("crates/lca-ui/src/overlay_registry.rs");
    for flag in ["world.panel_open = false;", "world.modal_open = false;"] {
        assert!(
            registry.contains(flag),
            "the {flag:?} flag is cleared with the overlays"
        );
    }
}
