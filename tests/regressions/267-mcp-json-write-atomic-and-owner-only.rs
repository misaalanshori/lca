//! Gh #267: `mcp.json` was written in place, so an interrupted write corrupted a
//! file holding server configs, environment variables and authentication headers —
//! and it landed with the process umask's permissions instead of `0600`.
//!
//! The behavioural rows live in `crates/lca-mcp/tests/config_writes.rs`: the
//! failed-write-leaves-the-old-file-intact property (by freezing the directory),
//! the owner-only mode, and the "restrict before publish" ordering. This file is
//! the NFR-24 guard, and it pins what those rows cannot see from the outside:
//!
//! 1. **there is exactly one atomic-write path in `lca-mcp`** — a second
//!    in-place `fs::write` of a config would be invisible to a row that only
//!    exercises the helper;
//! 2. **the restriction happens between the write and the rename** — the shape,
//!    not just the resulting mode, because a chmod after the rename passes the
//!    mode assertion while leaving the world-readable window open;
//! 3. **the Windows path is real code, not a doc comment** — this was the one
//!    genuine gap the earlier fix left: the doc claimed an owner-only DACL on
//!    Windows and no such call existed;
//! 4. **the confirmation prints the parsed name** — the defect's shape (echoing
//!    the argument) could recur in any command that reports its input back.
//!
//! # Why the "server name with a space" row the brief asked for does not exist
//!
//! `lca_mcp::is_valid_server_name` accepts only ASCII alphanumerics, `_` and `-`,
//! so a spaced name is rejected at parse time and a fixture using one would test
//! nothing. The substitute is a server whose name **is** a level keyword: `direct`
//! is a legal name and a legal level, so `exposure direct hidden` only resolves if
//! the parsed field is used.

use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(relative: &str) -> String {
    let path = repo_root().join(relative);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{} is readable: {error}", path.display()))
}

/// The body of one function, found by brace matching from its signature.
fn function_body<'a>(source: &'a str, signature: &str) -> &'a str {
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("{signature} exists"));
    let bytes = source.as_bytes();
    let mut depth = 0usize;
    let mut seen = false;
    let mut i = start;
    while i < bytes.len() {
        match bytes[i] {
            b'{' => {
                depth += 1;
                seen = true;
            }
            b'}' => {
                depth -= 1;
                if seen && depth == 0 {
                    return &source[start..=i];
                }
            }
            _ => {}
        }
        i += 1;
    }
    panic!("{signature} has a balanced body");
}

// Verifies: gh #267 - `write_json_file` writes a sibling temp file, restricts it,
// then renames. This is the ordering that makes an interrupted write harmless and
// keeps the file from ever being world-readable.
#[test]
fn the_json_write_restricts_the_temp_file_between_write_and_rename() {
    let source = read("crates/lca-mcp/src/config.rs");
    let body = function_body(&source, "fn write_json_file(");

    let write_at = body
        .find("std::fs::write(&temp")
        .unwrap_or_else(|| panic!("the write goes to the temp file: {body}"));
    let restrict_at = body
        .find("set_permissions(&temp")
        .or_else(|| body.find("set_owner_only(&temp"))
        .unwrap_or_else(|| panic!("the temp file is restricted: {body}"));
    let rename_at = body
        .find("std::fs::rename(&temp")
        .unwrap_or_else(|| panic!("the temp file is renamed over the target: {body}"));

    assert!(
        write_at < restrict_at,
        "the file exists before it is restricted (otherwise there is nothing to chmod)"
    );
    assert!(
        restrict_at < rename_at,
        "restriction happens BEFORE the rename - a chmod after it leaves a \
         world-readable window: {body}"
    );
    // And the target itself is never written in place: the only `fs::write` in
    // the body takes `&temp`.
    assert!(
        !body.contains("std::fs::write(path"),
        "the target is never opened for writing directly: {body}"
    );
}

