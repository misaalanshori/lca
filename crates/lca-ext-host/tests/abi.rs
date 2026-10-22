//! The ABI policy's machine-checkable half (docs/abi-versioning.md).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use lca_ext_abi::ABI_VERSION;
use lca_ext_host::{Manifest, SUPPORTED_ABI_WINDOW};

fn manifest_with_abi(abi: &str) -> Manifest {
    Manifest::parse(&format!(
        "name = \"window-test\"\nversion = \"1.0.0\"\nabi = \"{abi}\"\n\
         worlds = [\"tool\"]\ndescription = \"x\"\n"
    ))
    .expect("parses")
}

// Verifies: NFR-18 (the ABI line is `major.minor` semver - the shape
// the manifest and every window check parse) and NFR-19 (the host
// loads the current minor and the previous one, nothing else).
#[test]
fn the_window_accepts_the_current_and_previous_minor_only() {
    // The declared line is major.minor, exactly.
    let (major, minor) = ABI_VERSION
        .split_once('.')
        .expect("ABI_VERSION is major.minor");
    let major: u64 = major.parse().expect("major is a number");
    let minor: u64 = minor.parse().expect("minor is a number");
    assert!(
        SUPPORTED_ABI_WINDOW.contains('.'),
        "the window announces itself as a range: {SUPPORTED_ABI_WINDOW}"
    );

    // Current line: loads.
    assert!(
        manifest_with_abi(ABI_VERSION).abi_in_window(),
        "the current line {ABI_VERSION} must load"
    );

    // The previous minor: loads (the rebuild cycle abi-versioning
    // promises). During0.x the minor is the breaking position, so the
    // previous minor is the one before it.
    if minor > 0 {
        let previous = format!("{major}.{}", minor - 1);
        assert!(
            manifest_with_abi(&previous).abi_in_window(),
            "the previous line {previous} must load (NFR-19)"
        );
    }

    // The next minor (built against a newer host than this one):
    // refused - a new host line does not run on an old host.
    let next = format!("{major}.{}", minor + 1);
    assert!(
        !manifest_with_abi(&next).abi_in_window(),
        "the future line {next} must not load here"
    );

    // A different major: refused (NFR-18's semver meaning: a new major
    // is breaking, and this host is not it).
    assert!(
        !manifest_with_abi(&format!("{}.{}", major + 1, minor)).abi_in_window(),
        "a new major never loads on an older host"
    );
}

// Verifies: the versioning sync rule (ADR-0028's second annotation) makes a
// stale extension fail *legibly* at the manifest check, not at a link error:
// a line older than current-minus-one is refused, and the refusal names the
// accepted lines.
#[test]
fn a_stale_line_is_refused_legibly_at_the_manifest_check() {
    let (major, minor) = ABI_VERSION
        .split_once('.')
        .expect("ABI_VERSION is major.minor");
    let minor: u64 = minor.parse().expect("minor is a number");
    if minor < 2 {
        return;
    }
    let stale = format!("{major}.{}", minor - 2);
    let manifest = manifest_with_abi(&stale);
    assert!(
        !manifest.abi_in_window(),
        "{stale} is two lines back and must be refused"
    );

    // The refusal is the loader's typed window error, and its message names
    // the accepted range rather than leaving the user to guess.
    let err = lca_ext_host::LoadError::AbiUnsupported {
        declared: stale.clone(),
        window: SUPPORTED_ABI_WINDOW,
    };
    let text = err.to_string();
    assert!(text.contains(&stale), "names the declared line: {text}");
    assert!(
        text.contains("0.5") && text.contains("0.6"),
        "names the accepted lines: {text}"
    );
}

