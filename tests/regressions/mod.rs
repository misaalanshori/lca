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
