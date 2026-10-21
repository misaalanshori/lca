//! The embedded model catalog (gh #260): per-provider prices, context limits,
//! and reasoning flags from `models.dev`, with a user drop-in override.
//!
//! `lca-core` carries no vendor knowledge: when a provider reports no cost it
//! calls [`lookup`] and applies the returned rates. Everything vendor-shaped
//! lives here, in the data.
//!
//! ## Resolution order (highest first)
//!
//! 1. The provider's own wire cost - the caller checks that before asking;
//!    this module never overrides a reported number.
//! 2. The user's drop-in catalog, `~/.lca/catalogs/models.json`. It uses the
//!    same schema as `models.dev/api.json`, so a raw download parses as-is
//!    and a hand-written file with three models behaves the same way.
//! 3. The embedded build-time catalog (gh #260: `build.rs`).
//! 4. Nothing: [`lookup`] returns `None`, and the caller records tokens only.
//!    A rate nobody verified is never invented.
//!
//! ## Lookup key
//!
//! [`lookup`] takes a *provider id* (`openai`, `groq`, `opencode-go`) and a
//! model id. The same model priced on two providers is two rows, because the
//! services bill differently (`llama-3.3-70b` is $0.59/M on Groq and $2.80/M on
//! Venice). Resolution tries, in order:
//!
//! 1. The exact provider entry and the exact model id.
//! 2. The model id with any `profile/` or vendor prefix removed, in the same
//!    provider entry.
//! 3. Any provider entry that lists that bare id, preferring one whose
//!    `canonical_model_id` names it. This is the issue's canonical fallback:
//!    a user on an unlisted reseller still gets a real number instead of $0.
//!
//! ## What a caller gets
//!
//! [`ModelDetails`] is the issue's schema. [`ModelDetails::rates`] turns its
//! `cost` block into the per-1M-token rates the pricing math wants, applying
//! models.dev's rule that an absent cache rate falls back to the input rate.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::Deserialize;

/// One catalog document: provider id to that provider's models.
///
/// This is the *same* type for the embedded catalog, the user drop-in, and a
/// raw `models.dev/api.json` (acceptance 3): every field this module does not
/// name is ignored by Serde, and every field it does name is optional, so a
/// full document and a three-model file both parse.
pub type Catalog = HashMap<String, ProviderEntry>;

/// One provider's entry: its model map, plus whatever else the document
/// carries. Only `models` is read; `name`, `doc`, `env`, and `npm` are
/// ignored on purpose (the build-time strip removes them, and a raw document
/// that still carries them must parse identically).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ProviderEntry {
    /// Model id to that model's details. Missing is an empty map.
    #[serde(default)]
    pub models: HashMap<String, ModelDetails>,
}

/// What the catalog knows about one model. Every field is optional, so a
/// partially specified user entry overrides only what it names.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct ModelDetails {
    /// The vendor-qualified id this model is a variant of
    /// (`openai/gpt-4o` for a reseller's `gpt-4o`). The canonical fallback
    /// uses it to prefer the row that names the model itself.
    #[serde(default)]
    pub canonical_model_id: Option<String>,
    /// Token limits.
    #[serde(default)]
    pub limit: Option<ModelLimit>,
    /// Per-1M-token USD rates.
    #[serde(default)]
    pub cost: Option<ModelCost>,
    /// Whether the model exposes a reasoning/thinking control.
    #[serde(default)]
    pub reasoning: bool,
}

/// The token limits `models.dev` publishes. Input and output are separate
/// ceilings; `context` is the total.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct ModelLimit {
    /// Total context window in tokens.
    #[serde(default)]
    pub context: Option<u32>,
    /// Maximum output tokens, when the provider publishes one.
    #[serde(default)]
    pub output: Option<u32>,
}

/// Per-1M-token USD rates. A missing rate is not zero: see
/// [`ModelDetails::rates`] for the fallbacks.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct ModelCost {
    /// Fresh input tokens.
    #[serde(default)]
    pub input: Option<f64>,
    /// Output tokens.
    #[serde(default)]
    pub output: Option<f64>,
    /// Input tokens served from the prompt cache. Vendors discount these
    /// heavily, so an absent rate falls back to `input` rather than to zero -
    /// the same degradation gh #125 chose.
    #[serde(default)]
    pub cache_read: Option<f64>,
    /// Input tokens written to the prompt cache. Some vendors charge a
    /// premium here (the Claude line bills 1.25x input).
    #[serde(default)]
    pub cache_write: Option<f64>,
}

