//! The class fix for gh #272 and #273: a drift guard over the config
//! vocabulary mirrors.
//!
//! The two issues were the same failure: several hand-maintained lists
//! describe one vocabulary, and nothing checks that they agree.
//!
//! | mirror | what it decides |
//! |---|---|
//! | the `Config` struct fields | what the loader can hold |
//! | `Config::resolved()` | what `lca config` / `/settings` prints (FR-CFG-2) |
//! | `KNOWN_KEYS` | what a `LCA_*` environment variable can set |
//! | `TABLE_KEYS` | which subtrees the unknown-key walk leaves to their parser |
//! | `docs/configuration.md`'s key table | what users are told exists |
//!
//! A key added to one and not the others is exactly what shipped twice. These
//! rows make the next such omission fail a test instead of shipping.
//!
//! The `Config` struct fields are read from this crate's own source: they are
//! private, so no integration test can enumerate them any other way. Parsing
//! source in a test is unusual, and it is the only way to pin "every field
//! has a row" without exporting the struct's guts.
//!
//! Verifies: gh #272/#273 acceptance 5.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use lca_config::{Config, KNOWN_KEYS, TABLE_KEYS};

/// The repository root, from this crate's manifest directory.
fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

/// The `Config` struct's field names, read from `lca-config/src/lib.rs`.
///
/// Field declarations are the lines inside `pub struct Config { ... }` that
/// look like `name: Type,`. Nested braces are not used there, so a brace
/// depth scan is enough to find the end of the struct.
fn config_struct_fields() -> BTreeSet<String> {
    let source = std::fs::read_to_string(repo().join("crates/lca-config/src/lib.rs"))
        .expect("read lca-config/src/lib.rs");
    let start = source
        .find("pub struct Config {")
        .expect("the Config struct is declared");
    let mut depth = 0usize;
    let mut fields = BTreeSet::new();
    for line in source[start..].lines() {
        let trimmed = line.trim();
        if depth == 1
            && let Some((name, rest)) = trimmed.split_once(':')
            && !name.starts_with("//")
            && !name.starts_with("///")
            && rest.trim_end().ends_with(',')
            && name.chars().all(|c| c.is_ascii_lowercase() || c == '_')
        {
            fields.insert(name.to_string());
        }
        depth += line.matches('{').count();
        depth = depth.saturating_sub(line.matches('}').count());
        if depth == 0 && !fields.is_empty() {
            break;
        }
    }
    assert!(
        !fields.is_empty(),
        "no Config fields parsed from the source"
    );
    fields
}

/// The key table rows in `docs/configuration.md`: `| \`key\` | ...`.
fn documented_keys() -> BTreeSet<String> {
    let doc = std::fs::read_to_string(repo().join("docs/configuration.md"))
        .expect("read docs/configuration.md");
    let mut keys = BTreeSet::new();
    for line in doc.lines() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with("| `") {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("| `")
            && let Some((key, _)) = rest.split_once('`')
            && key
                .chars()
                .all(|c| c.is_ascii_lowercase() || c == '.' || c == '_')
        {
            keys.insert(key.to_string());
        }
    }
    assert!(
        keys.len() > 50,
        "the key table parsed suspiciously few rows ({}): did the table format change?",
        keys.len()
    );
    keys
}

/// What a `Config` field backs: one or more candidate keys, or nothing when
/// the field is bookkeeping rather than a setting.
#[derive(Debug, PartialEq, Eq)]
enum Backing {
    /// Candidate keys the field might implement (the doc-match check picks).
    Keys(Vec<String>),
    /// Not a setting: `sources` records provenance and `substituted`
    /// records which keys resolved a `{env:}`/`{file:}` token. Neither has
    /// a configuration key, so neither can appear in `resolved()` as a row
    /// of its own (the *fact* that a key substituted is covered by
    /// `Config::substituted`).
    Bookkeeping,
    /// No mapping was declared for this field: the guard must fail.
    Unmapped,
}