// Verifies: gh #267 - exactly one atomic-write path exists in `lca-mcp`.
//
// The sweep the brief asked for: a second in-place config write elsewhere in the
// crate would keep the defect alive on a different file while every `mcp.json`
// row stayed green.
#[test]
fn no_other_in_place_write_of_a_file_target_exists_in_lca_mcp() {
    let mut offenders: Vec<String> = Vec::new();
    let mut scanned = 0usize;
    for entry in walk(&repo_root().join("crates/lca-mcp/src")) {
        let text = std::fs::read_to_string(&entry).unwrap_or_default();
        scanned += 1;
        for (number, line) in text.lines().enumerate() {
            let code = line.trim_start();
            if code.starts_with("//") {
                continue;
            }
            // A write to something that is not a clearly-temporary path.
            let Some(at) = code.find("std::fs::write(") else {
                continue;
            };
            let rest = &code[at..];
            let temporary = rest.contains("&temp")
                || rest.contains("tmp")
                || rest.contains("staged")
                || rest.contains("scratch");
            if !temporary {
                offenders.push(format!(
                    "{}:{}: {}",
                    entry.display(),
                    number + 1,
                    code.trim_end()
                ));
            }
        }
    }
    assert!(scanned > 5, "the sweep covered the crate: {scanned} files");
    assert!(
        offenders.is_empty(),
        "every file write in `lca-mcp` targets a temp path, so no config is \
         truncated in place: {offenders:#?}"
    );
}

// Verifies: gh #267 - the Windows restriction is real code, not a claim.
//
// The gap the earlier fix left: `write_json_file`'s doc said "an owner-only DACL
// on Windows" while the body had only a `#[cfg(unix)]` branch, so on Windows the
// file kept whatever ACL the profile directory gave it. A doc comment is not a
// permission.
#[test]
fn the_windows_owner_only_call_exists_behind_a_windows_cfg() {
    let source = read("crates/lca-mcp/src/config.rs");
    let body = function_body(&source, "fn write_json_file(");

    let windows_at = body
        .find("#[cfg(windows)]")
        .unwrap_or_else(|| panic!("there is a Windows branch, not just a doc comment: {body}"));
    assert!(
        body[windows_at..].contains("set_owner_only"),
        "and it applies the owner-only ACL: {body}"
    );
    // The helper it calls is the credential store's own, so the two owner-only
    // files in the product cannot drift apart.
    let tools = read("crates/lca-tools/src/capabilities/mod.rs");
    assert!(
        tools.contains("pub fn set_owner_only"),
        "lca-tools re-exports the one DACL implementation: {tools}"
    );
    // The credential store still uses it, so this is a shared helper and not a
    // function added for `lca-mcp` alone.
    let store = read("crates/lca-tools/src/capabilities/store.rs");
    assert!(
        store.contains("windows_acl::set_owner_only"),
        "the credential store is the other caller: {store}"
    );
}

// Verifies: gh #267 - the confirmation reports the parsed name.
//
// The original replied with the whole argument in backticks, so the reply named a
// server that does not exist. Checked at the source as well as behaviourally: the
// format string must use the parsed binding, not `target`.
#[test]
fn the_confirmation_formats_the_parsed_name_not_the_argument() {
    let source = read("crates/lca-mcp/src/manager.rs");
    let body = function_body(&source, "fn set_exposure(");

    assert!(
        body.contains("let (name, next) = match target.rsplit_once(' ')"),
        "the name is parsed out of the argument: {body}"
    );
    let confirmation = body
        .rfind("format!(")
        .map(|at| &body[at..])
        .unwrap_or_else(|| panic!("the confirmation is formatted: {body}"));
    assert!(
        confirmation.contains("{name}") || confirmation.contains("name"),
        "the format uses the parsed `name` binding: {confirmation}"
    );
    assert!(
        !confirmation.contains("{target}") && !confirmation.contains("target}\""),
        "and not the raw argument: {confirmation}"
    );
}

// Verifies: gh #267 - a server name cannot contain a space, which is why the
// brief's fixture shape was substituted.
#[test]
fn a_spaced_server_name_is_rejected_so_the_fixture_uses_a_level_keyword() {
    assert!(!lca_mcp::is_valid_server_name("filesystem direct"));
    assert!(lca_mcp::is_valid_server_name("direct"));
    let rows = read("crates/lca-mcp/tests/config_writes.rs");
    assert!(
        rows.contains("a name that collides with a level word"),
        "the behavioural rows say why the substitute is the right one"
    );
}

/// Every `.rs` file under `dir`, recursively.
fn walk(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk(&path));
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
    out
}
