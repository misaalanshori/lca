//! gh #454 - the annotations everyone sends were silently dropped.
//!
//! `lca_protocol::ToolAnnotations` derived `Deserialize` with no `rename_all`, so
//! serde expected `read_only_hint` while every real payload says `readOnlyHint`:
//! the MCP spelling, and the spelling a WIT guest puts in
//! `extras["annotations"]`, which `ToolSpec::with_annotations_from_extras`
//! decodes. A camelCase payload therefore produced **all-`None`** - the hints
//! were dropped, not rejected.
//!
//! # Why the guard is behavioral
//!
//! Every existing row passed while every real payload decoded to nothing, because
//! the rows built the struct in Rust or wrote the snake_case spelling. A guard
//! that constructs the value it is checking cannot catch a decode bug. So the
//! rows that matter **start from JSON text**, and this file pins the property
//! that made the difference: the accepted spellings are both, the emitted
//! spelling is one, and the extras path - the one gh #303 depends on - carries a
//! guest's camelCase payload through.
//!
//! Three things must stay true:
//!
//! 1. **Accept both, emit one.** camelCase in (what the world sends) and
//!    snake_case in (what this tree's own fixtures wrote), camelCase out (the
//!    interop spelling).
//! 2. **An unknown key is ignored, deliberately.** A hint is display metadata the
//!    permission layer never decides on (ADR-0047), so a hint MCP adds later must
//!    not break decoding for a peer that sends it. `deny_unknown_fields` would.
//! 3. **No second silent-`None` type.** The sweep below is what makes this a class
//!    guard rather than a one-field fix.
//!
//! Behaviour rows live in `crates/lca-protocol/src/tool.rs::annotation_spelling_tests`
//! (six, all starting from JSON text) and the sanctioned re-pin in
//! `tests/regressions/303-shared-prompt-forwards-annotations.rs`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failure.

fn read(path: &str) -> String {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(root.join(path)).unwrap_or_else(|err| panic!("read {path}: {err}"))
}

