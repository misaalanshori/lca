//! gh #332 — one owner for the key map. NFR-24's guard.
//!
//! The bug was that **one fact had two owners**: `Chat` held the key map and
//! `Chat::editor` held its own clone, and nothing kept them equal, so `/reload`
//! replaced one and left the other. Adding the missing call would fix that instance
//! and leave the seam — the next place to replace the map would forget again, exactly
//! as `/reload` did.
//!
//! Two properties close it:
//!
//! 1. **The field is not writable from outside the module.** It was `pub`; it is
//!    `pub(crate)` now, so a host embedder cannot assign it at all and must go through
//!    `Chat::set_keybindings`, which updates all three holders together. That is
//!    enforced by the compiler, and this row checks the modifier rather than trusting
//!    that nobody re-publicises it.
//! 2. **There is exactly one setter that touches all three.** A second function
//!    assigning the field would be a second way to desync, so the row asserts the set
//!    of functions that assign it is exactly `set_keybindings`.
//!
//! The behaviour rows live in `crates/lca-ui/src/chat_reload_keys_tests.rs` (7 rows,
//! five of which fail when the editor propagation is removed).

use std::path::Path;

fn read(path: &str) -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(root.join(path)).unwrap_or_else(|err| panic!("read {path}: {err}"))
}

// Verifies: gh #332 - the key map cannot be replaced from outside the crate.
//
// The original defect was a public field, which made "assign the map and forget the
// editor" *expressible*. Narrowing the field is the structural fix; a row that only
// drove `/reload` would pass just as well against the old public field plus the one
// missing call, and the seam would still be there.
#[test]
fn the_key_map_is_not_publicly_writable() {
    let source = read("crates/lca-ui/src/chat.rs");
    let field = source
        .lines()
        .find(|line| line.contains("keybindings: Arc<KeybindingsManager>,"))
        .expect("the field exists");
    assert!(
        !field.trim_start().starts_with("pub keybindings"),
        "the key map must not be a `pub` field: a public field makes \"replace the map \
         and leave the editor on the old one\" expressible, which is gh #332 itself. \
         Found: {field}"
    );
    assert!(
        field.trim_start().starts_with("pub(crate) keybindings"),
        "and it should be `pub(crate)` - readable across this crate, writable only here: \
         {field}"
    );
}

// Verifies: gh #332 - exactly one function assigns the key map.
//
// "One setter that owns both" is the brief's design call. A second assignment is a
// second way to desync, and it would be written by someone who did not know the
// history - which is precisely how `/reload` missed it.
#[test]
fn only_one_function_assigns_the_key_map() {
    let source = read("crates/lca-ui/src/chat.rs");
    let lines: Vec<&str> = source.lines().collect();
    let mut assigners: Vec<String> = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let compact: String = line.chars().filter(|c| !c.is_whitespace()).collect();
        // `self.keybindings = ...` - a write, not a comparison or a field read.
        if !compact.starts_with("self.keybindings=") {
            continue;
        }
        // The enclosing function, found by walking back to the nearest `fn`.
        let owner = lines[..index]
            .iter()
            .rposition(|l| {
                let t = l.trim_start();
                t.starts_with("fn ")
                    || t.starts_with("pub fn ")
                    || t.starts_with("pub(crate) fn ")
                    || t.starts_with("pub(super) fn ")
                    || t.starts_with("fn ")
            })
            .map(|at| lines[at].trim().to_string())
            .unwrap_or_else(|| "<unknown>".to_string());
        assigners.push(format!("{}: {owner}", index + 1));
    }
    assert_eq!(
        assigners.len(),
        1,
        "exactly one function may assign the key map (gh #332: a second assignment is a \
         second way to leave the editor stale). Found:\n{}",
        assigners.join("\n")
    );
    assert!(
        assigners[0].contains("set_keybindings"),
        "and it is the one setter that updates all three holders: {}",
        assigners[0]
    );
}

// Verifies: gh #332 - the setter updates every holder, not just the field.
//
// The setter's whole value is that its body cannot be half-applied. If a future edit
// dropped the editor line the field would still agree with the transcript and the
// editor would silently stall - the original bug, one refactor later.
#[test]
fn the_setter_updates_the_field_the_editor_and_the_hints() {
    let source = read("crates/lca-ui/src/chat.rs");
    let start = source
        .find("pub fn set_keybindings(")
        .expect("the setter exists");
    let body = &source[start..];
    let body = &body[..body.find("\n    }\n").unwrap_or(body.len())];
    for (what, needle) in [
        (
            "the transcript's hint manager",
            "self.transcript.set_keybindings(",
        ),
        ("the prompt editor", "self.editor.set_keybindings("),
        ("the field itself", "self.keybindings = keybindings"),
    ] {
        assert!(
            body.contains(needle),
            "`set_keybindings` must update {what} - omitting it is how gh #332 happened: \
             {body}"
        );
    }
}

// Verifies: gh #332 - the editor resolves bindings live, so there is no stale cache.
//
// The brief's fifth point: "if the editor caches parsed bindings, that cache is part of
// the seam". It does not - `handle_key` clones the manager per keystroke and asks
// `matches(data, action)` - so replacing the map is sufficient and the two vacuous rows
// in the behaviour file are not hiding a cache. Pinned so a future cache is noticed as
// a seam rather than a performance win.
#[test]
fn the_editor_holds_no_parsed_binding_cache() {
    let editor = read("crates/lca-tui/src/widgets/editor.rs");
    // The only field holding bindings.
    let fields: Vec<&str> = editor
        .lines()
        .filter(|line| line.contains("keybindings"))
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect();
    assert!(
        fields
            .iter()
            .any(|line| line.contains("keybindings: Arc<KeybindingsManager>")),
        "the editor holds the manager itself: {fields:?}"
    );
    for line in &fields {
        assert!(
            !line.contains("HashMap")
                && !line.contains("accelerator")
                && !line.contains("BTreeMap"),
            "no parsed-binding cache lives beside the manager (a cache would need \
             invalidating on reload, which is a second seam): {line}"
        );
    }
    // And the lookup is a live query on the manager, not a precomputed match.
    let start = editor
        .find("pub fn handle_key(")
        .expect("the editor handles keys");
    let body = &editor[start..];
    let body = &body[..body.find("\n    }\n").unwrap_or(body.len())];
    assert!(
        body.contains("kb.matches(") || body.contains("keybindings.matches("),
        "the editor asks the manager at keystroke time: {}",
        &body[..body.len().min(400)]
    );
}
