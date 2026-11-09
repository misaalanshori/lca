#[path = "part0.rs"]
mod part0;

#[path = "part1.rs"]
mod part1;

#[path = "part2.rs"]
mod part2;

#[path = "part3.rs"]
mod part3;
// A shared test double, kept at the crate root so every part can `crate::mid_turn_slash_dispatches_as_command`.
#[path = "mid-turn-slash-dispatches-as-command.rs"]
mod mid_turn_slash_dispatches_as_command;
// A shared test double, kept at the crate root so every part can `crate::doubles_275`.
#[path = "doubles_275.rs"]
mod doubles_275;
// Gh #380 gap 1: `@file` completed only at column 0.
#[path = "380-at-sign-completes-anywhere-a-word-starts.rs"]
mod at_sign_completes_anywhere_a_word_starts;
// Gh #366 gap 3: a split terminal reply is no longer typed into the prompt.
#[path = "366-split-reply-never-typed.rs"]
mod split_reply_never_typed;
// Gh #386 gap 1: the popup's ragged description column and missing provenance.
#[path = "386-autocomplete-column-and-provenance.rs"]
mod autocomplete_column_and_provenance;
// Gh #290: MCP OAuth sign-in and the untrusted metadata document.
#[path = "290-mcp-oauth-untrusted-metadata.rs"]
mod mcp_oauth_untrusted_metadata;
// Gh #304: a headerless `SKILL.md` lost its opening instruction lines.
#[path = "304-skill-headerless-body-loss.rs"]
mod skill_headerless_body_loss;
// Gh #333: a tab switch must not leave an overlay pinned over the next tab.
#[path = "333-overlays-close-on-tab-switch.rs"]
mod overlays_close_on_tab_switch;
// Gh #328: an expanded diff card must not print its result twice.
#[path = "328-diff-card-does-not-duplicate.rs"]
mod diff_card_does_not_duplicate;
// Gh #301: the reference ui extension must not leak a pty child or erase its panel.
#[path = "301-panel-screen-and-pty-leak.rs"]
mod panel_screen_and_pty_leak;
// Gh #578: a hook is deliverable only to an extension that declares its world.
#[path = "578-hooks-only-for-callers.rs"]
mod hooks_only_for_callers;
// Gh #325: the mermaid renderer must not leak, re-parse a repaint, or drop a label.
#[path = "325-mermaid-leak-and-asymmetric-label.rs"]
mod mermaid_leak_and_asymmetric_label;
// Gh #323: one cache policy, per-terminal Kitty state, checked console modes.
#[path = "323-engine-robustness.rs"]
mod engine_robustness;
// Gh #579: the crate-wide cache-policy guard that #323's engine-only version missed.
#[path = "579-one-cache-policy-crate-wide.rs"]
mod one_cache_policy_crate_wide;
// Gh #326: a narrow rail keeps its close button, and the ladder promises no Sixel.
#[path = "326-tabs-keep-close-and-no-sixel-claim.rs"]
mod tabs_keep_close_and_no_sixel_claim;
// Gh #267: `mcp.json` is written atomically, owner-only, and never truncated in place.
#[path = "267-mcp-json-write-atomic-and-owner-only.rs"]
mod mcp_json_write_atomic_and_owner_only;
// Gh #262: each resource flag names its own expectation in a failure message.
#[path = "262-per-flag-resource-copy.rs"]
mod per_flag_resource_copy;
// Gh #268: RPC honours `queue.follow_up_mode`, and the teardown is awaited not bridged.
#[path = "268-follow-up-mode-and-async-teardown.rs"]
mod follow_up_mode_and_async_teardown;
// Gh #381 gaps 1-2: a paste chip is one unit for every motion, wrap and delete path.
#[path = "381-chip-is-one-unit.rs"]
mod chip_is_one_unit;