impl ModelDetails {
    /// The rates to bill with, or `None` when the catalog has no input or
    /// output rate (nothing to compute a cost from).
    ///
    /// Fallbacks, in the order gh #125 and gh #274 established:
    ///
    /// * `cache_read` absent or zero: the input rate.
    /// * `cache_write` absent or zero: the input rate.
    /// * the one-hour cache tier is `2 * input`, which is Anthropic's
    ///   published rule; models.dev carries no one-hour rate at all.
    #[must_use]
    pub fn rates(&self) -> Option<ModelRates> {
        let cost = self.cost.as_ref()?;
        let input = cost.input.filter(|rate| *rate > 0.0)?;
        let output = cost.output.filter(|rate| *rate > 0.0)?;
        let cache_read = cost.cache_read.filter(|rate| *rate > 0.0).unwrap_or(input);
        let cache_write = cost.cache_write.filter(|rate| *rate > 0.0).unwrap_or(input);
        Some(ModelRates {
            input_per_mtok: input,
            cache_read_per_mtok: cache_read,
            cache_write_per_mtok: cache_write,
            cache_write_1h_per_mtok: input * 2.0,
            output_per_mtok: output,
        })
    }

    /// The context window, when the catalog knows one and it is not the `0`
    /// models.dev uses for "not applicable" (an embedding model, say).
    #[must_use]
    pub fn context_window(&self) -> Option<u32> {
        self.limit
            .as_ref()
            .and_then(|limit| limit.context)
            .filter(|tokens| *tokens > 0)
    }

    /// The maximum output tokens, under the same rule as
    /// [`ModelDetails::context_window`].
    #[must_use]
    pub fn max_output_tokens(&self) -> Option<u32> {
        self.limit
            .as_ref()
            .and_then(|limit| limit.output)
            .filter(|tokens| *tokens > 0)
    }
}

/// Per-1M-token USD rates resolved into the shape the cost math wants.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelRates {
    /// Fresh input tokens.
    pub input_per_mtok: f64,
    /// Input tokens served from the prompt cache.
    pub cache_read_per_mtok: f64,
    /// Input tokens written to the prompt cache (five-minute tier).
    pub cache_write_per_mtok: f64,
    /// Input tokens written to the extended one-hour tier.
    pub cache_write_1h_per_mtok: f64,
    /// Output tokens.
    pub output_per_mtok: f64,
}

/// The user drop-in path inside a data home: `<data>/catalogs/models.json`.
#[must_use]
pub fn user_catalog_path(data_dir: &Path) -> PathBuf {
    data_dir.join("catalogs").join("models.json")
}

/// The data home a lookup reads a user catalog from, or `None` for the
/// embedded catalog alone.
///
/// `lca-core` sets this once per session from its own `data_dir`, so the
/// registry never guesses a home directory and a sandboxed host (or the SDK)
/// controls exactly which file wins. An unset cell is the embedded catalog
/// and nothing else, which is the right answer for a test.
static USER_DATA_DIR: OnceLock<Option<PathBuf>> = OnceLock::new();

/// Point the user-catalog lookup at `data_dir` for the life of the process.
///
/// The first call wins: the later ones are ignored, because a session's data
/// home does not change mid-run and a silently swapped catalog would make
/// costs change under the user. Returns whether this call set the home.
pub fn set_user_data_dir(data_dir: impl Into<PathBuf>) -> bool {
    USER_DATA_DIR.set(Some(data_dir.into())).is_ok()
}

/// The user catalog this process reads, if any.
#[must_use]
pub fn user_catalog() -> Option<&'static Catalog> {
    static USER: OnceLock<Option<Catalog>> = OnceLock::new();
    USER.get_or_init(|| {
        let path = USER_DATA_DIR.get()?.as_ref()?;
        parse_catalog_file(&user_catalog_path(path))
    })
    .as_ref()
}

