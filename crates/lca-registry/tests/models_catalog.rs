//! The embedded model catalog (gh #260) as a consumer sees it: the artifact
//! size ceiling, the user drop-in's precedence, and the lookup contract the
//! issue's acceptance criteria name.
//!
//! These rows drive the public surface only (`lca_registry::models`), so the
//! unit tests inside the module can change shape without these drifting.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use lca_registry::models::{self, Catalog, ModelDetails, parse_catalog};

/// A three-provider fixture shaped exactly like the stripped artifact.
fn fixture() -> Catalog {
    parse_catalog(
        br#"{
            "openai": {"models": {
                "gpt-4o": {
                    "canonical_model_id": "openai/gpt-4o",
                    "cost": {"input": 2.5, "output": 10.0, "cache_read": 1.25},
                    "limit": {"context": 128000, "output": 16384}
                }
            }},
            "groq": {"models": {
                "llama-3.3-70b-versatile": {"cost": {"input": 0.59, "output": 0.79}}
            }},
            "venice": {"models": {
                "llama-3.3-70b-versatile": {"cost": {"input": 2.8, "output": 3.5}}
            }},
            "zzz-unlisted": {"models": {}}
        }"#,
    )
    .expect("fixture parses")
}

// Verifies: gh #260 acceptance 2 - the embedded artifact is under 200 KiB, and
// a build that fetched carries a real provider matrix. A build that ran
// offline embeds the stub instead (acceptance 6), so the width assertion is
// scoped to the fetched artifact rather than to whatever this machine's build
// happened to do.
#[test]
fn the_embedded_catalog_is_under_two_hundred_kib() {
    let path = env!("LCA_MODELS_CATALOG_GZIP");
    let size = std::fs::metadata(path)
        .unwrap_or_else(|err| panic!("the embedded catalog at {path} is readable: {err}"))
        .len();
    assert!(
        size <= 200 * 1024,
        "the embedded catalog is {size} bytes; the issue's ceiling is 200 KiB"
    );

    let catalog = models::embedded();
    assert!(!catalog.is_empty(), "the build embedded a catalog");
    let model_count: usize = catalog.values().map(|entry| entry.models.len()).sum();
    assert!(model_count > 0, "the embedded catalog carries models");

    // Only a fetched (gzipped) artifact promises the full matrix; the stub is
    // deliberately a handful of rows.
    let gzipped = std::fs::read(path).is_ok_and(|bytes| bytes.starts_with(&[0x1f, 0x8b]));
    if gzipped {
        assert!(
            model_count >= 1000,
            "a fetched catalog carries a real provider matrix, not {model_count} models"
        );
        assert!(
            catalog.len() >= 100,
            "a fetched catalog carries most providers, not {}",
            catalog.len()
        );
    }
}

// Verifies: gh #260 acceptance 2 - provider-specific costs survive. The same
// model priced by two providers resolves to two different numbers, which is
// the whole reason the catalog is keyed by provider.
#[test]
fn provider_specific_costs_survive_the_lookup() {
    let catalog = fixture();
    let groq = models::lookup_in(None, &catalog, "groq", "llama-3.3-70b-versatile")
        .expect("groq lists it");
    let venice = models::lookup_in(None, &catalog, "venice", "llama-3.3-70b-versatile")
        .expect("venice lists it");
    assert_eq!(groq.cost.as_ref().and_then(|c| c.input), Some(0.59));
    assert_eq!(venice.cost.as_ref().and_then(|c| c.input), Some(2.8));
    assert_ne!(
        groq.rates().map(|r| r.input_per_mtok),
        venice.rates().map(|r| r.input_per_mtok),
        "the provider is part of the price"
    );
}

// Verifies: gh #260 acceptance 5 - a user drop-in at
// `~/.lca/catalogs/models.json` takes precedence over the embedded catalog,
// and the path is the one the docs name.
#[test]
fn the_user_catalog_path_is_where_the_docs_say_it_is() {
    let path = models::user_catalog_path(std::path::Path::new("/home/u/.lca"));
    assert_eq!(
        path,
        std::path::Path::new("/home/u/.lca/catalogs/models.json")
    );
}

