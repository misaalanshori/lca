//! The key vocabulary is one list per family, and the lists agree.
//!
//! Gh #545 phase 2 replaced `apply_toml_layer`'s 612-line closure with a parser
//! per key family. The defect the closure carried was not its length: the same
//! key was known in three places - a `match` arm that typed the value, a string
//! in a 65-entry apply loop, and `is_documented_key`'s table - and the file's own
//! comments said so twice ("a key missing from either loads as silence", gh
//! #272). A split that keeps three lists only moves the drift.
//!
//! So the split's contract is these two rows:
//!
//! 1. **Every key a family applies has exactly one parser arm in that family, and
//!    no parser owns a key its family does not list.** The family's `KEYS` *is*
//!    the apply list (the dispatch table in `toml_layer/mod.rs` holds the
//!    families, not the keys), so "in the loop but not in the match" is now
//!    impossible rather than documented.
//! 2. **The lists agree**: the families' keys are exactly `KNOWN_KEYS` plus the
//!    table-shaped keys in `TABLE_KEYS`, minus `permissions.proposals` - which
//!    `Config::load` reads beside the project-trust check it belongs to.
//!
//! Both read the source textually. `lca-config` has no `module_sources` walk
//! (that lives in the root `lca-regressions` crate, which this crate must not
//! depend on), so the rows glob `src/toml_layer/*.rs` themselves - which is the
//! same thing a walk would do, and keeps working if a family splits again.
//!
//! Verifies: gh #545 phase 2 acceptance 1-2 (one owner per key, one list).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use lca_config::{KNOWN_KEYS, TABLE_KEYS};

fn src_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/toml_layer")
}

/// Every `*.rs` file in the module directory, with its name and text.
fn family_sources() -> Vec<(String, String)> {
    let dir = src_dir();
    let mut out = Vec::new();
    for entry in
        std::fs::read_dir(&dir).unwrap_or_else(|err| panic!("read {}: {err}", dir.display()))
    {
        let path = entry.expect("a directory entry").path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .expect("a utf-8 file name")
            .to_string();
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("read {}: {err}", path.display()));
        out.push((name, text));
    }
    assert!(
        out.len() > 2,
        "the module directory holds only {} file(s): is this still a directory?",
        out.len()
    );
    out.sort();
    out
}

/// The strings in the array literal a `needle` introduces: `"= &[" for a
/// constant, `"for key in ["` for a loop. The needle is matched, not a bare
/// `[`, because `const KEYS: &[&str]` carries a `[` of its own that would
/// otherwise end the scan on an empty slice.
fn array_at(text: &str, needle: &str) -> Vec<String> {
    let i = text
        .find(needle)
        .unwrap_or_else(|| panic!("the source names `{needle}`"));
    let start = i + needle.len() - 1;
    assert_eq!(&text[start..start + 1], "[", "the needle ends at the `[`");
    let mut depth = 0usize;
    for (offset, ch) in text[start..].char_indices() {
        match ch {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    return quoted_strings(&text[start..start + offset + 1]);
                }
            }
            _ => {}
        }
    }
    panic!("the array at `{needle}` never closes")
}

/// Every `"double-quoted"` run in a slice of source.
fn quoted_strings(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find('"') {
        rest = &rest[open + 1..];
        let Some(close) = rest.find('"') else { break };
        out.push(rest[..close].to_string());
        rest = &rest[close + 1..];
    }
    out
}

/// The keys a family's `parse` function names in its `match` arms.
///
/// An arm's pattern is the run of lines at the arm indent that starts with a
/// quote: rustfmt puts one key per line in a multi-key arm (`"a"` then
/// `| "b" => {`), and every body line is one level deeper. The `_ =>` fallback
/// starts with `_`, not a quote, so it is skipped - it is the mis-route
/// refusal, not an owner.
fn parser_keys(text: &str) -> Vec<String> {
    let body = text
        .split("fn parse(")
        .nth(1)
        .unwrap_or_else(|| panic!("a family file declares `fn parse`"));
    let match_line = body
        .lines()
        .find(|line| line.trim_start().starts_with("match key {"))
        .expect("the family parser matches on `key`");
    let arm_indent = match_line.len() - match_line.trim_start().len() + 4;

    let mut out = Vec::new();
    let mut in_pattern = false;
    for line in body.lines() {
        let indent = line.len() - line.trim_start().len();
        if indent == arm_indent && line.trim_start().starts_with('"') {
            in_pattern = true;
        }
        if in_pattern {
            out.extend(quoted_strings(line));
            if line.contains("=>") {
                in_pattern = false;
            }
        }
    }
    out
}

/// The `KEYS` a family file declares, keyed by the family's file name.
fn family_keys() -> BTreeMap<String, Vec<String>> {
    let mut out = BTreeMap::new();
    for (name, text) in family_sources() {
        if name == "mod.rs" {
            continue;
        }
        if !text.contains("const KEYS") {
            continue;
        }
        out.insert(name, array_at(&text, "const KEYS: &[&str] = &["));
    }
    assert!(
        out.len() >= 5,
        "only {} family files declare `KEYS`: the split lost a family",
        out.len()
    );
    out
}

