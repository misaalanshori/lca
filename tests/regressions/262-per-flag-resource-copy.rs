//! Gh #262: `--theme` failures told the user about skill files.
//!
//! `check_resource_paths` served both flags but the failure sentence was a
//! hardcoded `"not a skill file or directory"`, so `lca --theme ./my-theme.json`
//! said the theme was not a skill file — the wrong noun for the flag the user
//! typed and the wrong extension for what they were writing.
//!
//! The behavioural rows live in `crates/lca-cli/src/invoke_tests.rs`. This file is
//! the NFR-24 guard, and it pins the **structure**, because a green suite would
//! not notice the copy drifting back:
//!
//! 1. **one table holds the expectation and the sentence together** — the defect
//!    existed because they were maintained in two places, so a fix that adds four
//!    `if`s has not fixed it;
//! 2. **the table covers every flag `prepare_resources` validates** — a fifth
//!    resource flag cannot be checked without an entry, because the guard reads
//!    the call sites and requires a matching `ResourceKind`;
//! 3. **no hardcoded noun survives in the function body** — the sentence comes
//!    from the table, not from a literal beside it;
//! 4. **an unlisted flag cannot inherit another flag's noun** — the fallback is
//!    extension-only, which is what stops a future flag silently describing
//!    itself as a theme.

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

// Verifies: gh #262 - the expectation and the message are one value.
//
// `ResourceKind` carries the flag, the extension, the noun and the home together,
// and `wrong_kind_message` reads the noun *from that record*. The defect was two
// separately maintained facts, so the guard is that there is nowhere else the
// noun could come from: the function body contains no `"skill` / `"theme`
// literal.
#[test]
fn the_message_comes_from_the_table_not_from_a_literal_beside_it() {
    let source = read("crates/lca-cli/src/invoke.rs");

    // The table exists, is the only one, and carries the sentence material.
    assert_eq!(
        source.matches("pub const RESOURCE_KINDS").count(),
        1,
        "exactly one resource table"
    );
    let table = source
        .find("pub const RESOURCE_KINDS")
        .map(|at| &source[at..])
        .expect("the table");
    let table = &table[..table.find("];").expect("its end")];
    for needle in [
        "flag: \"--skill\"",
        "flag: \"--theme\"",
        "noun: \"skill file\"",
        "noun: \"theme file\"",
        "extension: \"md\"",
        "extension: \"toml\"",
    ] {
        assert!(
            table.contains(needle),
            "the table carries `{needle}`: {table}"
        );
    }

    // The message builder reads the noun from the record rather than wording it
    // itself: no `"skill`/`"theme` literal appears in its body.
    let builder = function_body(&source, "fn wrong_kind_message(");
    for borrowed in ["\"skill", "\"theme"] {
        assert!(
            !builder.contains(borrowed),
            "`wrong_kind_message` does not hardcode `{borrowed}...`: {builder}"
        );
    }
    assert!(
        builder.contains("kind.noun"),
        "it prints the table's noun: {builder}"
    );

    // And the checker consults the table rather than an `if` per flag.
    let checker = function_body(&source, "pub fn check_resource_paths(");
    assert!(
        checker.contains("wrong_kind_message"),
        "the checker asks the table for its sentence: {checker}"
    );
    for borrowed in ["\"not a skill", "\"skill file"] {
        assert!(
            !checker.contains(borrowed),
            "the checker no longer carries the hardcoded copy (`{borrowed}`): {checker}"
        );
    }
}

// Verifies: gh #262 - every flag the product validates has a table entry.
//
// This is the "a fifth flag cannot inherit the wrong sentence" half. The guard
// reads the actual call sites in `prepare_resources` and requires each flag to
// appear in `RESOURCE_KINDS`, so adding `check_resource_paths("--prompt", ...)`
// without a record fails here.
#[test]
fn every_validated_flag_has_a_table_entry() {
    let source = read("crates/lca-cli/src/invoke.rs");

    // Every `check_resource_paths("--x"` call site in the file, not just in
    // `prepare_resources`: the call for `--theme` is wrapped across lines by
    // rustfmt, and a body-scoped scan found only the first flag. Scanning the
    // file is also the stronger claim - no call anywhere may bypass the table.
    let code: String = source
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    // `check_resource_paths(` followed by optional whitespace, then a quoted flag:
    // rustfmt wraps the `--theme` call onto its own line, so a scan that demanded
    // the quote immediately after the paren found only the first flag.
    let mut flags: Vec<String> = Vec::new();
    let mut rest = code.as_str();
    while let Some(at) = rest.find("check_resource_paths(") {
        let tail = &rest[at + "check_resource_paths(".len()..];
        let trimmed = tail.trim_start();
        if let Some(literal) = trimmed.strip_prefix('"') {
            let end = literal.find('"').unwrap_or(0);
            let flag = literal[..end].to_string();
            if !flag.is_empty() && !flags.contains(&flag) {
                flags.push(flag);
            }
        }
        rest = &tail[tail.len().min(1)..];
    }
    assert!(
        flags.len() >= 2,
        "the guard found the call sites: {flags:?}"
    );

    let table = source
        .find("pub const RESOURCE_KINDS")
        .map(|at| &source[at..])
        .expect("the table");
    let table = &table[..table.find("];").expect("its end")];
    for flag in &flags {
        assert!(
            table.contains(&format!("flag: \"{flag}\"")),
            "`{flag}` is validated and has a table entry, so its message is its \
             own: {table}"
        );
    }
}

// Verifies: gh #262 - an unlisted flag gets a generic sentence, never a
// borrowed noun.
//
// The fallback is the seam that makes the table safe to extend: a flag someone
// validates without adding a record describes itself by its extension rather
// than inheriting "theme file" from whichever record happened to be first.
#[test]
fn an_unlisted_flag_falls_back_to_the_extension_only_form() {
    let source = read("crates/lca-cli/src/invoke.rs");
    let builder = function_body(&source, "fn wrong_kind_message(");
    assert!(
        builder.contains("else {"),
        "there is an explicit unlisted-flag branch: {builder}"
    );
    assert!(
        builder.contains("file or directory"),
        "and it describes the shape generically: {builder}"
    );
    // The fallback must not mention a noun from the table.
    let fallback_at = builder.find("else {").expect("the branch");
    let fallback = &builder[fallback_at..];
    for borrowed in ["theme file", "skill file"] {
        assert!(
            !fallback.contains(borrowed),
            "the fallback does not borrow `{borrowed}`: {fallback}"
        );
    }
}

// Verifies: gh #262 - the sentence tells the user what to DO.
//
// `AGENTS.md`: "error copy tells the user what to DO". Here that is the
// extension to use and, for themes, the directory they belong in. Both come from
// the record, so a flag without a home simply omits that half.
#[test]
fn the_sentence_names_the_extension_and_the_home_when_there_is_one() {
    let source = read("crates/lca-cli/src/invoke.rs");
    let builder = function_body(&source, "fn wrong_kind_message(");
    assert!(
        builder.contains("expected `.{extension}`"),
        "the extension to use is in the sentence: {builder}"
    );
    assert!(
        builder.contains("kind.home"),
        "and the home comes from the record: {builder}"
    );
    let table = source
        .find("pub const RESOURCE_KINDS")
        .map(|at| &source[at..])
        .expect("the table");
    let table = &table[..table.find("];").expect("its end")];
    assert!(
        table.contains("home: Some(\"~/.lca/themes/\")"),
        "the theme record names where themes live: {table}"
    );
}