/// Which key a `Config` field backs. Most are the field name with `_` for `.`,
/// but the mapping is not mechanical, so it is named.
fn field_backing(field: &str) -> Backing {
    // Fields with no configuration key: bookkeeping, or covered by a
    // key whose name differs (`thinking_visibility` is `ui.thinking`).
    let aliases: &[(&str, &[&str])] = &[
        ("provider", &["provider"]),
        ("model", &["model"]),
        ("ui_color", &["ui.color"]),
        ("ui_theme", &["ui.theme"]),
        ("thinking", &["thinking"]),
        ("shell_tool", &["shell.tool"]),
        ("shell_path", &["shell.path"]),
        ("shell_command_prefix", &["shell.command_prefix"]),
        ("permissions_mode", &["permissions.mode"]),
        ("thinking_visibility", &["ui.thinking"]),
        ("skills_inject_matched", &["skills.inject_matched"]),
        ("permissions_proposals", &["permissions.proposals"]),
        ("models_enabled", &["models.enabled"]),
        ("models_thinking_levels", &["models.thinking_levels"]),
        ("thinking_budgets", &["thinking.budgets"]),
        ("extensions_catalogs", &["extensions.catalogs"]),
        // Gh #412: a table key like the others above - user-shaped children, a
        // key's own parser, no single-value environment form.
        ("personas", &["personas"]),
        ("sources", &[]),
        ("substituted", &[]),
    ];
    if let Some((_, keys)) = aliases.iter().find(|(name, _)| *name == field) {
        return if keys.is_empty() {
            Backing::Bookkeeping
        } else {
            Backing::Keys(keys.iter().map(|key| key.to_string()).collect())
        };
    }
    // The mechanical case: `compaction_threshold` -> `compaction.threshold`,
    // choosing the split that is a documented key. A field whose key is not
    // mechanical must be added to `aliases` above, which is the point of this
    // row: a new field fails here until someone names its key.
    let mut candidates = Vec::new();
    let parts: Vec<&str> = field.split('_').collect();
    for cut in 1..parts.len() {
        let left = parts[..cut].join("_");
        let right = parts[cut..].join("_");
        candidates.push(format!("{left}.{right}"));
    }
    if candidates.is_empty() {
        // A single-word field with no alias: nothing to try, so the guard
        // must refuse rather than accept it silently.
        return Backing::Unmapped;
    }
    Backing::Keys(candidates)
}

// Verifies: gh #272 - every `Config` field is reported by `resolved()`, so
// `lca config` cannot under-report a setting again. A new field with no row
// (and no alias naming its key) turns this red.
//
// The two steps are deliberately separate: `docs/configuration.md` decides
// *which* key a field backs (the mechanical `_`-to-`.` split is ambiguous -
// `ui_fullscreen_scrollbar` could be `ui.fullscreen_scrollbar` or
// `ui_fullscreen.scrollbar`, and only the reference settles it), and then
// `resolved()` must actually report that key. Accepting "documented" as a
// pass would let a dropped `resolved()` row slip through, which is precisely
// the gh #272 defect - so it is not accepted.
#[test]
fn every_config_field_is_reported_by_the_inspector() {
    let config = Config::defaults();
    let reported: BTreeSet<&str> = config.resolved().map(|(key, _, _)| key).collect();
    let documented = documented_keys();
    let mut problems = Vec::new();
    for field in config_struct_fields() {
        let candidates = match field_backing(&field) {
            Backing::Bookkeeping => continue,
            Backing::Unmapped => {
                problems.push(format!(
                    "{field}: no key mapping declared (add it to `aliases`)"
                ));
                continue;
            }
            Backing::Keys(candidates) => candidates,
        };
        // Step 1: the reference says which candidate is the real key.
        let real: Vec<&String> = candidates
            .iter()
            .filter(|key| documented.contains(*key) || TABLE_KEYS.contains(&key.as_str()))
            .collect();
        if real.is_empty() {
            problems.push(format!(
                "{field}: none of {candidates:?} is a documented key"
            ));
            continue;
        }
        // Step 2: the inspector must report it.
        for key in real {
            if !reported.contains(key.as_str()) {
                problems.push(format!(
                    "{field}: `{key}` is documented but missing from resolved()"
                ));
            }
        }
    }
    assert!(
        problems.is_empty(),
        "the inspector under-reports these settings (gh #272): {problems:#?}"
    );
}

// Verifies: gh #272 - every documented key is in `KNOWN_KEYS` or is a table
// key. This is the direction that was broken: a key could be documented,
// parse in a file, and be invisible to the environment loop.
#[test]
fn every_documented_scalar_key_can_be_set_from_the_environment() {
    let mut missing = Vec::new();
    for key in documented_keys() {
        if TABLE_KEYS.contains(&key.as_str()) {
            // A table has no single-value form: documented as file-only.
            continue;
        }
        if !KNOWN_KEYS.contains(&key.as_str()) {
            missing.push(key);
        }
    }
    assert!(
        missing.is_empty(),
        "documented keys missing from KNOWN_KEYS (their LCA_* variable is inert): {missing:?}"
    );
}

// Verifies: gh #272 - and the other direction: every entry in `KNOWN_KEYS` is
// documented, so the environment surface and the reference cannot diverge by
// a key that works but is undocumented.
#[test]
fn every_environment_key_is_documented() {
    let documented = documented_keys();
    let mut undocumented = Vec::new();
    for key in KNOWN_KEYS {
        if !documented.contains(*key) {
            undocumented.push(*key);
        }
    }
    assert!(
        undocumented.is_empty(),
        "KNOWN_KEYS entries with no row in docs/configuration.md: {undocumented:?}"
    );
}