// Verifies: gh #260 acceptance 5 - precedence is user, then embedded, then
// nothing. Exercised over explicit catalogs so the row never depends on what
// the machine's home directory holds.
#[test]
fn the_user_catalog_wins_and_the_embedded_one_fills_the_gaps() {
    let embedded = fixture();
    let user =
        parse_catalog(br#"{"openai":{"models":{"gpt-4o":{"cost":{"input":9.0,"output":90.0}}}}}"#)
            .expect("user catalog parses");
    let overridden = models::lookup_in(Some(&user), &embedded, "openai", "gpt-4o").expect("found");
    assert_eq!(overridden.cost.as_ref().and_then(|c| c.input), Some(9.0));
    let untouched = models::lookup_in(Some(&user), &embedded, "groq", "llama-3.3-70b-versatile")
        .expect("the embedded catalog still answers");
    assert_eq!(untouched.cost.as_ref().and_then(|c| c.input), Some(0.59));
    assert!(
        models::lookup_in(Some(&user), &embedded, "groq", "no-such-model").is_none(),
        "and nothing answers for a model nobody lists"
    );
}

// Verifies: gh #260 acceptance 3 - one struct set parses the stripped artifact
// and a raw `models.dev/api.json`, extras and all.
#[test]
fn one_schema_parses_the_stripped_artifact_and_a_raw_document() {
    // The stored shape the build writes.
    let stripped = parse_catalog(
        br#"{"openai":{"models":{"gpt-4o":{"canonical_model_id":"openai/gpt-4o",
            "cost":{"input":2.5,"output":10.0,"cache_read":1.25},
            "limit":{"context":128000,"output":16384}}}}}"#,
    )
    .expect("stripped parses");
    // The raw shape a user downloads from models.dev.
    let raw = parse_catalog(
        br#"{"openai":{"id":"openai","name":"OpenAI","env":["OPENAI_API_KEY"],
            "npm":"@ai-sdk/openai","doc":"https://example.test","models":{"gpt-4o":{
            "id":"gpt-4o","name":"GPT-4o","description":"marketing copy",
            "attachment":true,"reasoning":false,"tool_call":true,
            "knowledge":"2024-06","release_date":"2024-05-13",
            "modalities":{"input":["text","image"],"output":["text"]},
            "limit":{"context":128000,"output":16384},
            "cost":{"input":2.5,"output":10.0,"cache_read":1.25},
            "canonical_model_id":"openai/gpt-4o"}}}}"#,
    )
    .expect("raw parses");
    assert_eq!(
        stripped["openai"].models["gpt-4o"], raw["openai"].models["gpt-4o"],
        "the strip removes only fields nothing reads"
    );
}

// Verifies: gh #260 acceptance 4's shape - `lca-core` names no vendor. The
// old hand-curated table is gone and the crate's source carries no model id;
// this row is the guard that keeps one from creeping back in.
#[test]
fn the_core_crate_carries_no_vendor_model_ids() {
    let core = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../lca-core");
    assert!(
        !core.join("resources/pricing.toml").exists(),
        "crates/lca-core/resources/pricing.toml is deleted (acceptance 4)"
    );
    let mut offenders = Vec::new();
    let mut stack = vec![core.join("src")];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("read source tree").flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("read source");
            // Test code may name a model: it is pinning behavior, not
            // carrying a price. Production code is everything before the
            // file's `#[cfg(test)]` block (the workspace convention).
            let production = text.split("\n#[cfg(test)]").next().unwrap_or(text.as_str());
            // A quoted vendor model id in production code: the shape the
            // deleted table used, and the one the issue names.
            for model in [
                "\"gpt-4o\"",
                "\"gpt-4.1\"",
                "\"claude-sonnet-4-5\"",
                "\"claude-opus-4-1\"",
                "\"claude-haiku-4-5\"",
                "\"gemini-2.5-pro\"",
                "\"gemini-2.5-flash\"",
                "\"gpt-4o-mini\"",
            ] {
                if production.contains(model) {
                    offenders.push(format!("{}: {model}", path.display()));
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "lca-core must name no vendor model:\n{}",
        offenders.join("\n")
    );
}

// Verifies: gh #260 - the lookup never invents a rate. A row with a limit but
// no cost still answers with its limit and yields no rates, so the caller
// records tokens only.
#[test]
fn a_row_without_rates_still_reports_its_limit() {
    let details = ModelDetails {
        limit: Some(models::ModelLimit {
            context: Some(200_000),
            output: Some(8_192),
        }),
        cost: None,
        ..ModelDetails::default()
    };
    assert_eq!(details.context_window(), Some(200_000));
    assert!(
        details.rates().is_none(),
        "no cost block means no billable number"
    );
}