fn parse_catalog_file(path: &Path) -> Option<Catalog> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// Parse the embedded catalog and the user drop-in now, on the caller's
/// thread, instead of on the first lookup.
///
/// The first [`lookup`] inflates and parses about 1.5 MB of JSON, which was
/// measured at ~460 ms on a warm machine. That is a one-off, but it is not
/// free in *where* it happens: a caller that prices its first turn pays it inside
/// that turn. A front end that resolves a model at startup should call this
/// once there, so the first turn's timing is predictable.
///
/// Idempotent and cheap after the first call (both catalogs are cached in
/// `OnceLock`s). A caller that never prices anything pays nothing.
pub fn warm() {
    let _ = embedded();
    let _ = user_catalog();
}

/// The embedded catalog, parsed once.
///
/// The bytes come from the build (`LCA_MODELS_CATALOG_GZIP`): a gzipped,
/// stripped `models.dev` document when the build had one, else the
/// checked-in stub, uncompressed. A document that does not parse yields an
/// empty catalog rather than a panic - a bad build input must not take a
/// running agent down, and the pricing path already treats "no entry" as
/// tokens-only.
#[must_use]
pub fn embedded() -> &'static Catalog {
    static EMBEDDED: OnceLock<Catalog> = OnceLock::new();
    EMBEDDED.get_or_init(|| match embedded_json() {
        // The same tolerant parse the user drop-in gets: one junk key in a
        // build artifact must not empty every price in the binary.
        Some(text) => parse_catalog(&text).unwrap_or_default(),
        None => Catalog::new(),
    })
}

fn embedded_json() -> Option<Vec<u8>> {
    let raw = include_bytes!(env!("LCA_MODELS_CATALOG_GZIP"));
    if raw.starts_with(&[0x1f, 0x8b]) {
        // Bounded: the strip's output is ~1.5 MiB today, and a corrupt
        // artifact must not hand the decompressor an unbounded budget. 64 MiB
        // is far past any real catalog and well short of a problem.
        use std::io::Read as _;
        let mut out = Vec::new();
        let decoder = flate2::read::GzDecoder::new(raw.as_slice());
        return decoder
            .take(64 * 1024 * 1024)
            .read_to_end(&mut out)
            .ok()
            .map(|_| out);
    }
    // The stub path: already stripped JSON, embedded verbatim.
    Some(raw.to_vec())
}

/// Look one model up in the effective catalog: the user drop-in first, then
/// the embedded catalog.
///
/// `provider` is the provider id the catalog keys on (`openai`, `groq`,
/// `opencode-go`); `model_id` is the id the provider reports. A `profile/` or
/// vendor prefix on `model_id` is tolerated, because the configured name and
/// the wire name differ for a routed preset (`openrouter/openai/gpt-4o`).
///
/// The result is owned, so the caller does not hold a lock or a borrow across
/// its own pricing math. An unknown model returns `None`, and the caller
/// records tokens only (acceptance 5's last step, "never a guess").
#[must_use]
pub fn lookup(provider: &str, model_id: &str) -> Option<ModelDetails> {
    let user = user_catalog();
    let embedded = embedded();
    lookup_in(user, embedded, provider, model_id)
}

/// The lookup, over explicit catalogs.
///
/// Split out so tests can drive it with fixture documents instead of whatever
/// the network or the user's home happens to hold (the brief's determinism
/// rule: a test never asserts against fetched data).
#[must_use]
pub fn lookup_in(
    user: Option<&Catalog>,
    embedded: &Catalog,
    provider: &str,
    model_id: &str,
) -> Option<ModelDetails> {
    if model_id.is_empty() {
        return None;
    }
    for catalog in [user, Some(embedded)].into_iter().flatten() {
        if let Some(found) = lookup_in_one(catalog, provider, model_id) {
            return Some(found);
        }
    }
    None
}

fn lookup_in_one(catalog: &Catalog, provider: &str, model_id: &str) -> Option<ModelDetails> {
    let bare = bare_id(model_id);
    if let Some(entry) = catalog.get(provider) {
        // Exact id first: a reseller's own name is what the user configured.
        if let Some(details) = entry.models.get(model_id) {
            return Some(details.clone());
        }
        if bare != model_id
            && let Some(details) = entry.models.get(bare)
        {
            return Some(details.clone());
        }
        // models.dev qualifies most ids with a vendor inside a routed
        // provider (`openrouter` lists `openai/gpt-4o`), so a bare id matches
        // the row whose suffix is that id.
        if let Some((_, details)) = entry
            .models
            .iter()
            .find(|(id, _)| id.rsplit('/').next() == Some(bare))
        {
            return Some(details.clone());
        }
    }
    canonical_fallback(catalog, bare)
}