// Verifies: NFR-20's discipline (a change that removes or alters an
// export increments the major) rests on the changelog being written
// for the live line in the same change as the WIT edit
// (docs/abi-versioning.md's "Making a change"): the current version
// must appear in the ABI changelog or the entry is missing its own
// version bump.
#[test]
fn the_abi_changelog_carries_the_live_version() {
    let changelog = include_str!("../../../wit/CHANGELOG.md");
    assert!(
        changelog.contains(ABI_VERSION),
        "wit/CHANGELOG.md has no entry for the live ABI {ABI_VERSION}: \
         an ABI change without its changelog entry is incomplete \
         (docs/abi-versioning.md, NFR-18/NFR-20)"
    );
}

// ── gh #288: a typo'd world must fail loudly, not load as a no-op ──────────

/// A minimal valid manifest with the given `worlds` line.
fn manifest_with_worlds(worlds: &str) -> Result<Manifest, String> {
    Manifest::parse(&format!(
        "name = \"world-check\"\nversion = \"0.1.0\"\nabi = \"{ABI_VERSION}\"\n\
         worlds = {worlds}\ndescription = \"x\"\n"
    ))
    .map_err(|err| err.to_string())
}

// Verifies: gh #288 - the case the issue filed. `worlds = ["contxt-transform"]`
// used to install and load as an extension matching no world at all: a
// successful install, no error anywhere, and nothing working.
#[test]
fn a_typo_in_a_world_is_refused_with_a_suggestion() {
    let error =
        manifest_with_worlds(r#"["contxt-transform"]"#).expect_err("a typo'd world must not parse");
    assert!(
        error.contains("contxt-transform"),
        "the error names what the author wrote: {error}"
    );
    assert!(
        error.contains("did you mean `context-transform`?"),
        "the error suggests the world they meant: {error}"
    );
}

/// A typo'd `hooks-` suffix is refused too - the family is a fixed set, not a
/// prefix, so `hooks-tpyo` cannot slip through a `starts_with` check.
///
/// This is the shape a loose prefix rule would let in, and the reason the
/// vocabulary is a table rather than a prefix match.
#[test]
fn a_typo_in_a_hooks_world_is_refused() {
    // `hooks` itself is a real world (the six frozen hook points), so the family
    // is a *fixed set* - not a `hooks-` prefix. Both halves are pinned here,
    // because a `starts_with("hooks")` rule would accept every one of the typos
    // below while still passing a naive "real hooks parse" row.
    assert!(
        manifest_with_worlds(r#"["hooks"]"#).is_ok(),
        "`hooks` is a world in its own right"
    );
    for real in [
        "hooks-message",
        "hooks-tool-call",
        "hooks-tool-result",
        "hooks-stream",
        "hooks-settle",
        "hooks-compaction",
        "hooks-cache",
        "hooks-trust",
    ] {
        assert!(
            manifest_with_worlds(&format!("[{real:?}]")).is_ok(),
            "`{real}` is a real hooks world"
        );
    }
    for typo in [
        "hooks-tpyo",
        "hooks-tool-results",
        "hooks-tust",
        "hooks-messag",
        "hooksstream",
        "hook-stream",
    ] {
        let error = match manifest_with_worlds(&format!("[{typo:?}]")) {
            Err(error) => error,
            Ok(_) => panic!("`{typo}` must be refused, not accepted as a world"),
        };
        assert!(
            error.contains(typo),
            "the error names what was written (`{typo}`): {error}"
        );
    }
}

/// Every world in the vocabulary parses, generated from the vocabulary itself.
///
/// Generated rather than listed: a world added to `lca_ext_abi::WORLDS` is
/// covered here without anyone remembering to add it, which is the whole point of
/// having one table.
#[test]
fn every_vocabulary_world_parses() {
    for (name, _) in lca_ext_abi::WORLDS {
        let manifest = manifest_with_worlds(&format!("[{name:?}]"))
            .unwrap_or_else(|err| panic!("`{name}` is a real world but was refused: {err}"));
        assert_eq!(
            manifest.worlds,
            vec![(*name).to_string()],
            "`{name}` survives parsing verbatim"
        );
    }
    // ...and the dispatcher resolves every one of them to a `World`, so the
    // vocabulary cannot hold a name nothing dispatches on.
    for (name, world) in lca_ext_abi::WORLDS {
        assert_eq!(
            lca_ext_abi::parse_world(name),
            Some(*world),
            "`{name}` resolves to its own `World`"
        );
        assert_eq!(
            lca_ext_abi::world_name(*world),
            *name,
            "`{world:?}` renders back to its manifest spelling"
        );
    }
}

// Verifies: gh #288 design point 4 - a duplicate world is refused rather than
// silently deduplicated. The schema already says `uniqueItems`; the code was the
// half that disagreed.
#[test]
fn a_duplicate_world_is_refused() {
    let error = manifest_with_worlds(r#"["tool", "tool"]"#)
        .expect_err("a duplicate world must not parse silently");
    assert!(
        error.contains("more than once") && error.contains("tool"),
        "the error names the repeated world: {error}"
    );
}

// Verifies: gh #288 - a non-string entry used to be dropped by `filter_map`, so
// `["tool", 7]` parsed as `["tool"]` with no complaint.
#[test]
fn a_non_string_world_entry_is_refused() {
    let error = manifest_with_worlds(r#"["tool", 7]"#).expect_err("a number is not a world");
    assert!(
        error.contains("not a string"),
        "the error says what kind of problem it is: {error}"
    );
}

/// A data-only extension (`worlds = []` + resources) still loads, and an absent
/// `worlds` key still means the same thing.
#[test]
fn a_data_only_extension_still_loads() {
    let parsed = Manifest::parse(&format!(
        "name = \"data-only\"\nversion = \"0.1.0\"\nabi = \"{ABI_VERSION}\"\n\
         worlds = []\nresources = [\"skills\"]\ndescription = \"x\"\n"
    ))
    .expect("a data-only extension parses");
    assert!(parsed.worlds.is_empty());
    assert_eq!(parsed.resources, vec!["skills".to_string()]);
}

/// The vocabulary and the JSON schema's `enum` are the same set.
///
/// Two copies of one fact, so this row is the one that stops them drifting. The
/// schema is what an author's editor autocompletes from and what a strict
/// validator accepts; the code is what the loader dispatches on. A world landing
/// in one and not the other is exactly how gh #288 arose.
#[test]
fn the_schema_enum_matches_the_vocabulary() {
    let schema = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../schemas/extension-manifest.schema.json"
    ))
    .expect("the manifest schema is in the tree");
    let schema: serde_json::Value = serde_json::from_str(&schema).expect("valid JSON");
    let enum_names: Vec<String> = schema["properties"]["worlds"]["items"]["enum"]
        .as_array()
        .expect("`worlds.items.enum` is an array")
        .iter()
        .map(|value| {
            value
                .as_str()
                .expect("every enum entry is a string")
                .to_string()
        })
        .collect();
    let mut from_code: Vec<String> = lca_ext_abi::WORLDS
        .iter()
        .map(|(name, _)| (*name).to_string())
        .collect();
    let mut from_schema = enum_names;
    from_code.sort();
    from_schema.sort();
    assert_eq!(
        from_code, from_schema,
        "the schema's `worlds` enum and `lca_ext_abi::WORLDS` must be the same set"
    );
}

/// Every manifest shipped in the tree still parses.
///
/// The change tightens what is accepted, so a first-party extension with a
/// world outside the table would now refuse to load. This walks the real files
/// rather than a fixture, which is the only way to notice that.
#[test]
fn every_shipped_manifest_parses() {
    let root = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."));
    let extensions = root.join("extensions");
    let mut checked = 0;
    for entry in std::fs::read_dir(&extensions).expect("extensions/ is in the tree") {
        let path = entry.expect("readable dir entry").path();
        let manifest = path.join("extension.toml");
        if !manifest.is_file() {
            continue;
        }
        let text = std::fs::read_to_string(&manifest).expect("readable manifest");
        Manifest::parse(&text).unwrap_or_else(|err| {
            panic!(
                "{} no longer parses, so this change broke a shipped extension: {err}",
                manifest.display()
            )
        });
        checked += 1;
    }
    assert!(
        checked >= 10,
        "only {checked} shipped manifests found; the walk is probably wrong"
    );
}

/// The world table in `docs/extension-authoring.md` lists exactly the vocabulary.
///
/// The docs are the law, and a hand-maintained 16-row table is the kind of thing
/// that rots. Parsing both and comparing means a world added to `WORLDS` without
/// a doc line (or a doc line for a world that does not exist) fails here.
///
/// A regex over a markdown table is a shortcut with a known limit: the rows are
/// `` | `world` | description | `` and a reformat to different columns makes this
/// row fail rather than pass silently - the `at least 16` assertion is what
/// catches that.
#[test]
fn the_documented_worlds_match_the_vocabulary() {
    let doc = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/extension-authoring.md"
    ))
    .expect("the authoring guide is in the tree");
    let section = doc
        .split("### The `worlds` list")
        .nth(1)
        .expect("the worlds section exists");
    let mut documented: Vec<String> = Vec::new();
    for line in section.lines() {
        let Some(rest) = line.trim().strip_prefix("| `") else {
            continue;
        };
        let Some((name, _)) = rest.split_once('`') else {
            continue;
        };
        if name.chars().all(|c| c.is_ascii_lowercase() || c == '-') {
            documented.push(name.to_string());
        }
    }
    assert!(
        documented.len() >= 16,
        "the doc table was not parsed; {} rows found - a reformat broke the row",
        documented.len()
    );
    let from_code: Vec<String> = lca_ext_abi::WORLDS
        .iter()
        .map(|(name, _)| (*name).to_string())
        .collect();
    assert_eq!(
        documented, from_code,
        "the doc's world table and `lca_ext_abi::WORLDS` must list the same worlds, \
         in the same order"
    );
}

