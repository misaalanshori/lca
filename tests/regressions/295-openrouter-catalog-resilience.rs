//! GitHub #295: `discover_models` aborted the whole catalog read when a
//! single entry lacked a string `id`.
//!
//! The loop did `let id = entry.get("id")?.as_str()?;`. That `?` returns
//! from `discover_models`, not from the iteration, so one non-conforming row
//! — an experimental or deprecated model with `{"id": null}`, a
//! rate-limit or deprecation notice shaped `{"message": "..."}`, any entry
//! whose `id` is a number — discarded the entire payload and returned `None`.
//! The caller then took the documented fallback: the preset's **three**
//! curated rows. So an OpenRouter user with a 300-model catalog saw three,
//! and nothing said why.
//!
//! The harm is disproportionate in a way that matters: the trigger is a row
//! the user neither controls nor sees, and the effect is a silent downgrade
//! of the one list the picker offers. A gateway is allowed to add rows; this
//! reader has to be allowed to ignore one.
//!
//! The guard below pins both halves — the good rows survive, and the drop is
//! counted rather than swallowed — plus the degradation row that keeps
//! "resilient" from turning into "empty picker".
//!
//! Verifies: NFR-24 (a shipped defect's guard), GitHub issue #295,
//! gh #295 acceptance 1–3.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use lca_protocol::{CapabilityError, ModelInfo, ProviderCap};

/// A capability whose `/models` answer is this test's payload.
struct CatalogCap {
    body: String,
    status: u16,
    /// Per-instance: a shared `static` here would serve only the first test's
    /// payload and silently starve every later one.
    sent: std::sync::atomic::AtomicBool,
}

impl CatalogCap {
    fn serving(body: &str) -> CatalogCap {
        CatalogCap {
            body: body.to_string(),
            status: 200,
            sent: std::sync::atomic::AtomicBool::new(false),
        }
    }

    fn failing(status: u16) -> CatalogCap {
        CatalogCap {
            body: "{}".to_string(),
            status,
            sent: std::sync::atomic::AtomicBool::new(false),
        }
    }
}

impl ProviderCap for CatalogCap {
    fn net_request(
        &self,
        _method: &str,
        _url: &str,
        _headers: &[(&str, &str)],
        _body: Option<&[u8]>,
    ) -> Result<u32, CapabilityError> {
        Ok(1)
    }

    fn net_response_status(&self, _handle: u32) -> Result<u16, CapabilityError> {
        Ok(self.status)
    }

    fn net_read_body(&self, _handle: u32, _max: usize) -> Result<Option<Vec<u8>>, CapabilityError> {
        // One chunk then end, like the real reader.
        use std::sync::atomic::Ordering;
        if self.sent.swap(true, Ordering::SeqCst) {
            return Ok(None);
        }
        Ok(Some(self.body.clone().into_bytes()))
    }

    fn net_close_response(&self, _handle: u32) -> Result<(), CapabilityError> {
        Ok(())
    }

    fn credentials_get(&self, key: &str) -> Option<String> {
        (key == "access").then(|| "sk-or-mock".to_string())
    }

    fn credentials_set(&self, _key: &str, _value: &str) -> Result<(), CapabilityError> {
        Ok(())
    }

    fn credentials_delete(&self, _key: &str) -> Result<(), CapabilityError> {
        Ok(())
    }
}

/// Three good rows with `bad` spliced into the middle.
fn catalog_with(bad: &str) -> String {
    format!(
        r#"{{"data":[
          {{"id":"openai/gpt-4o","context_length":128000}},
          {bad},
          {{"id":"mock/test-model","context_length":32000}},
          {{"id":"anthropic/claude-3.5-sonnet","context_length":200000}}
        ]}}"#
    )
}

fn ids(models: &[ModelInfo]) -> Vec<&str> {
    models.iter().map(|model| model.id.as_str()).collect()
}

const GOOD: [&str; 3] = [
    "openai/gpt-4o",
    "mock/test-model",
    "anthropic/claude-3.5-sonnet",
];

