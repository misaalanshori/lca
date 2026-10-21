//! The embedded model catalog (gh #260) as a consumer sees it: the artifact
//! size ceiling, the user drop-in's precedence, and the lookup contract the
//! issue's acceptance criteria name.
//!
//! These rows drive the public surface only (`lca_registry::models`), so the
//! unit tests inside the module can change shape without these drifting.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use lca_registry::models::{self, Catalog, ModelDetails, parse_catalog};

// The build script's decision logic, included rather than duplicated so the
// rule "a CI build never fetches" is asserted against the code that runs
// (gh #260 fix-up). Only `std` in there, so it compiles unchanged as a test
// module.
#[allow(dead_code, missing_docs)]
mod build_support {
    include!("../build_support.rs");
}

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

// ---------------------------------------------------------------------------
// gh #260 fix-up: NFR-17's reproducible double build
// ---------------------------------------------------------------------------
//
// The pipeline's double build (`.github/workflows/publish.yml`) runs two
// independent `CARGO_TARGET_DIR`s on one fresh checkout and compares the two
// binary hashes. Before this fix both builds missed the cache and both
// fetched models.dev across a network boundary, so an upstream or CDN
// difference between the two responses changed a compressed blob inside the
// binary and failed the gate for a reason unrelated to the commit.
//
// Two mechanisms make it deterministic, and these rows pin both of them.
// They drive the exact functions `build.rs` runs, rather than a copy of the
// rules: `build.rs` and this test `include!` the same `build_support.rs`.

use std::ffi::OsStr;
use std::path::Path;

/// `OsStr` from a string literal.
fn os(value: &str) -> &OsStr {
    OsStr::new(value)
}

// Verifies: gh #260 fix-up (NFR-17) - a CI build never reaches the network.
//
// `CI` and `GITHUB_ACTIONS` both mean "offline", in every combination, and
// the result is the stub even when a perfectly good cache is sitting right
// there: a cached artifact is per-checkout state, and a fresh release
// checkout has none, so a rule that trusted it would behave differently on
// the pipeline than on a developer's machine.
#[test]
fn a_ci_build_chooses_the_stub_and_never_the_network() {
    use build_support::{Origin, choose, in_ci};

    assert!(in_ci(Some(os("1")), None), "CI alone is enough");
    assert!(
        in_ci(None, Some(os("true"))),
        "GITHUB_ACTIONS alone is enough"
    );
    assert!(in_ci(Some(os("true")), Some(os("true"))));
    assert!(!in_ci(None, None), "no marker, no CI");
    assert!(
        !in_ci(Some(os("false")), None),
        "CI=false means what it says"
    );
    assert!(!in_ci(Some(os("0")), None));

    for ci in [true] {
        // Every CI combination lands on the stub, with or without a cache
        // and with or without a refresh request.
        for (refresh, cache_ok) in [(false, true), (false, false), (true, true), (true, false)] {
            assert_eq!(
                choose(false, false, ci, refresh, cache_ok),
                Origin::Stub,
                "a CI build resolves to the stub (refresh={refresh}, cache_ok={cache_ok})"
            );
        }
        // And `refresh` in CI still cannot reach the network.
        assert_ne!(choose(false, false, ci, true, false), Origin::Network);
    }

    // The development path is unchanged: cache, then network.
    assert_eq!(choose(false, false, false, false, true), Origin::Cache);
    assert_eq!(choose(false, false, false, false, false), Origin::Network);
    assert_eq!(
        choose(false, false, false, true, true),
        Origin::Network,
        "refresh ignores a warm cache"
    );
    // An explicit offline switch wins over the cache outside CI too.
    assert_eq!(choose(false, true, false, false, true), Origin::Stub);
    // A usable pinned input beats everything.
    for (offline, ci) in [(false, false), (true, false), (false, true), (true, true)] {
        assert_eq!(
            choose(true, offline, ci, false, true),
            Origin::Pinned,
            "a pinned input is used even offline and in CI (offline={offline}, ci={ci})"
        );
    }
}

// Verifies: gh #260 fix-up (NFR-17) - the cache lives where the build is.
//
// The defect this row was written for: `build.rs` hard-coded
// `<workspace>/target`, so a build that set `CARGO_TARGET_DIR` (the release
// job's `target/repro-a` / `target/repro-b`, or any `--target-dir` user)
// never found its own cache and refetched every time.
#[test]
fn the_cache_follows_the_target_dir() {
    use build_support::{CACHE_FILE, cache_path};

    let workspace = Path::new("/src");

    // Unset: the conventional workspace target.
    assert_eq!(
        cache_path(None, workspace),
        Path::new("/src/target").join(CACHE_FILE)
    );
    // Empty is the same as unset (cargo treats it that way).
    assert_eq!(
        cache_path(Some(os("")), workspace),
        Path::new("/src/target").join(CACHE_FILE)
    );
    // The release job's shape: a relative target dir resolves at the
    // workspace root, which is the working directory there.
    assert_eq!(
        cache_path(Some(os("target/repro-a")), workspace),
        Path::new("/src/target/repro-a").join(CACHE_FILE)
    );
    assert_eq!(
        cache_path(Some(os("target/repro-b")), workspace),
        Path::new("/src/target/repro-b").join(CACHE_FILE)
    );
    // An absolute one stands as written.
    assert_eq!(
        cache_path(Some(os("/build/t")), workspace),
        Path::new("/build/t").join(CACHE_FILE)
    );
    // Two distinct target dirs get two distinct caches: this is exactly why
    // they cannot share one through the network, and why CI uses the stub.
    assert_ne!(
        cache_path(Some(os("target/repro-a")), workspace),
        cache_path(Some(os("target/repro-b")), workspace)
    );
}

// Verifies: gh #260 fix-up (NFR-17) - a CI build says what it shipped.
//
// The coverage cost is real: the stub is 13 models across 5 providers, not
// models.dev's 9,075. A release built in CI must not report full coverage
// silently, so the warning names the stub and the way to pin the full
// catalog.
#[test]
fn the_ci_warning_names_the_stub_and_the_pin() {
    use build_support::{Origin, warning};

    let line = warning(Origin::Stub, true);
    assert!(line.contains("CI build"), "{line}");
    assert!(line.contains("stub"), "{line}");
    assert!(line.contains("NFR-17"), "the reason is named: {line}");
    assert!(
        line.contains("LCA_MODELS_CATALOG"),
        "the way out is named: {line}"
    );
    // Outside CI the same origin is not news.
    let dev = warning(Origin::Stub, false);
    assert!(!dev.contains("CI build"), "{dev}");
}

// Verifies: gh #260 fix-up (NFR-17) - the flags mean one thing.
#[test]
fn a_flag_reads_the_shapes_ci_sets() {
    use build_support::flag;

    // The values GitHub Actions and a shell actually export.
    for on in ["1", "true", "TRUE", "yes", "anything"] {
        assert!(flag(Some(os(on))), "{on} is on");
    }
    for off in ["", "0", "false", "FALSE", "no"] {
        assert!(!flag(Some(os(off))), "{off:?} is off");
    }
    assert!(!flag(None), "unset is off");
}