/// Every implementation file a module is spread across, as
/// `(path, source)` pairs in declaration order.
///
/// Gh #574 split `lca-ui`'s near-ceiling files along their own seams, and gh #543
/// does the same for `lca-cli`; a structural guard that reads one **filename**
/// stops seeing the code it guards as soon as that code moves - and if it greps
/// for one line, it may pass *vacuously* instead of failing. Guards therefore
/// read the *module*: this walks the module file's own `mod` declarations
/// (skipping `#[cfg(test)]` ones) and reads each child it finds, recursively,
/// honouring `#[path = "…"]`. Generic over crate on purpose - the walk is
/// ordinary `mod` syntax, so `crates/lca-cli/src/tui/mod.rs` serves as well as
/// `crates/lca-ui/src/chat.rs`.
///
/// The pairs carry their path so a failure message can still name the file the
/// offending line is in.
pub fn module_sources(module_rel: &str) -> Vec<(String, String)> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut out = Vec::new();
    collect_sources(root, module_rel, &mut out);
    assert!(!out.is_empty(), "{module_rel} does not exist");
    out
}

/// The concatenated source of a module's implementation files (see
/// [`module_sources`]). Use this when a guard greps for one line and the
/// line's file is an implementation detail the ceiling split may change.
pub fn module_text(module_rel: &str) -> String {
    module_sources(module_rel)
        .into_iter()
        .map(|(_, text)| text)
        .collect::<Vec<_>>()
        .join("\n")
}

/// The concatenation of several roots, for a guard whose partition the module
/// walk cannot know: `chat_keys.rs`'s picker arms moved to a sibling declared
/// from the crate root, not from `chat_keys.rs`, so no single root reaches both.
/// The same applies to `lca-cli` files that have no submodules but gain one
/// (`ext.rs` becomes `ext/mod.rs` with `install`/`manage` beside it).
pub fn text_of(roots: &[&str]) -> String {
    roots
        .iter()
        .map(|root| module_text(root))
        .collect::<Vec<_>>()
        .join("\n")
}

fn collect_sources(root: &std::path::Path, rel: &str, out: &mut Vec<(String, String)>) {
    let path = root.join(rel);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) => panic!("read {}: {err}", path.display()),
    };
    out.push((rel.to_string(), text.clone()));

    // Where this file's children live: a `mod.rs` owns its directory, a plain
    // `name.rs` owns the sibling directory `name/`.
    let parent = std::path::Path::new(rel)
        .parent()
        .map(|dir| dir.to_string_lossy().to_string())
        .unwrap_or_default();
    let stem = std::path::Path::new(rel)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let child_dir = if stem == "mod" || stem == "lib" {
        parent.clone()
    } else {
        format!("{parent}/{stem}")
    };

    let mut pending_path: Option<String> = None;
    let mut gated = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("#[cfg(test)]") {
            gated = true;
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("#[path = \"") {
            pending_path = rest.strip_suffix("\"]").map(|s| s.to_string());
            continue;
        }
        // Any other attribute keeps the state it is in; any real statement
        // resets it, so a `#[cfg(test)]` further up never gates a later `mod`.
        let Some(rest) = trimmed.strip_prefix("mod ") else {
            if !trimmed.starts_with('#') && !trimmed.is_empty() {
                pending_path = None;
                gated = false;
            }
            continue;
        };
        // An inline `mod name { … }` has no file of its own.
        let Some(name) = rest.strip_suffix(';') else {
            pending_path = None;
            gated = false;
            continue;
        };
        let skip = gated;
        gated = false;
        let attribute = pending_path.take();
        if skip {
            continue;
        }
        let child = match attribute {
            // `#[path]` resolves against the declaring file's own directory.
            Some(path) => format!("{child_dir}/{path}"),
            None => {
                let candidates = [
                    format!("{child_dir}/{name}.rs"),
                    format!("{child_dir}/{name}/mod.rs"),
                ];
                match candidates
                    .iter()
                    .find(|candidate| root.join(candidate).exists())
                {
                    Some(found) => found.clone(),
                    None => continue,
                }
            }
        };
        collect_sources(root, &child, out);
    }
}