/// The issue's step 2: any provider entry with that bare id. A row whose
/// `canonical_model_id` names the model wins over one that merely shares the
/// id, so `gpt-4o` prices from a row that says it *is* `openai/gpt-4o`.
///
/// Deterministic by construction: providers are visited in sorted key order,
/// so an id listed on twenty resellers always resolves to the same row.
/// `HashMap` iteration order is per-process, and a cost that changes between
/// two runs of the same binary would be a defect nobody could reproduce.
fn canonical_fallback(catalog: &Catalog, bare: &str) -> Option<ModelDetails> {
    let mut providers: Vec<&String> = catalog.keys().collect();
    providers.sort();
    let mut fallback = None;
    for provider in providers {
        let models = &catalog[provider].models;
        let mut ids: Vec<&String> = models.keys().collect();
        ids.sort();
        for id in ids {
            if id != bare && id.rsplit('/').next() != Some(bare) {
                continue;
            }
            let details = &models[id];
            if details
                .canonical_model_id
                .as_deref()
                .is_some_and(|canonical| {
                    canonical == bare || canonical.ends_with(&format!("/{bare}"))
                })
            {
                return Some(details.clone());
            }
            fallback.get_or_insert_with(|| details.clone());
        }
    }
    fallback
}

/// A model id with any `profile/` or vendor prefix removed.
fn bare_id(model_id: &str) -> &str {
    model_id.rsplit('/').next().unwrap_or(model_id)
}