/// The loader asks "does the manifest declare this world?" in terms of `World`,
/// never by comparing world strings.
///
/// gh #288's deeper half: after the validator was fixed, the *loader* still spelled
/// world names as string literals (`.any(|world| world == "ui")`). A typo there -
/// `"uii"` - passed every test in this file, because validation accepted the name
/// the author wrote while the loader never matched it. Measured: injecting
/// `"uii"` for `"ui"` left all ten world rows green. Asking in terms of
/// `lca_ext_abi::World` makes that a compile error.
#[test]
fn the_loader_compares_worlds_as_worlds_not_strings() {
    let loader = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/lib.rs"))
        .expect("the loader is in the tree");
    // Any world name at all in a comparison: the check is "no world string is
    // compared", not "these four specific lines are gone". A list of forbidden
    // literals only catches the spellings I happened to think of - which is how
    // my first version of this row missed an `if world == "uii"` injection.
    for (name, _) in lca_ext_abi::WORLDS {
        let needle = format!("world == \"{name}\"");
        assert!(
            !loader.contains(&needle),
            "the loader still compares a world string ({needle}); a typo there is \
             a silent no-op that no validation can catch"
        );
    }
    assert!(
        loader.contains("lca_ext_abi::parse_world(world)"),
        "the loader resolves declared worlds through the table"
    );
    // And the accessor exists, so a caller with a `Manifest` in hand does not
    // reach for the strings either.
    let manifest = manifest_with_worlds(r#"["ui", "hooks-tool-call"]"#).expect("a valid manifest");
    assert!(manifest.declares(lca_ext_abi::World::Ui));
    assert!(manifest.declares(lca_ext_abi::World::HooksToolCall));
    assert!(
        !manifest.declares(lca_ext_abi::World::Tool),
        "an undeclared world answers false"
    );
}