// Verifies: gh #545 phase 2 - one owner per key, and the owner is the one that
// types it. The two ways a key can go silent are a key in the apply list whose
// `match` arm is missing (it would fall to `_ =>` and be refused as a stranger)
// and an arm that types a key its family never routes (dead code that a future
// reader trusts). Both are red here.
#[test]
fn every_known_key_has_exactly_one_parser_and_no_parser_owns_a_stranger() {
    let families = family_keys();
    let mut owners: BTreeMap<String, String> = BTreeMap::new();
    let mut problems = Vec::new();

    for (file, text) in family_sources() {
        if file == "mod.rs" {
            continue;
        }
        let listed: BTreeSet<String> = families
            .get(&file)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .collect();
        let parsed: Vec<String> = parser_keys(&text);
        let parsed_set: BTreeSet<String> = parsed.iter().cloned().collect();

        // A family with a parser must declare its keys: that list is the apply
        // list, so a parser with no `KEYS` applies nothing and is dead.
        if text.contains("fn parse(") && listed.is_empty() {
            problems.push(format!("{file}: declares `parse` but no `KEYS`"));
        }
        for key in &listed {
            if !parsed_set.contains(key) {
                problems.push(format!(
                    "{file}: routes `{key}` but no arm types it (it would be \
                     refused as a stranger)"
                ));
            }
            if let Some(other) = owners.insert(key.clone(), file.clone()) {
                problems.push(format!("`{key}` is owned by both {other} and {file}"));
            }
        }
        for key in &parsed_set {
            if !listed.contains(key) {
                problems.push(format!(
                    "{file}: types `{key}` but does not list it (nothing routes it)"
                ));
            }
        }
        // A duplicate arm inside one file would shadow the second: `match` takes
        // the first, so the later one is dead and the two can drift apart.
        if parsed.len() != parsed_set.len() {
            let mut dupes: Vec<&String> = Vec::new();
            let mut seen = BTreeSet::new();
            for key in &parsed {
                if !seen.insert(key) {
                    dupes.push(key);
                }
            }
            problems.push(format!("{file}: two arms own the same key: {dupes:?}"));
        }
    }

    assert!(
        problems.is_empty(),
        "the family parsers and their key lists disagree:\n{}",
        problems.join("\n")
    );
}

// Verifies: gh #545 phase 2 / gh #272 - the `KEY` list, the documented vocabulary
// (`KNOWN_KEYS` + `TABLE_KEYS`, which is what `is_documented_key` consults), and
// `Config::load`'s own default-source loop all name one set.
//
// The shape differences are named rather than absorbed:
//  - `permissions.proposals` is a documented table key that this layer must NOT
//    route: `Config::load` reads it beside the project-trust check it belongs to,
//    so routing it here would split that rule. It is the one expected absence.
//  - `extensions.catalogs` is in both `KNOWN_KEYS` and `TABLE_KEYS`, because it
//    has a table form *and* is named in `KNOWN_KEYS` for a different reason; the
//    union (not the concatenation) is what the families must match.
#[test]
fn the_three_key_lists_agree() {
    let families = family_keys();
    let routed: BTreeSet<String> = families.values().flatten().cloned().collect();
    // Table keys that `Config::load` reads itself rather than through this layer.
    let routed_elsewhere: BTreeSet<String> = ["permissions.proposals".to_string()].into();
    let documented: BTreeSet<String> = KNOWN_KEYS
        .iter()
        .chain(TABLE_KEYS.iter())
        .map(|key| key.to_string())
        .collect();

    assert_eq!(
        routed,
        documented.difference(&routed_elsewhere).cloned().collect(),
        "the families' keys and the documented vocabulary disagree: \
         routed-only={:?} documented-only={:?}",
        routed.difference(&documented).collect::<Vec<_>>(),
        documented.difference(&routed).collect::<Vec<_>>()
    );
    assert!(
        documented.iter().all(|key| !key.is_empty()),
        "a key in the vocabulary is an empty string"
    );
    assert_eq!(
        routed.len(),
        families.values().map(Vec::len).sum::<usize>(),
        "a key is routed by two families (the counts differ)"
    );

    // `Config::load`'s default-source loop is the second allowlist the file's
    // comment called out. It seeds every documented key with `MergeSource::Default`
    // so `lca config` can report a value no layer set, which is why it lists the
    // table keys too.
    let lib = std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/lib.rs"))
        .expect("read src/lib.rs");
    let defaults: BTreeSet<String> = array_at(
        &lib[lib.find("pub fn load").expect("`load` exists")..],
        "for key in [",
    )
    .into_iter()
    .collect();
    // It seeds the keys this layer routes, which is why `permissions.proposals`
    // (read in `load` itself) is absent from both lists - the same expected
    // absence, seen from the second list.
    assert_eq!(
        defaults,
        routed,
        "`Config::load`'s default-source loop and the families' keys disagree: \
         loop-only={:?} families-only={:?}",
        defaults.difference(&routed).collect::<Vec<_>>(),
        routed.difference(&defaults).collect::<Vec<_>>()
    );
}
