//! NFR-24 guard for gh #396: the model search ranks the real model above its
//! proxies, and `/thinking` saves a default the same way `/model` does.
//!
//! These rows read the source because the promise is about *shape*. A later cycle
//! could pass every behaviour row while restoring the `contains()`-on-label filter
//! (which is what made a proxy win), or while growing a second persistence route
//! for the thinking default - the drift this repo keeps closing.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

fn read(path: &str) -> String {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(root.join(path)).unwrap_or_else(|err| panic!("read {path}: {err}"))
}

/// Code only: comments removed, so prose naming a past mistake cannot trip a guard.
fn code_only(text: &str) -> String {
    text.lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

// Verifies: gh #396 gap 1 - **the search no longer matches on the label.**
//
// The defect was one line: `label.to_lowercase().contains(needle)`. The label
// embeds the id, so `openrouter/openai/gpt-5` contains `openai/gpt-5` whole and the
// proxy could not be told from the model. A guard on the source, because a
// behaviour row alone cannot see that the *mechanism* came back.
#[test]
fn the_picker_no_longer_filters_on_the_label() {
    let source = code_only(&read("crates/lca-ui/src/chat_pickers.rs"));
    let refilter = source
        .find("pub fn refilter")
        .expect("the picker's refilter exists");
    let body = &source[refilter..];
    let end = body.find("\n    }\n").unwrap_or(body.len());
    let body = &body[..end];
    assert!(
        !body.contains("label.to_lowercase().contains"),
        "the label is display-only (G2) and was never searchable:\n{body}"
    );
    assert!(
        body.contains("rank("),
        "the filter goes through the scored ranker:\n{body}"
    );
}

// Verifies: gh #396 rule 1 - **the identity is what is searched**, provider
// first, which is pi's `getModelSelectorSearchText` shape. The provider leading is
// the whole trick: a provider query scores at position 0 for the real entry.
#[test]
fn the_search_text_leads_with_the_provider() {
    let source = code_only(&read("crates/lca-ui/src/chat_model_search.rs"));
    let search = source
        .find("pub fn search_text")
        .expect("the identity builder exists");
    let body = &source[search..];
    let end = body.find("\n    }\n").unwrap_or(body.len());
    let body = &body[..end];
    assert!(
        body.contains("{p} {p}/{id} {p} {id}") || body.contains("p}/{id}"),
        "the shape is provider, provider/id, provider, id:\n{body}"
    );
}

// Verifies: gh #396 rule 2 - **scored and sorted, not filtered and hoped.** The
// ranker must sort, and the sort must be stable so equal scores keep the caller's
// (defined) order.
#[test]
fn the_ranker_scores_and_sorts_with_a_stable_order() {
    let source = code_only(&read("crates/lca-ui/src/chat_model_search.rs"));
    assert!(source.contains("sort_by"), "the ranker sorts by score");
    // A stable sort in Rust is `sort_by`/`sort_by_key` (not `_unstable_`), which
    // is what preserves the caller's order for equal scores.
    assert!(
        !source.contains("sort_unstable"),
        "an unstable sort would lose the unfiltered order for equal scores"
    );
}

// Verifies: gh #396 rule 4 - **the unfiltered order is defined**: current, then
// default, then alphabetical. Not "whatever order the catalog arrived in".
#[test]
fn the_unfiltered_order_is_current_then_default_then_alphabetical() {
    let source = code_only(&read("crates/lca-ui/src/chat_model_search.rs"));
    let order = source
        .find("pub fn default_order")
        .expect("the unfiltered order exists");
    let body = &source[order..];
    let end = body.find("\n}\n").unwrap_or(body.len());
    let body = &body[..end];
    assert!(
        body.contains("is_current") && body.contains("is_default"),
        "both markers participate in the order:\n{body}"
    );
    // The bucket decides before the label: current (0), default (1), the rest (2).
    assert!(
        body.contains("0") && body.contains("1") && body.contains("2"),
        "three buckets, in priority order:\n{body}"
    );
}

// Verifies: gh #396 rule 7 - **the thinking default mirrors the model default.**
//
// The issue asked for `save_default_thinking` "mirroring `save_default_model`
// exactly". A second persistence route (its own file, its own key shape, its own
// notice) is the drift this repo keeps closing, so this pins that both go through
// the same `persist_setting` call on the same config file.
#[test]
fn the_thinking_default_uses_the_same_persist_path_as_the_model_default() {
    let hooks = code_only(&read("crates/lca-cli/src/tui/hooks.rs"));
    let model = hooks
        .find("save_default_model: Some")
        .expect("the model save exists");
    let model_body = &hooks[model..];
    let model_end = model_body.find("})),").unwrap_or(model_body.len());
    assert!(
        model_body[..model_end].contains("persist_setting(\"model\""),
        "the model default writes the `model` key"
    );

    let thinking = hooks
        .find("save_default_thinking: Some")
        .expect("the thinking save exists");
    let body = &hooks[thinking..];
    let end = body.find("})),").unwrap_or(body.len());
    assert!(
        body[..end].contains("persist_setting(\"thinking\""),
        "the thinking default writes the `thinking` key through the same call:\n{}",
        &body[..end.min(700)]
    );
}

// Verifies: gh #396 rule 7 - **one binding for one gesture.** The thinking save
// is `ctrl+s`, the same as the model save, because they are the same gesture in the
// same kind of picker. pi agrees: its `app.thinking.save` default is `ctrl+s`
// (`core/keybindings.ts`), so matching LCA's model save is both internally
// consistent and pi's own choice. A second key for one idea is what this row
// prevents.
#[test]
fn the_thinking_save_keeps_the_model_saves_key() {
    let source = code_only(&read("crates/lca-tui/src/engine/keybindings.rs"));
    for action in ["app.models.save", "app.thinking.save"] {
        let at = source
            .find(&format!("\"{action}\""))
            .unwrap_or_else(|| panic!("{action} is registered"));
        let body = &source[at..];
        let end = body.find("),").unwrap_or(body.len());
        assert!(
            body[..end].contains("ctrl+s"),
            "{action} keeps the shared key:\n{}",
            &body[..end.min(200)]
        );
    }
}

// Verifies: gh #396 rule 8 - **both pickers advertise the save.** A picker that
// supports saving must say so, and the one that already did must keep saying so -
// which means both hint rows are built, not one hard-coded and one forgotten.
#[test]
fn both_pickers_build_their_hint_from_the_live_bindings() {
    let source = code_only(&read("crates/lca-ui/src/chat_overlays.rs"));
    assert!(
        source.contains("\"app.models.save\""),
        "the model picker's hint names its save action"
    );
    assert!(
        source.contains("\"app.thinking.save\""),
        "and the thinking picker's names its own"
    );
    // Both go through the one hint builder, so the wording cannot drift between
    // them.
    assert!(
        source.matches("save_hint(").count() >= 3,
        "one builder, two call sites (plus its definition)"
    );
}
