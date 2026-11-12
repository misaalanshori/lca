//! The catalog rows (gh #8/#112): parsing keeps every hub field, merging marks
//! provenance, ABI filtering follows the host window, and the cache knows fresh
//! from stale.
//!
//! Split out of `tests/registry.rs` beside the `catalog` module.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

// Verifies: #227 - catalog parsing: the format gate refuses, entries
// carry every field the hub lists, and capabilities ride along.
#[test]
fn catalog_parsing_keeps_every_hub_field() {
    let text = r#"
[version]
format = 1

[[extension]]
name = "probe"
version = "1.2.0"
abi = "0.6"
category = "provider"
description = "A probe."
oci_ref = "ghcr.io/example/probe:abi-0.6"
capabilities = ["net", "credentials"]
"#;
    let entries = lca_registry::parse_catalog(text).expect("parses");
    assert_eq!(entries.len(), 1);
    let entry = &entries[0];
    assert_eq!(entry.name, "probe");
    assert_eq!(entry.version, "1.2.0");
    assert_eq!(entry.abi, "0.6");
    assert_eq!(entry.category, "provider");
    assert_eq!(entry.oci_ref, "ghcr.io/example/probe:abi-0.6");
    assert_eq!(entry.capabilities, vec!["net", "credentials"]);
    assert!(lca_registry::parse_catalog("no toml [[[[").is_err());
    assert!(lca_registry::parse_catalog("[version]\nformat = 99\n").is_err());
    assert!(
        lca_registry::parse_catalog("[version]\nformat = 1\n[[extension]]\nname = \"x\"\n")
            .is_err()
    );
}

// Verifies: #227 - provenance merging: official rows lead, each custom
// catalog follows under its own name, and a name present twice lists
// twice so the hub can show both origins.
#[test]
fn catalog_merging_marks_every_provenance() {
    let official = lca_registry::CatalogSource {
        id: "official".to_string(),
        name: "Official (misaalanshori/lca)".to_string(),
        url: None,
    };
    let custom = lca_registry::CatalogSource {
        id: "acme".to_string(),
        name: "Acme Corp".to_string(),
        url: Some("https://example.invalid/catalog.toml".to_string()),
    };
    let entry = |name: &str| lca_registry::CatalogEntry {
        name: name.to_string(),
        version: "1.0.0".to_string(),
        abi: "0.6".to_string(),
        category: "provider".to_string(),
        description: String::new(),
        oci_ref: String::new(),
        capabilities: Vec::new(),
    };
    let merged = lca_registry::merge_catalogs(&[
        (official, vec![entry("a"), entry("shared")]),
        (custom, vec![entry("shared"), entry("b")]),
    ]);
    let names: Vec<(&str, &str, bool, Option<&str>)> = merged
        .iter()
        .map(|row| {
            (
                row.entry.name.as_str(),
                row.source_id.as_str(),
                row.official,
                row.source_url.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        names,
        vec![
            ("a", "official", true, None),
            ("shared", "official", true, None),
            (
                "shared",
                "acme",
                false,
                Some("https://example.invalid/catalog.toml")
            ),
            (
                "b",
                "acme",
                false,
                Some("https://example.invalid/catalog.toml")
            ),
        ]
    );
}

// Verifies: #227 - ABI filtering follows the host window: current and
// previous minors load, older lines and garbage do not.
#[test]
fn catalog_abi_filtering_follows_the_host_window() {
    let entry = |abi: &str| lca_registry::CatalogEntry {
        name: "x".to_string(),
        version: "1.0.0".to_string(),
        abi: abi.to_string(),
        category: "provider".to_string(),
        description: String::new(),
        oci_ref: String::new(),
        capabilities: Vec::new(),
    };
    for abi in ["0.6", "0.5", "1.0"] {
        assert!(
            lca_registry::abi_supported(&entry(abi)),
            "{abi} loads on this host"
        );
    }
    for abi in ["0.4", "0.7", "1.1", "nope", ""] {
        assert!(
            !lca_registry::abi_supported(&entry(abi)),
            "{abi} is filtered out"
        );
    }
}

// Verifies: #227 - the shipped first-party catalog parses with every
// row loadable: versions, refs, and capabilities travel together, so
// catalog drift fails here instead of in the hub.
#[test]
fn shipped_catalog_parses_with_every_row_loadable() {
    let text = include_str!("../../../../extensions/catalog.toml");
    let entries = lca_registry::parse_catalog(text).expect("shipped catalog parses");
    // SANCTIONED CHANGE (gh #418 Phase A): the floor was 13. `mcp` left the
    // catalog because it is now a first-class internal subsystem
    // (`crates/lca-mcp`, ADR-0047), not an installable extension - there is no
    // `mcp.wasm` to publish any more. The floor still guards the regression it
    // was written for (a row silently vanishing), so it moves with the shipped
    // set rather than being deleted.
    assert!(entries.len() >= 12, "all shippable extensions listed");
    for entry in &entries {
        assert!(
            lca_registry::abi_supported(entry),
            "{} names an installable line: {}",
            entry.name,
            entry.abi
        );
        assert!(
            entry.oci_ref.starts_with("ghcr.io/misaalanshori/lca/"),
            "{} names the first-party registry: {}",
            entry.name,
            entry.oci_ref
        );
        assert!(
            !entry.description.is_empty(),
            "{} carries a line",
            entry.name
        );
    }
    let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
    for expected in [
        "openai-compatible",
        "anthropic",
        "antigravity",
        "codex",
        "github-copilot",
        "grok",
        "kimi-coding",
        "llama",
        "meta",
        "openrouter",
        "compaction-default",
        "skills",
    ] {
        assert!(names.contains(&expected), "{expected} is catalogued");
    }
    // SANCTIONED CHANGE (gh #418 Phase A): `mcp` was on that list. It is a
    // first-class internal subsystem now (`crates/lca-mcp`, ADR-0047) with no
    // WASM guest and nothing to publish, so it is deliberately absent - and the
    // absence is asserted, not merely tolerated, so a stray re-add fails here.
    assert!(
        !names.contains(&"mcp"),
        "mcp is an internal subsystem, not a catalogued extension (gh #418)"
    );
}

// Verifies: #227 - the TTL cache: a fresh copy reads without network,
// and freshness is an age comparison the hub can trust.
#[test]
fn catalog_cache_distinguishes_fresh_from_stale() {
    let dir = lca_testkit::scratch_path("lca-catalog-cache");
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join("official.toml");
    assert!(lca_registry::read_cache(&path).is_none(), "absent is none");
    let text = "[version]\nformat = 1\n";
    lca_registry::write_cache(&path, text).expect("write");
    let (entries, fresh) = lca_registry::read_cache(&path).expect("reads");
    assert!(entries.is_empty());
    assert!(fresh, "a just-written copy is fresh");
    assert!(
        lca_registry::cache_fresh(std::time::Duration::from_secs(60)),
        "a minute is fresh"
    );
    assert!(
        !lca_registry::cache_fresh(lca_registry::CATALOG_TTL + std::time::Duration::from_secs(1)),
        "past the TTL is stale"
    );
}