/// The issue's own row: `{"id": null}` among valid entries. Pre-fix this
/// returned the three curated rows; the payload's models were all lost.
#[test]
fn a_single_null_id_does_not_discard_the_catalog() {
    let cap = CatalogCap::serving(&catalog_with(r#"{"id":null}"#));
    let (models, dropped) = openrouter::discover_report(&cap);
    assert_eq!(ids(&models), GOOD, "every good row survives one bad one");
    assert_eq!(dropped, 1, "and the drop is counted");
}

/// A notice-shaped entry — no `id`, a `message` instead. Gateways send
/// these for deprecations and advisories, which is why this is the shape
/// most likely to appear in a real payload.
#[test]
fn a_notice_entry_is_skipped_not_fatal() {
    let cap = CatalogCap::serving(&catalog_with(r#"{"message":"deprecated 2026-11"}"#));
    let (models, dropped) = openrouter::discover_report(&cap);
    assert_eq!(ids(&models), GOOD);
    assert_eq!(dropped, 1);
}

/// Every other non-conforming shape is skipped by the same rule. Listed
/// together so a future "tighten the id check" change has to face them.
#[test]
fn every_non_conforming_id_shape_is_skipped() {
    for bad in [
        r#"{"id":null}"#,
        r#"{"id":""}"#,
        r#"{"id":42}"#,
        r#"{"id":["a"]}"#,
        r#"{"id":{"nested":true}}"#,
        r#"{"id":true}"#,
        r#"{}"#,
    ] {
        let cap = CatalogCap::serving(&catalog_with(bad));
        let (models, dropped) = openrouter::discover_report(&cap);
        assert_eq!(ids(&models), GOOD, "skipped cleanly: {bad}");
        assert_eq!(dropped, 1, "counted: {bad}");
    }
}

/// The degradation row: a payload where every entry is bad still takes the
/// documented curated fallback. "Resilient" must not mean "empty picker",
/// and this is the row that keeps the fix from overshooting.
#[test]
fn a_wholly_malformed_catalog_still_falls_back() {
    let cap = CatalogCap::serving(r#"{"data":[{"id":null},{"message":"no"},{"id":""}]}"#);
    let (models, dropped) = openrouter::discover_report(&cap);
    assert!(models.is_empty(), "nothing usable: {models:?}");
    assert_eq!(dropped, 3, "all three are accounted for");
    // And `list_models` still answers with the curated floor.
    let listed = openrouter::list_models(&cap);
    assert_eq!(
        ids(&listed),
        vec![
            "openai/gpt-4o",
            "anthropic/claude-3.5-sonnet",
            "google/gemini-2.0-flash",
        ],
        "the fallback is unchanged"
    );
}

/// A non-array `data` and a failing status both keep the pre-fix
/// behaviour: no rows, no skip count, fallback. These are read failures,
/// not partial answers, and must not be reported as skips.
#[test]
fn a_read_failure_is_not_reported_as_a_skip() {
    for cap in [
        CatalogCap::serving(r#"{"data":{"id":"x"}}"#),
        CatalogCap::serving("not json at all"),
        CatalogCap::failing(500),
        CatalogCap::failing(401),
    ] {
        let (models, dropped) = openrouter::discover_report(&cap);
        assert!(models.is_empty(), "{models:?}");
        assert_eq!(dropped, 0, "a failed read skipped nothing");
        assert_eq!(openrouter::list_models(&cap).len(), 3, "curated floor");
    }
}

/// Order is the gateway's. A skipped row must not reorder the survivors,
/// or the picker shuffles between refreshes for no reason the user can see.
#[test]
fn a_skipped_row_does_not_reorder_the_survivors() {
    let bad_first = r#"{"data":[{"id":null},{"id":"a"},{"id":"b"},{"id":"c"}]}"#;
    let cap = CatalogCap::serving(bad_first);
    let (models, _) = openrouter::discover_report(&cap);
    assert_eq!(ids(&models), vec!["a", "b", "c"], "gateway order");
}

/// The drop is observable through `list_models` too, which is the entry
/// point the host actually calls: the sentence both modes emit names the
/// counts, so "returned 40 of 300 and said nothing" cannot recur.
#[test]
fn the_partial_answer_is_reportable() {
    assert_eq!(
        openrouter::skip_message(1, 40),
        "openrouter: skipped 1 malformed catalog entry; 40 models usable"
    );
    assert_eq!(
        openrouter::skip_message(2, 1),
        "openrouter: skipped 2 malformed catalog entries; 1 model usable"
    );
    // And `list_models` keeps the good rows rather than falling back.
    let cap = CatalogCap::serving(&catalog_with(r#"{"id":null}"#));
    assert_eq!(ids(&openrouter::list_models(&cap)), GOOD);
}

/// The notice is machine-visible, not just a log line: it rides the first
/// row's documented `extras`, which is the one channel that crosses the ABI
/// in both delivery modes. A log message would be a dead write in the guest
/// (`captured_logs` is a test accessor the host never drains).
#[test]
fn the_skip_notice_crosses_the_boundary_on_the_first_row() {
    let cap = CatalogCap::serving(&catalog_with(r#"{"id":null}"#));
    let rows = openrouter::list_models(&cap);
    assert_eq!(
        rows[0]
            .extras
            .get(openrouter::SKIPPED_EXTRA)
            .map(String::as_str),
        Some("1"),
        "the count is on the first row: {:?}",
        rows[0].extras
    );
    assert!(
        rows[1..]
            .iter()
            .all(|row| !row.extras.contains_key(openrouter::SKIPPED_EXTRA)),
        "attached once, not per row"
    );
}

/// And a clean catalog carries nothing, so the marker means what it says.
#[test]
fn a_clean_catalog_carries_no_notice() {
    let cap = CatalogCap::serving(r#"{"data":[{"id":"a"},{"id":"b"}]}"#);
    let rows = openrouter::list_models(&cap);
    assert!(
        rows.iter()
            .all(|row| !row.extras.contains_key(openrouter::SKIPPED_EXTRA))
    );
}

/// The context window keeps its tolerant handling: a present
/// `context_length` rides, a missing or non-numeric one is an unknown (0)
/// window rather than a rejection. gh #34's ground-truth rule, restated
/// here because the fix touched this loop.
#[test]
fn the_window_stays_tolerant_after_the_fix() {
    let cap = CatalogCap::serving(
        r#"{"data":[
          {"id":"with","context_length":8192},
          {"id":"without"},
          {"id":"weird","context_length":"big"},
          {"id":"negative","context_length":-1}
        ]}"#,
    );
    let (models, dropped) = openrouter::discover_report(&cap);
    assert_eq!(dropped, 0, "none of these are malformed ids");
    let windows: Vec<u32> = models.iter().map(|m| m.context_window).collect();
    assert_eq!(
        windows,
        vec![8192, 0, 0, 0],
        "present window rides; absent or unusable is unknown"
    );
}