// Verifies: gh #272 - every key the inspector prints is a documented key.
// A row for a key the reference does not name is either a doc gap or a
// leftover; both need a human decision, so this refuses to guess.
#[test]
fn every_reported_key_is_documented() {
    let documented = documented_keys();
    let config = Config::defaults();
    let mut orphaned = Vec::new();
    for (key, _, _) in config.resolved() {
        if !documented.contains(key) && !TABLE_KEYS.contains(&key) {
            orphaned.push(key);
        }
    }
    assert!(
        orphaned.is_empty(),
        "resolved() rows with no row in docs/configuration.md: {orphaned:?}"
    );
}

// Verifies: gh #273 - the table-key list is what the unknown-key walk trusts,
// so every entry must be a real documented key and must not also be a scalar
// in `KNOWN_KEYS` (a scalar there would make the walk skip its children and
// skip validating a scalar, which is the bug shape the walk exists to stop).
#[test]
fn the_table_key_list_matches_the_documented_tables() {
    let documented = documented_keys();
    for key in TABLE_KEYS {
        assert!(
            documented.contains(*key),
            "`{key}` is in TABLE_KEYS but has no row in docs/configuration.md"
        );
    }
    // The documented reference marks its tables with "table" in the type
    // column; every such row must be a TABLE_KEYS entry.
    let doc = std::fs::read_to_string(repo().join("docs/configuration.md")).expect("read doc");
    let mut table_rows = Vec::new();
    for line in doc.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("| `")
            && let Some((key, after)) = rest.split_once('`')
            && let Some(cells) = after.split('|').nth(1)
            && cells.trim().starts_with("table")
        {
            table_rows.push(key.to_string());
        }
    }
    assert!(
        !table_rows.is_empty(),
        "no table-typed rows parsed from the reference"
    );
    for key in table_rows {
        assert!(
            TABLE_KEYS.contains(&key.as_str()),
            "docs/configuration.md calls `{key}` a table, but TABLE_KEYS does not list it"
        );
    }
}

// Verifies: gh #272 - `resolved()` reports each key once. The dump builds a
// `BTreeMap`, so a duplicate would silently shadow a row instead of showing.
#[test]
fn no_key_is_reported_twice() {
    let config = Config::defaults();
    let keys: Vec<&str> = config.resolved().map(|(key, _, _)| key).collect();
    let unique: BTreeSet<&str> = keys.iter().copied().collect();
    assert_eq!(keys.len(), unique.len(), "duplicate row in resolved()");
}

// Verifies: gh #273 - the vocabulary is closed: every documented key is
// either a scalar `KNOWN_KEYS` entry or a `TABLE_KEYS` table. A documented key
// that is neither would be refused by the file walk, which is the exact
// failure mode the pair of issues had to avoid by landing in order.
#[test]
fn no_documented_key_falls_outside_the_vocabulary() {
    for key in documented_keys() {
        assert!(
            KNOWN_KEYS.contains(&key.as_str()) || TABLE_KEYS.contains(&key.as_str()),
            "`{key}` is documented but is neither a KNOWN_KEYS entry nor a TABLE_KEYS entry"
        );
    }
}

/// The keys the `/settings` selector (`lca-cli`'s `tui/display.rs`) offers
/// rows for. It is a fourth mirror of the vocabulary: a row whose key is not
/// in `resolved()` renders the `<unset> [default]` fallback, so a typo'd key
/// looks like an unset setting instead of an error.
fn selector_row_keys() -> BTreeSet<String> {
    let source = std::fs::read_to_string(repo().join("crates/lca-cli/src/tui/display.rs"))
        .expect("read display.rs");
    let mut keys = BTreeSet::new();
    for line in source.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with("row(") {
            continue;
        }
        // `row("Section", "key", &[...])`, single- or multi-line: take the
        // quoted string that is not the section label (sections have spaces
        // and capitals, keys never do).
        for (index, part) in trimmed.split('"').enumerate() {
            if index % 2 == 1
                && part
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c == '.' || c == '_')
                && part.contains('.')
            {
                keys.insert(part.to_string());
            }
        }
    }
    assert!(
        keys.len() > 20,
        "the selector row list parsed suspiciously few keys ({}): did `row(` change shape?",
        keys.len()
    );
    keys
}

// Verifies: gh #272 - every row `/settings` offers has a key `resolved()`
// reports, so the selector cannot display the `<unset>` fallback for a
// setting that exists (or, worse, for a key that does not exist at all under
// a name that looks plausible).
#[test]
fn every_settings_selector_row_has_a_resolved_key() {
    let config = Config::defaults();
    let reported: BTreeSet<&str> = config.resolved().map(|(key, _, _)| key).collect();
    let mut orphans = Vec::new();
    for key in selector_row_keys() {
        if !reported.contains(key.as_str()) {
            orphans.push(key);
        }
    }
    assert!(
        orphans.is_empty(),
        "`/settings` rows whose key resolved() does not report (they would render `<unset> [default]`): {orphans:?}"
    );
}