// Verifies: gh #454 - the type accepts both spellings and emits camelCase.
//
// Stated against the *source* as well as the behaviour, because the behaviour
// rows live in another crate's unit tests and this is the row that would fail if
// `rename_all` were dropped while someone kept the aliases (which would accept
// both and emit snake_case - silently breaking every MCP peer that reads us).
#[test]
fn the_type_renames_on_the_wire_and_aliases_the_old_spelling() {
    let source = read("crates/lca-protocol/src/tool.rs");
    let start = source
        .find("pub struct ToolAnnotations {")
        .expect("the type exists");
    // The derive and its serde attributes sit above the struct.
    let header_start = source[..start]
        .rfind("#[derive(")
        .expect("the struct has a derive");
    let header = &source[header_start..start];
    assert!(
        header.contains(r#"rename_all = "camelCase""#),
        "emission is camelCase, the interop spelling: {header}"
    );
    for field in [
        "read_only_hint",
        "destructive_hint",
        "idempotent_hint",
        "open_world_hint",
    ] {
        let at = source
            .find(&format!("pub {field}:"))
            .expect("the field exists");
        let above = &source[header_start..at];
        let attrs = above.rsplit("#[serde(").next().unwrap_or("");
        assert!(
            attrs.contains(&format!(r#"alias = "{field}""#)),
            "`{field}` keeps accepting the snake_case spelling: {attrs}"
        );
    }
}

// Verifies: gh #454 design point 2 - **tolerance is the decision, and it is
// written down.** `deny_unknown_fields` on `ToolAnnotations` would make a future
// MCP hint break decoding, which is the failure this issue is about, inverted.
#[test]
fn unknown_keys_are_ignored_on_purpose() {
    let source = read("crates/lca-protocol/src/tool.rs");
    let start = source
        .find("pub struct ToolAnnotations {")
        .expect("the type exists");
    let header_start = source[..start]
        .rfind("#[derive(")
        .expect("the struct has a derive");
    let header = &source[header_start..start];
    assert!(
        !header.contains("deny_unknown_fields"),
        "a hint MCP adds later must not break decoding: {header}"
    );
    // And the reason is recorded where the decision lives.
    let doc_start = source[..header_start]
        .rfind("/// Hints about what a tool does")
        .expect("the type's doc comment");
    let doc = &source[doc_start..header_start];
    assert!(
        doc.contains("unknown key is ignored") || doc.contains("ignored rather than fatal"),
        "the tolerance is documented as a choice, not left implicit: {doc}"
    );
}

// Verifies: gh #454 - **the extras path**, which is the one that reads JSON a
// guest wrote. Its decode goes through the same `ToolAnnotations`, so the fix
// reaches it without touching the call site - and this row is what says so.
#[test]
fn the_extras_decode_is_the_same_type_not_a_second_parser() {
    let source = read("crates/lca-protocol/src/tool.rs");
    let start = source
        .find("pub fn with_annotations_from_extras")
        .expect("the extras path exists");
    let body = &source[start..];
    let body = &body[..body.find("\n    }\n").unwrap_or(body.len())];
    assert!(
        body.contains("serde_json::from_str::<ToolAnnotations>"),
        "the extras payload decodes into the renamed type: {body}"
    );
    assert!(
        !body.contains("readOnlyHint") && !body.contains("read_only_hint"),
        "and not by hand-reading the keys, which is how `lca-mcp` avoided the \
         bug and why nobody noticed: {body}"
    );
}

// Verifies: gh #454 design point 4 - **the class sweep.**
//
// The sweep looked for any other `lca_protocol` type that derives `Deserialize`,
// has multi-word fields, no `rename_all`/`alias`, and is fed JSON written by
// someone outside this tree. Findings:
///
/// - `ToolAnnotations` - the defect, fixed.
/// - `Usage` - crosses WIT **structurally** (`host_imports::from_wit_usage`), never
///   as JSON, so its snake_case field names are not a wire spelling at all.
/// - `ToolCall`, `ToolResult`, `ImageContent`, `NestedCallRecord`, and `Record`'s
///   payloads - JSON that this tree both writes and reads (session log, nested
///   transcript). Both sides are ours, so a rename is an internal refactor with no
///   interop risk; `Record` even carries an explicit `rename_all = "kebab-case"`
///   for its variant tags.
///
/// This row pins the discriminator rather than the list: an `annotations`-shaped
/// decode must go through a type whose rename matches the payload it is given.
#[test]
fn no_second_protocol_type_is_decoded_from_foreign_json_without_a_rename() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    // Every JSON decode into a protocol type, across the workspace.
    let mut foreign_decodes: Vec<String> = Vec::new();
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                out.push(path.clone());
                walk(&path, out);
            } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                out.push(path);
            }
        }
    }
    let mut files = Vec::new();
    walk(&root.join("crates"), &mut files);
    for path in files {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        for marker in ["from_str::<lca_protocol::", "from_value::<lca_protocol::"] {
            let mut rest = text.as_str();
            while let Some(index) = rest.find(marker) {
                rest = &rest[index + marker.len()..];
                if let Some(end) = rest.find('>') {
                    foreign_decodes.push(format!("{}:{}", &rest[..end], path.display()));
                }
            }
        }
    }
    // The only protocol types decoded from JSON text anywhere in the workspace
    // are the session record (both sides ours) and the annotations (fixed here).
    for entry in &foreign_decodes {
        let name = entry.split(':').next().unwrap_or("");
        assert!(
            name == "Record" || name == "ToolAnnotations",
            "a new protocol type is being JSON-decoded: {entry} - check its wire \
             spelling against whoever writes that JSON (gh #454's class)"
        );
    }
}

// Verifies: gh #454 - no new dependency; the fix is serde attributes.
#[test]
fn no_new_dependency_was_added() {
    let manifest = read("crates/lca-protocol/Cargo.toml");
    for unwanted in ["serde_with", "serde-aux", "rename"] {
        assert!(
            !manifest.contains(&format!("\n{unwanted} =")),
            "serde's own `rename_all`/`alias` are enough for four fields"
        );
    }
}