/// Parse a catalog document - the embedded form, a user drop-in, or a raw
/// `models.dev/api.json` - into [`Catalog`] (acceptance 3).
///
/// Unknown *fields* are ignored by Serde, so the same call handles a full
/// 5.6 MB document and a hand-written three-model file.
///
/// A provider entry that is not an object is skipped rather than failing the
/// whole document, because a hand-written drop-in will carry comments
/// (`"_comment": "..."`) and one stray key must not silently delete every
/// other price the user wrote. A document that is not JSON at all, or that
/// carries no provider entry, is still an error: the caller needs to know its
/// file is junk.
pub fn parse_catalog(text: &[u8]) -> Result<Catalog, serde_json::Error> {
    let document: HashMap<String, serde_json::Value> = serde_json::from_slice(text)?;
    let catalog: Catalog = document
        .into_iter()
        .filter(|(_, value)| value.is_object())
        .filter_map(|(provider, value)| {
            serde_json::from_value(value)
                .ok()
                .map(|entry| (provider, entry))
        })
        .collect();
    Ok(catalog)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fixture catalog shaped like the stripped artifact.
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
                    "llama-3.3-70b-versatile": {
                        "cost": {"input": 0.59, "output": 0.79}
                    }
                }},
                "openrouter": {"models": {
                    "openai/gpt-4o": {
                        "canonical_model_id": "openai/gpt-4o",
                        "cost": {"input": 2.5, "output": 10.0, "cache_read": 1.25}
                    }
                }}
            }"#,
        )
        .expect("fixture parses")
    }

    // Verifies: gh #260 acceptance 1 - the schema is the issue's, field for
    // field, and a partially specified row parses (a user drop-in names only
    // what it wants to override).
    #[test]
    fn the_schema_parses_the_stripped_shape_and_partial_rows() {
        let catalog = fixture();
        let gpt = catalog["openai"].models["gpt-4o"].clone();
        assert_eq!(gpt.canonical_model_id.as_deref(), Some("openai/gpt-4o"));
        assert_eq!(gpt.cost.as_ref().and_then(|c| c.input), Some(2.5));
        assert_eq!(gpt.context_window(), Some(128_000));
        assert_eq!(gpt.max_output_tokens(), Some(16_384));
        assert!(!gpt.reasoning);

        // A row with only a rate parses: every field is optional.
        let partial = catalog["groq"].models["llama-3.3-70b-versatile"].clone();
        assert!(partial.limit.is_none());
        assert_eq!(partial.cost.as_ref().and_then(|c| c.output), Some(0.79));
    }

    // Verifies: gh #260 acceptance 3 - a raw full `models.dev/api.json`
    // document parses with the same structs, extra fields silently ignored.
    #[test]
    fn a_raw_full_document_parses_and_ignores_extra_fields() {
        let raw = br#"{
            "openai": {
                "id": "openai", "name": "OpenAI", "env": ["OPENAI_API_KEY"],
                "npm": "@ai-sdk/openai", "doc": "https://platform.openai.com",
                "models": {
                    "gpt-4o": {
                        "id": "gpt-4o", "name": "GPT-4o",
                        "description": "A long marketing string",
                        "attachment": true, "reasoning": false,
                        "tool_call": true, "temperature": true,
                        "knowledge": "2024-06", "release_date": "2024-05-13",
                        "last_updated": "2024-08-06",
                        "modalities": {"input": ["text", "image"], "output": ["text"]},
                        "open_weights": false,
                        "limit": {"context": 128000, "output": 16384},
                        "cost": {"input": 2.5, "output": 10, "cache_read": 1.25}
                    }
                }
            }
        }"#;
        let catalog = parse_catalog(raw).expect("a raw document parses");
        let gpt = &catalog["openai"].models["gpt-4o"];
        assert_eq!(gpt.context_window(), Some(128_000));
        assert_eq!(gpt.rates().map(|r| r.input_per_mtok), Some(2.5));
    }

    // Verifies: gh #260 acceptance 3 - one struct set for both documents: a
    // stripped row and the raw row it came from land on the same details.
    #[test]
    fn the_stripped_and_raw_shapes_agree() {
        let stripped = parse_catalog(
            br#"{"openai":{"models":{"gpt-4o":{"canonical_model_id":null,
                "cost":{"input":2.5,"output":10.0,"cache_read":1.25},
                "limit":{"context":128000,"output":16384},"reasoning":false}}}}"#,
        )
        .expect("stripped parses");
        let raw = parse_catalog(
            br#"{"openai":{"name":"OpenAI","models":{"gpt-4o":{"name":"GPT-4o",
                "description":"x","cost":{"input":2.5,"output":10.0,"cache_read":1.25},
                "limit":{"context":128000,"output":16384,"input":100},"reasoning":false}}}}"#,
        )
        .expect("raw parses");
        assert_eq!(
            stripped["openai"].models["gpt-4o"], raw["openai"].models["gpt-4o"],
            "the strip removes only fields nobody reads"
        );
    }

    // Verifies: gh #260 acceptance 3 - a bad document is an error, not an
    // empty catalog, so a user's typo'd drop-in is visible.
    #[test]
    fn a_malformed_document_is_an_error() {
        assert!(parse_catalog(b"{not json").is_err());
        assert!(
            parse_catalog(b"[]").is_err(),
            "an array is not a catalog document"
        );
    }

    // Verifies: gh #260 - one non-object key does not delete the rest. A
    // hand-written drop-in with a comment must keep the rates beside it.
    #[test]
    fn a_comment_key_does_not_discard_the_catalog() {
        let catalog = parse_catalog(
            br#"{"_comment": "my rates", "openai": {"models": {"gpt-4o":
                {"cost": {"input": 9.0, "output": 90.0}}}}}"#,
        )
        .expect("the document parses");
        assert_eq!(
            catalog["openai"].models["gpt-4o"]
                .cost
                .as_ref()
                .and_then(|cost| cost.input),
            Some(9.0),
            "the comment key is skipped, not fatal"
        );
    }

    // Verifies: gh #260 - exact provider match beats the canonical fallback:
    // the same model on two services carries two prices, and the configured
    // provider's row is the one that bills.
    #[test]
    fn exact_provider_match_wins_over_the_canonical_fallback() {
        let embedded = fixture();
        let groq =
            lookup_in(None, &embedded, "groq", "llama-3.3-70b-versatile").expect("groq lists it");
        assert_eq!(groq.cost.as_ref().and_then(|c| c.input), Some(0.59));

        let openai = lookup_in(None, &embedded, "openai", "gpt-4o").expect("openai lists it");
        assert_eq!(openai.cost.as_ref().and_then(|c| c.input), Some(2.5));
    }

    // Verifies: gh #260 - the canonical fallback: an unlisted provider still
    // prices a model another provider lists, preferring the row that names it
    // canonically.
    #[test]
    fn an_unlisted_provider_falls_back_to_a_canonical_row() {
        let embedded = fixture();
        let found = lookup_in(None, &embedded, "some-reseller", "gpt-4o")
            .expect("the canonical fallback finds it");
        assert_eq!(found.cost.as_ref().and_then(|c| c.input), Some(2.5));
        assert_eq!(found.context_window(), Some(128_000));
    }

    // Verifies: gh #260 - a `profile/` prefix is tolerated on the model id.
    #[test]
    fn a_qualified_model_id_matches_the_bare_row() {
        let embedded = fixture();
        let found =
            lookup_in(None, &embedded, "openai", "profile/gpt-4o").expect("the prefix is stripped");
        assert_eq!(found.cost.as_ref().and_then(|c| c.input), Some(2.5));
    }

    // Verifies: gh #260 acceptance 5 - the user drop-in beats the embedded
    // catalog, and an entry it does not carry falls through to it.
    #[test]
    fn the_user_catalog_overrides_the_embedded_one() {
        let embedded = fixture();
        let user = parse_catalog(
            br#"{"openai":{"models":{"gpt-4o":{"cost":{"input":9.0,"output":90.0}}}}}"#,
        )
        .expect("user catalog parses");
        let found = lookup_in(Some(&user), &embedded, "openai", "gpt-4o").expect("listed");
        assert_eq!(found.cost.as_ref().and_then(|c| c.input), Some(9.0));
        assert!(
            found.limit.is_none(),
            "the user's partial row is what is returned, not a merge"
        );
        // A model the user's file does not mention still resolves.
        assert!(
            lookup_in(Some(&user), &embedded, "groq", "llama-3.3-70b-versatile").is_some(),
            "the embedded catalog still answers"
        );
    }

    // Verifies: gh #260 acceptance 5's last step - never a guess. An unknown
    // model and an empty id both return `None`.
    #[test]
    fn an_unknown_model_returns_none() {
        let embedded = fixture();
        assert!(lookup_in(None, &embedded, "openai", "not-a-model").is_none());
        assert!(
            lookup_in(None, &embedded, "not-a-provider", "gpt-4o").is_some(),
            "canonical"
        );
        assert!(lookup_in(None, &embedded, "openai", "").is_none());
    }

    // Verifies: gh #274's fallbacks survive the move - a missing cache rate
    // bills at the input rate, and the one-hour tier is 2x input.
    #[test]
    fn absent_cache_rates_fall_back_and_the_one_hour_tier_is_twice_input() {
        let details = ModelDetails {
            cost: Some(ModelCost {
                input: Some(3.0),
                output: Some(15.0),
                cache_read: None,
                cache_write: Some(3.75),
            }),
            ..ModelDetails::default()
        };
        let rates = details.rates().expect("rates");
        assert_eq!(rates.cache_read_per_mtok, 3.0, "absent falls back to input");
        assert_eq!(rates.cache_write_per_mtok, 3.75);
        assert_eq!(rates.cache_write_1h_per_mtok, 6.0, "2x input");

        // A model with no output rate has nothing to bill from.
        let incomplete = ModelDetails {
            cost: Some(ModelCost {
                input: Some(1.0),
                output: None,
                cache_read: None,
                cache_write: None,
            }),
            ..ModelDetails::default()
        };
        assert!(incomplete.rates().is_none());
    }

    // Verifies: gh #260 - a `0` limit means "not applicable" (models.dev's
    // embedding rows), and it must read as unknown, never as a real window.
    #[test]
    fn a_zero_limit_reads_as_unknown() {
        let details = ModelDetails {
            limit: Some(ModelLimit {
                context: Some(0),
                output: Some(0),
            }),
            ..ModelDetails::default()
        };
        assert_eq!(details.context_window(), None);
        assert_eq!(details.max_output_tokens(), None);
    }

    // Verifies: gh #260 acceptance 1/6 - the embedded catalog parses, and the
    // build always produced *something* to parse (the stub when offline).
    #[test]
    fn the_embedded_catalog_parses() {
        let catalog = embedded();
        assert!(
            !catalog.is_empty(),
            "the build embeds at least the stub catalog"
        );
        assert!(
            catalog.values().any(|entry| !entry.models.is_empty()),
            "the embedded catalog carries models"
        );
    }
}
