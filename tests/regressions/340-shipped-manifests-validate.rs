//! Gh #340 - the normative manifest schema would reject manifests the product
//! ships, and nothing checked that.
//!
//! Three drifts, all verified against the tree before fixing:
//!
//! 1. `extensions/openai-compatible/extension.toml` declares a top-level
//!    `resources = [...]`, and the schema had `additionalProperties: false` with
//!    no `resources` property - so a **first-party shipped manifest** failed the
//!    normative document. The host requires `resources` for a data-only package
//!    (`worlds.is_empty() && resources.is_empty()` is refused) while the schema
//!    said `worlds` had `minItems: 1`, so the schema contradicted both the
//!    product and the loader (ADR-0030).
//! 2. `limits.fuel_per_call` documented a default of 10,000,000 against the
//!    host's `DEFAULT_FUEL_PER_CALL` of 25,000,000 - the number an author reads.
//! 3. `$id` and the description still said ABI 0.1 while the line is 0.6.
//!
//! # Why this row exists beside the script's check
//!
//! `scripts/docs-consistency.sh` runs the **complete** JSON-Schema validation
//! through `python3 -m jsonschema` (with a skip-with-a-reason when that module
//! is absent). This row is the *subset* of the same rules in Rust, so the
//! normative facts are also pinned on a machine or CI leg without the Python
//! module, and so a failure names the rule rather than a validator's path
//! expression. The subset is stated plainly: top-level key set, "a world or a
//! resource", the `worlds` enum, and the fuel default. It is not a general
//! validator and does not pretend to be.
//!
//! Verifies: gh #340.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failure.

use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The shipped `extension.toml` files, as `(path, text)`.
fn shipped_manifests() -> Vec<(String, String)> {
    let mut out = Vec::new();
    let dir = root().join("extensions");
    for entry in std::fs::read_dir(&dir).expect("read extensions/") {
        let entry = entry.expect("extension directory entry");
        let manifest = entry.path().join("extension.toml");
        if manifest.is_file() {
            let text = std::fs::read_to_string(&manifest).expect("read the manifest");
            out.push((entry.file_name().to_string_lossy().into_owned(), text));
        }
    }
    assert!(
        out.len() >= 20,
        "the tree still ships its extensions ({} found)",
        out.len()
    );
    out.sort();
    out
}

/// The top-level keys of a manifest, as the schema's own vocabulary names them.
///
/// A text scan rather than a TOML parse: the regressions crate has no `toml`
/// dependency (deliberately - the brief forbids adding one), and the shape read
/// here is "a bare key starting at column 0 before any table header", which is
/// exact for this file format.
fn top_level_keys(text: &str) -> Vec<String> {
    text.lines()
        .take_while(|line| !line.trim_start().starts_with('['))
        .filter(|line| !line.trim_start().starts_with('#'))
        .filter_map(|line| line.split('=').next())
        .map(str::trim)
        .filter(|key| !key.is_empty() && !key.contains('"'))
        .map(str::to_string)
        .collect()
}

/// The schema's declared top-level property names, read from the real schema.
fn schema_properties() -> Vec<String> {
    let text = std::fs::read_to_string(root().join("schemas/extension-manifest.schema.json"))
        .expect("the schema is in the tree");
    let schema: serde_json::Value = serde_json::from_str(&text).expect("valid JSON");
    schema["properties"]
        .as_object()
        .expect("the schema has properties")
        .keys()
        .cloned()
        .collect()
}

/// `worlds = [...]`'s entries, or empty; the manifest's own spelling.
fn worlds_of(text: &str) -> Vec<String> {
    let Some(start) = text.find("worlds = [") else {
        return Vec::new();
    };
    let rest = &text[start + "worlds = [".len()..];
    let end = rest.find(']').expect("worlds closes");
    rest[..end]
        .split(',')
        .map(|item| item.trim().trim_matches('"').to_string())
        .filter(|item| !item.is_empty())
        .collect()
}

/// Whether the manifest declares at least one resource kind.
fn declares_resources(text: &str) -> bool {
    let Some(start) = text.find("resources = [") else {
        return false;
    };
    let rest = &text[start + "resources = [".len()..];
    let end = rest.find(']').expect("resources closes");
    !rest[..end].trim().trim_matches(',').is_empty()
}

// Verifies: gh #340 - **every shipped manifest's top-level keys are schema
// properties.** This is the rule `additionalProperties: false` states, and the
// one that rejected `resources` on a first-party manifest.
#[test]
fn every_shipped_manifest_uses_only_schema_top_level_keys() {
    let allowed = schema_properties();
    for (name, text) in shipped_manifests() {
        for key in top_level_keys(&text) {
            assert!(
                allowed.contains(&key),
                "`extensions/{name}` declares top-level `{key}`, which the normative schema does                  not have as a property (so `additionalProperties: false` refuses the manifest).                  Schema properties: {allowed:?}"
            );
        }
    }
    // The defect side: the property this cycle added is the one that was missing.
    assert!(
        allowed.iter().any(|name| name == "resources"),
        "the schema has a top-level `resources` property (ADR-0030's data-only shape): {allowed:?}"
    );
}

