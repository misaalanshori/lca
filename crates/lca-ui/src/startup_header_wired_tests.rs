//! gh #399 rows for the wiring: the quiet-startup ladder reaches the transcript, a
//! headless run prints none of it, and the header renders from live keys and width.
//!
//! These sit beside the pure `startup_header` rows on purpose. Those prove the
//! block's *shape*; these prove it is **reachable** - a pure renderer nobody calls
//! is a library, not a feature.

use crate::chat::Chat;
use crate::state::{DisplayTuning, StartupHeaderRung};
use lca_tui::engine::keybindings::KeybindingsManager;
use std::sync::Arc;

/// A chat whose display tuning says `rung`.
fn chat_with_rung(rung: StartupHeaderRung) -> Chat {
    let mut options = super::tests::options();
    options.plain = true;
    options.hooks.display_tuning = Some(Arc::new(move || DisplayTuning {
        startup_header: rung,
        startup_version: ("0.6.1".to_string(), "0.6".to_string()),
        ..DisplayTuning::default()
    }));
    Chat::new(options, Arc::new(KeybindingsManager::new()))
}

fn plain_rows(chat: &Chat, width: u16) -> Vec<String> {
    chat.transcript
        .render(width, &crate::theme::Theme::plain())
        .iter()
        .map(|row| lca_tui::engine::text::strip_terminal_sequences(row))
        .collect()
}

#[test]
fn the_full_rung_draws_the_logo_block() {
    let chat = chat_with_rung(StartupHeaderRung::Full);
    let rows = plain_rows(&chat, 200);
    let text = rows.join("\n");
    assert!(text.contains("v0.6.1 (abi 0.6)"), "the version row: {text}");
    assert!(text.contains("interrupt"), "the live hint row: {text}");
    assert!(
        text.contains("LCA can explain its own features"),
        "the onboarding nudge: {text}"
    );
    assert!(text.contains('█'), "the mark itself: {text}");
}

#[test]
fn quiet_startup_still_wins() {
    // The setting's whole point, and worker 2 holds `startup.rs` this window: a
    // quiet startup shows no logo at all.
    let hidden = chat_with_rung(StartupHeaderRung::Hidden);
    let text = plain_rows(&hidden, 200).join("\n");
    assert!(
        !text.contains('█') && !text.contains("v0.6.1"),
        "nothing draws when quiet: {text}"
    );

    // `"header"` is the middle rung: the version row and no mark, no hints.
    let version_only = chat_with_rung(StartupHeaderRung::VersionOnly);
    let text = plain_rows(&version_only, 200).join("\n");
    assert!(
        text.contains("v0.6.1 (abi 0.6)"),
        "the version stays: {text}"
    );
    assert!(!text.contains('█'), "but the mark does not: {text}");
    assert!(!text.contains("interrupt"), "nor the hints: {text}");
}

#[test]
fn a_rebind_reaches_the_header_in_the_transcript() {
    // The live-key resolution measured where a reader sees it, not in the pure
    // function: the transcript re-renders the entry, so the new key appears.
    let mut options = super::tests::options();
    options.plain = true;
    options.hooks.display_tuning = Some(Arc::new(|| DisplayTuning {
        startup_header: StartupHeaderRung::Full,
        startup_version: ("0.6.1".to_string(), "0.6".to_string()),
        ..DisplayTuning::default()
    }));
    let mut user = std::collections::BTreeMap::new();
    user.insert("app.interrupt".to_string(), vec!["ctrl+q".to_string()]);
    let chat = Chat::new(
        options,
        Arc::new(KeybindingsManager::with_user_bindings(user)),
    );
    let text = plain_rows(&chat, 200).join("\n");
    assert!(
        text.contains("ctrl+q interrupt"),
        "the rebind lands: {text}"
    );
    assert!(
        !text.contains("escape interrupt"),
        "the old key is gone: {text}"
    );
}

#[test]
fn a_narrow_pane_degrades_the_header_in_the_transcript() {
    // The width rung is decided at render, so the same transcript renders
    // differently at two widths - which is what makes `apply_display` and a resize
    // reach it rather than only the startup frame.
    let chat = chat_with_rung(StartupHeaderRung::Full);
    let wide = plain_rows(&chat, 200).join("\n");
    let narrow = plain_rows(&chat, 30).join("\n");
    assert!(wide.contains("interrupt"), "wide keeps the hints: {wide}");
    assert_ne!(wide, narrow, "the header follows the width");
    // At 30 columns the beside rung does not fit (the owner's hint line is 79), so
    // the wordmark gets its own row and the text follows - which is the rung, and
    // it is visible as the mark no longer sharing its row with the version.
    assert!(narrow.starts_with("LCA"), "the wordmark leads: {narrow}");
    assert!(
        !narrow.lines().next().unwrap_or("").contains("v0.6.1"),
        "the version is not beside the mark any more: {narrow}"
    );
    assert!(
        narrow.contains("v0.6.1 (abi 0.6)"),
        "but it is still printed: {narrow}"
    );
}

#[test]
fn the_header_prints_the_version_lca_dash_dash_version_reports() {
    // One source, so the header and `--version` cannot disagree. The tuning carries
    // both strings; this pins that the block uses them rather than its own idea of
    // the version.
    let mut options = super::tests::options();
    options.plain = true;
    options.hooks.display_tuning = Some(Arc::new(|| DisplayTuning {
        startup_header: StartupHeaderRung::Full,
        startup_version: ("9.9.9.babc1234".to_string(), "0.6".to_string()),
        ..DisplayTuning::default()
    }));
    let chat = Chat::new(options, Arc::new(KeybindingsManager::new()));
    let text = plain_rows(&chat, 200).join("\n");
    assert!(
        text.contains("v9.9.9.babc1234 (abi 0.6)"),
        "the supplied version, verbatim: {text}"
    );
}
