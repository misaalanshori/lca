//! gh #410 - a key hint names the key the interface will accept.
//!
//! The defect was a documented promise the code did not keep. `key_text(action)`
//! said its hints "cannot lie when the interface is rebound" and built a **fresh
//! default** `KeybindingsManager` on every call, so all fourteen UI sites printed
//! the default key after a rebind: the label lied in exactly the situation the
//! comment promised it could not.
//!
//! This guard pins the two things that let it happen and would let it happen
//! again:
//!
//! 1. **No helper named for live hints may resolve from the default table.** The
//!    free function is `default_key_text` now, and says so; a future
//!    `key_text`-shaped helper is the 15th call site waiting to be written.
//! 2. **Every hint site goes through the live manager.** A `display_keys` call on
//!    a manager the site does not own, or a quiet return to the default table, is
//!    the same bug with a different spelling.
//!
//! The behaviour rows are in `crates/lca-ui/src/key_hint_tests.rs` (rebound across
//! three render surfaces, multiple-bindings and chord form, unbound renders
//! nothing, the relief repaint after `/reload`, and the dim/muted/plain-theme
//! formatting). This is the NFR-24 pin that the mechanism stays wired.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failure.

fn read(path: &str) -> String {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(root.join(path)).unwrap_or_else(|err| panic!("read {path}: {err}"))
}

// Verifies: gh #410 design point 2 - the free function no longer promises live
// labels, and the only default-table entry point is named for what it is.
#[test]
fn the_default_table_helper_is_named_as_such() {
    let keybindings = read("crates/lca-tui/src/engine/keybindings.rs");
    assert!(
        keybindings.contains("pub fn default_key_text("),
        "the default-table helper is `default_key_text`"
    );
    // The lying name must be gone: a `pub fn key_text(` that resolves defaults is
    // precisely the shape this cycle removed.
    assert!(
        !keybindings.contains("pub fn key_text("),
        "no free `key_text` may exist: its doc promised live labels and its body \
         returned defaults"
    );
    // And the live path exists on the manager, which is the thing that knows the
    // user's overrides.
    for fact in [
        "pub fn display_keys(",
        "pub fn display_keys_capitalized(",
        "pub fn primary_key(",
    ] {
        assert!(
            keybindings.contains(fact),
            "the manager exposes {fact} so a hint can resolve live"
        );
    }
    // The doc on `default_key_text` records why it is not for hints.
    assert!(
        keybindings.contains("gh #410"),
        "the rename records the defect that caused it"
    );
}

// Verifies: gh #410 - no UI site resolves a hint from the default table.
//
// This is the guard that catches the 15th call site. A row that merely checked
// "keybindings.rs has display_keys" would pass while a new render path quietly
// rebuilt a default manager.
#[test]
fn no_ui_site_resolves_a_hint_from_the_default_table() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let ui = root.join("crates/lca-ui/src");
    let mut offenders = Vec::new();
    let mut stack = vec![ui];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
                continue;
            }
            // The rows themselves name the default manager on purpose (they are
            // asserting the baseline), so only non-test sources are scanned.
            let name = path.to_string_lossy();
            if name.ends_with("_tests.rs") || name.ends_with("/tests.rs") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            for (index, line) in text.lines().enumerate() {
                if line.contains("default_key_text(") {
                    offenders.push(format!("{name}:{}: {line}", index + 1));
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "a hint must never resolve from the default table (use the live manager):\n{}",
        offenders.join("\n")
    );
}

// Verifies: gh #410 - the hint formatter always takes the manager.
//
// The one-form rule is the structural half of the fix: a `render_key_hint` that
// could be called without a manager would be the lying helper again, one layer
// down. The signature is asserted rather than a call site, because a new call
// site is exactly what the signature is meant to constrain.
#[test]
fn the_hint_formatter_has_no_default_using_form() {
    let render = read("crates/lca-ui/src/transcript/render.rs");
    let signature = render
        .split("pub(crate) fn render_key_hint(")
        .nth(1)
        .expect("the formatter exists");
    let signature = signature.split('{').next().unwrap_or(signature);
    assert!(
        signature.contains("keybindings: &lca_tui::engine::keybindings::KeybindingsManager"),
        "the formatter takes the live manager: {signature}"
    );
    // Exactly one definition, so no overload can be added that skips the manager.
    assert_eq!(
        render.matches("fn render_key_hint(").count(),
        1,
        "one form only: a variant without a manager is the defect again"
    );
    // And the unbound case returns empty rather than a blank key press.
    let body = render
        .split("pub(crate) fn render_key_hint(")
        .nth(1)
        .expect("formatter");
    let body = body.split("\n}\n").next().unwrap_or(body);
    assert!(
        body.contains("is_empty()"),
        "an unbound action renders nothing: {body}"
    );
}

// Verifies: gh #410 design point 1 - the manager is threaded, not rebuilt per
// render (the #389 footer lesson: no per-frame config read).
#[test]
fn the_transcript_owns_the_manager_instead_of_building_one_per_render() {
    let transcript = read("crates/lca-ui/src/transcript/mod.rs");
    assert!(
        transcript.contains(
            "keybindings: std::sync::Arc<lca_tui::engine::keybindings::KeybindingsManager>"
        ),
        "the transcript holds the live manager"
    );
    assert!(
        transcript.contains("pub fn set_keybindings("),
        "and `/reload` can install a new one"
    );
    // The render path passes the field, never a fresh default.
    assert!(
        transcript.contains("self.keybindings.as_ref(),"),
        "the render path threads the owned manager"
    );
    assert!(
        !transcript.contains("KeybindingsManager::new()"),
        "the transcript never constructs a default manager in its render path"
    );
    // The cache cannot outlive a rebind: the epoch is part of the key.
    assert!(
        transcript.contains("cache_epoch"),
        "a cached frame must not survive a rebind"
    );
}

// Verifies: gh #410 design point 3 - the rebind reaches already-rendered hints.
//
// Checked structurally *and* semantically: `set_keybindings` drops the cache, or
// the acceptance row would pass only while the cache happened to be cold.
#[test]
fn installing_bindings_invalidates_the_cached_frames() {
    let transcript = read("crates/lca-ui/src/transcript/mod.rs");
    let body = transcript
        .split("pub fn set_keybindings(")
        .nth(1)
        .expect("the setter exists");
    let body = body.split("\n    }\n").next().unwrap_or(body);
    assert!(
        body.contains("invalidate_cache()"),
        "a rebind drops every cached frame: {body}"
    );
}