// Verifies: gh #340 - **a manifest is a component or a bag, never neither.**
//
// The host refuses `worlds.is_empty() && resources.is_empty()`
// (`crates/lca-ext-host/src/manifest.rs`); the schema says the same thing in its
// `allOf` clause. This row asserts the shape over every shipped manifest, and
// both directions of the rule: an empty world list is legal exactly when
// resources are declared.
#[test]
fn every_shipped_manifest_implements_a_world_or_declares_a_resource() {
    for (name, text) in shipped_manifests() {
        let worlds = worlds_of(&text);
        let resources = declares_resources(&text);
        assert!(
            !worlds.is_empty() || resources,
            "`extensions/{name}` declares neither a world nor a resource kind, which the host              refuses at load (ADR-0030)"
        );
    }
    // The negative direction, so this cannot pass by every manifest having both:
    // the data-only shape is accepted, and the neither-shape is refused.
    let none = "name = \"nothing\"\nversion = \"0.1.0\"\nabi = \"0.6\"\nworlds = []\n";
    assert!(
        worlds_of(none).is_empty() && !declares_resources(none),
        "the fixture is the refused shape"
    );
    let only_bag = "name = \"bag\"\nversion = \"0.1.0\"\nabi = \"0.6\"\nworlds = []\nresources = [\"skills\"]\n";
    assert!(
        worlds_of(only_bag).is_empty() && declares_resources(only_bag),
        "the fixture is the data-only shape"
    );
}

// Verifies: gh #340 - **the `worlds` entries are the vocabulary.**
//
// The schema's 17-entry enum and `lca_ext_abi::WORLDS` are already pinned as
// equal sets by gh #288's rows; this one closes the *third* copy: what the
// shipped manifests actually spell. A manifest naming a world the code does not
// dispatch is a silent no-op at load.
#[test]
fn every_shipped_manifest_names_real_worlds() {
    let schema: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(root().join("schemas/extension-manifest.schema.json"))
            .expect("the schema is in the tree"),
    )
    .expect("valid JSON");
    let enum_names: Vec<String> = schema["properties"]["worlds"]["items"]["enum"]
        .as_array()
        .expect("worlds.items.enum")
        .iter()
        .map(|value| value.as_str().expect("string").to_string())
        .collect();
    for (name, text) in shipped_manifests() {
        for world in worlds_of(&text) {
            assert!(
                enum_names.contains(&world),
                "`extensions/{name}` implements `{world}`, which is not in the schema's worlds                  enum (and therefore not in `lca_ext_abi::WORLDS`)"
            );
        }
    }
}

// Verifies: gh #340 - **the fuel default is the host's, not a stale number.**
//
// The schema's `limits.fuel_per_call.default` is what an author reads when
// deciding whether to declare the key at all; it said 10,000,000 while the host
// runs 25,000,000. The number is asserted against the *host constant*, so a
// future change to either side fails here rather than shipping a number nobody
// honours.
#[test]
fn the_schema_fuel_default_is_the_hosts_default() {
    let schema: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(root().join("schemas/extension-manifest.schema.json"))
            .expect("the schema is in the tree"),
    )
    .expect("valid JSON");
    let default = schema["properties"]["limits"]["properties"]["fuel_per_call"]["default"]
        .as_u64()
        .expect("the fuel default is an integer");
    assert_eq!(
        default,
        lca_ext_host::DEFAULT_FUEL_PER_CALL,
        "the schema documents fuel_per_call default {default}, the host runs {}",
        lca_ext_host::DEFAULT_FUEL_PER_CALL
    );
}

// Verifies: gh #340 - **the schema names the ABI line it describes.**
//
// `$id` and the description said 0.1 while every manifest in the tree targets
// 0.6. A schema whose own name is a different line than the one it validates is
// how an author ends up reading the wrong document.
#[test]
fn the_schema_names_the_current_abi_line() {
    let schema: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(root().join("schemas/extension-manifest.schema.json"))
            .expect("the schema is in the tree"),
    )
    .expect("valid JSON");
    let id = schema["$id"].as_str().expect("$id is a string");
    let description = schema["description"].as_str().expect("a description");
    let line = lca_ext_abi::ABI_VERSION;
    assert!(
        id.contains(&format!("/{line}.json")),
        "the schema's $id names ABI {line}: {id}"
    );
    assert!(
        description.contains(line),
        "and its description names the line too: {description}"
    );
}
