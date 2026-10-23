//! Ranking rows for `/model` search (gh #396 gap 1).
//!
//! The proxy-ranked-first row is the issue's defect and is written to be red
//! against the old `contains()`-on-label filter: with that filter the proxied
//! entry came first because its label contained the direct entry's whole name.

use super::*;

/// A direct entry and the proxy that resells it - the issue's own example.
///
/// **The proxy comes first in catalog order**, which is what makes this row a real
/// test: `contains` could not reorder, so it returned the proxy first simply because
/// the catalog listed it first. The fix has to *move* the direct entry up.
fn direct_and_proxy() -> Vec<ModelEntry> {
    vec![
        // The proxy's id already carries the provider path, which is what made
        // `contains` unable to separate them.
        ModelEntry {
            id: "openai/gpt-5".to_string(),
            label: "openai/gpt-5 (openrouter)".to_string(),
            provider: "openrouter".to_string(),
        },
        ModelEntry {
            id: "gpt-5".to_string(),
            label: "gpt-5 (openai)".to_string(),
            provider: "openai".to_string(),
        },
    ]
}

// Verifies: gh #396 gap 1's headline defect - **the direct model outranks the
// proxy for a query that names it.** `openrouter` resells `openai/gpt-5`; a user
// typing `gpt-5` means the model, not the reseller.
#[test]
fn the_direct_model_outranks_the_proxy_that_resells_it() {
    let entries = direct_and_proxy();
    let ranked = rank(&entries, "gpt-5");
    assert_eq!(ranked.len(), 2, "both match: {ranked:?}");
    assert_eq!(
        entries[ranked[0]].provider,
        "openai",
        "the direct entry ranks first, not the proxied one - and the proxy was \
         listed first, so the fix had to move it: {:?}",
        ranked
            .iter()
            .map(|i| (&entries[*i].id, &entries[*i].provider))
            .collect::<Vec<_>>()
    );
}

// Verifies: gh #396 rule 1 - **a provider query finds the direct entry.** pi's
// `getModelSelectorSearchText` puts the provider at position 0 for exactly this
// reason: the proxied entry only contains `openai` later in a longer string.
#[test]
fn a_provider_query_ranks_the_providers_own_entry_first() {
    let entries = vec![
        ModelEntry {
            id: "gpt-5".to_string(),
            label: "gpt-5 (openrouter)".to_string(),
            provider: "openrouter".to_string(),
        },
        ModelEntry {
            id: "gpt-5".to_string(),
            label: "gpt-5 (openai)".to_string(),
            provider: "openai".to_string(),
        },
    ];
    let ranked = rank(&entries, "openai");
    assert_eq!(
        entries[ranked[0]].provider, "openai",
        "the named provider's entry ranks first"
    );
}

// Verifies: gh #396 gap 1's second half - **typo tolerance.** `4ogpt` is `gpt-4o`
// with its runs transposed, which `contains` cannot see. The tolerance is pi's
// `fuzzyMatch` alpha/digit swap: one run of letters and one run of digits in
// either order are treated as equivalent, so `4ogpt` finds `gpt-4o`.
#[test]
fn a_transposed_query_still_finds_the_model() {
    let entries = vec![
        ModelEntry {
            id: "gpt-4o".to_string(),
            label: "gpt-4o (openai)".to_string(),
            provider: "openai".to_string(),
        },
        ModelEntry {
            id: "claude-sonnet".to_string(),
            label: "claude-sonnet (anthropic)".to_string(),
            provider: "anthropic".to_string(),
        },
    ];
    let ranked = rank(&entries, "4ogpt");
    assert_eq!(
        ranked.len(),
        1,
        "the transposed query finds exactly the model it means: {:?}",
        ranked.iter().map(|i| &entries[*i].id).collect::<Vec<_>>()
    );
    assert_eq!(entries[ranked[0]].id, "gpt-4o");
}

// Verifies: gh #396 rule 3 - **a multi-token query matches across the fields.**
// `4o gpt` is `gpt-4o` written in two tokens, and the tokens must be ANDed: a
// model matching only one of them is not a match.
#[test]
fn a_multi_token_query_matches_across_the_identity() {
    let entries = vec![
        ModelEntry {
            id: "gpt-4o".to_string(),
            label: "gpt-4o (openai)".to_string(),
            provider: "openai".to_string(),
        },
        ModelEntry {
            id: "gpt-5".to_string(),
            label: "gpt-5 (openai)".to_string(),
            provider: "openai".to_string(),
        },
        ModelEntry {
            id: "claude-sonnet".to_string(),
            label: "claude-sonnet (anthropic)".to_string(),
            provider: "anthropic".to_string(),
        },
    ];
    let ranked = rank(&entries, "4o gpt");
    let ids: Vec<&str> = ranked.iter().map(|i| entries[*i].id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["gpt-4o"],
        "both tokens must match, and only gpt-4o has both: {ids:?}"
    );
}

// Verifies: gh #396 rule 6 - **the no-regression row.** `gpt-4` must still find
// `openai/gpt-4o` near the top; a ranking that made the common case worse would
// trade one defect for a bigger one.
#[test]
fn a_plain_prefix_query_still_finds_the_model_near_the_top() {
    let entries = vec![
        ModelEntry {
            id: "openai/gpt-4o".to_string(),
            label: "openai/gpt-4o (openai)".to_string(),
            provider: "openai".to_string(),
        },
        ModelEntry {
            id: "openai/gpt-4o-mini".to_string(),
            label: "openai/gpt-4o-mini (openai)".to_string(),
            provider: "openai".to_string(),
        },
        ModelEntry {
            id: "claude-sonnet".to_string(),
            label: "claude-sonnet (anthropic)".to_string(),
            provider: "anthropic".to_string(),
        },
    ];
    let ranked = rank(&entries, "gpt-4");
    assert!(!ranked.is_empty(), "`gpt-4` finds the gpt-4 family");
    let top = &entries[ranked[0]];
    assert!(
        top.id.contains("gpt-4"),
        "the top result is a gpt-4 model: {top:?}"
    );
    // And the two gpt-4s come before the unrelated model.
    assert_eq!(
        ranked.len(),
        2,
        "only the gpt-4 family matches: {:?}",
        ranked.iter().map(|i| &entries[*i].id).collect::<Vec<_>>()
    );
}

// Verifies: gh #396 rule 4 - **the unfiltered order is defined**: current, then
// default, then alphabetical. Not whatever order the catalog arrived in.
#[test]
fn the_unfiltered_order_is_current_then_default_then_alphabetical() {
    let entries = vec![
        ModelEntry::without_provider("zeta", "zeta (z)"),
        ModelEntry::without_provider("dflt", "dflt (d)"),
        ModelEntry::without_provider("cur", "cur (c)"),
        ModelEntry::without_provider("alpha", "alpha (a)"),
    ];
    let ordered = default_order(&entries, Some("cur"), Some("dflt"));
    let ids: Vec<&str> = ordered.iter().map(|i| entries[*i].id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["cur", "dflt", "alpha", "zeta"],
        "current, then default, then alphabetical"
    );
}

// Verifies: gh #396 rule 4's edge - **one entry can be both current and
// default**, and it must appear once, first, not twice.
#[test]
fn an_entry_that_is_both_current_and_default_appears_once_first() {
    let entries = vec![
        ModelEntry::without_provider("alpha", "alpha"),
        ModelEntry::without_provider("same", "same"),
    ];
    let ordered = default_order(&entries, Some("same"), Some("same"));
    assert_eq!(
        ordered,
        vec![1, 0],
        "the entry is current, so it leads, and appears once"
    );
}

// Verifies: gh #396 rule 4's other edge - **a `provider/id` active cell still
// matches a bare id.** The picker showed no `✓` for a qualified active model
// before this, because the comparison was string equality.
#[test]
fn the_current_marker_matches_both_id_spellings() {
    let entries = vec![
        ModelEntry::without_provider("gpt-5", "gpt-5"),
        ModelEntry::without_provider("other", "other"),
    ];
    // Active cell holds `openai/gpt-5`; the row's id is bare.
    let ordered = default_order(&entries, Some("openai/gpt-5"), None);
    assert_eq!(ordered[0], 0, "the bare id row is recognised as current");
    // And the reverse: the cell holds the bare id, the row is qualified.
    let entries2 = vec![
        ModelEntry::without_provider("openai/gpt-5", "openai/gpt-5"),
        ModelEntry::without_provider("other", "other"),
    ];
    let ordered2 = default_order(&entries2, Some("gpt-5"), None);
    assert_eq!(ordered2[0], 0, "the qualified row is recognised as current");
}

// Verifies: an empty query returns the caller's order untouched, so the
// unfiltered rule is not re-sorted into catalog order by the ranker.
#[test]
fn an_empty_query_preserves_the_callers_order() {
    let entries = vec![
        ModelEntry::without_provider("b", "b"),
        ModelEntry::without_provider("a", "a"),
    ];
    assert_eq!(rank(&entries, ""), vec![0, 1]);
    assert_eq!(rank(&entries, "   "), vec![0, 1]);
}

// Verifies: pi's identity shape - **provider first**, so a provider query scores
// at position 0 for the real entry rather than later inside a proxy path.
#[test]
fn the_search_text_leads_with_the_provider() {
    let entry = ModelEntry {
        id: "gpt-5".to_string(),
        label: "gpt-5 (openai)".to_string(),
        provider: "openai".to_string(),
    };
    let text = entry.search_text();
    assert!(text.starts_with("openai"), "the provider leads: {text}");
    assert!(
        text.contains("openai/gpt-5"),
        "and the qualified form: {text}"
    );
    // An entry with no provider searches on its id alone, with no stray space.
    let bare = ModelEntry::without_provider("gpt-5", "gpt-5");
    assert_eq!(bare.search_text(), "gpt-5");
}

// Verifies: a query with no match returns nothing, rather than everything.
#[test]
fn a_query_matching_nothing_returns_no_rows() {
    let entries = direct_and_proxy();
    assert!(rank(&entries, "zzzzzz").is_empty());
}
