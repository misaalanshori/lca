//! Gh #333: a tab switch must close **every** transient overlay (gh #333).
//!
//! The behavioural rows live here; the structural guard that makes forgetting
//! impossible is in `tests/regressions/333-*` (it parses the struct declarations, which
//! a unit test cannot do).
//!
//! The defect: `Chat::close_pickers` was a hand-maintained list of ten pickers, and the
//! overlays #227 added - `extension_hub`, `extension_manager`, `install_consent` - were
//! never added to it. `/extensions` in Tab 1 stayed pinned over Tab 2's transcript.
//! `mcp_manager` (#418 Phase G) leaked the same way.

use super::*;
use super::{options, strip};
use std::sync::Arc;

/// A two-session host, the shape `chat_tab_tests` uses.
fn tabbed() -> Chat {
    use lca_tui::engine::keybindings::KeybindingsManager;
    let mut opts = options();
    let minted = Arc::new(std::sync::Mutex::new(3u32));
    let next = minted.clone();
    opts.hooks.new_session = Some(Arc::new(move || {
        let mut n = next.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let id = format!("s{}", *n);
        *n += 1;
        Some(id)
    }));
    opts.hooks.switch_session = Some(Arc::new(|id: &str| {
        Some(vec![lca_protocol::Record::User {
            v: lca_protocol::FORMAT_VERSION,
            ts: 1,
            id: format!("{id}-u"),
            parent: None,
            content: format!("hello from {id}"),
            attachments: Vec::new(),
            queue: None,
        }])
    }));
    opts.hooks.session_list = Some(Arc::new(|| {
        vec![crate::resume::SessionEntry {
            id: "s1".to_string(),
            title: "title s1".to_string(),
            messages: 1,
            age: "now".to_string(),
            labels: Vec::new(),
            modified_ms: 0,
            parent: None,
        }]
    }));
    opts.hooks.current_session_id = Some(Arc::new(|| "s1".to_string()));
    // The hub and the manager need a catalog to open against; a fixture row set keeps
    // these rows about the *switch*, not about discovery.
    opts.hooks.fetch_extensions = Some(Arc::new(|| crate::chat_hub::HubData {
        installed: Vec::new(),
        available: vec![crate::chat_pickers::HubRow {
            name: "word-count".to_string(),
            version: "1.0.0".to_string(),
            capabilities: vec!["net".to_string()],
            description: "Count words".to_string(),
            oci_ref: "oci://example.invalid/word-count:1.0.0".to_string(),
            source_name: "Official (misaalanshori/lca)".to_string(),
            source_url: "https://example.invalid/catalog.toml".to_string(),
            official: true,
            installed: false,
        }],
        notice: None,
    }));
    opts.hooks.list_managed = Some(Arc::new(|| crate::chat_extman::ManagedData {
        extensions: Vec::new(),
        strategy: None,
    }));
    Chat::new(opts, Arc::new(KeybindingsManager::new()))
}

/// Two tabs open, on the second.
fn two_tabs() -> Chat {
    let mut chat = tabbed();
    chat.handle_key("\x14");
    chat.select_tab(0);
    chat
}

// ============================================================================
// The issue's three, one row each. These are red before the fix.
// ============================================================================

// Verifies: gh #333 - the Extension Hub does not survive a tab switch.
#[test]
fn the_extension_hub_is_gone_after_a_tab_switch() {
    let mut chat = two_tabs();
    chat.dispatch_extensions();
    assert!(chat.extension_hub.is_some(), "the hub opened");
    chat.select_tab(1);
    assert_eq!(chat.active_tab, 1, "the switch happened");
    assert!(
        chat.extension_hub.is_none(),
        "the hub belongs to the tab that opened it"
    );
}

// Verifies: gh #333 - the Extension Manager does not survive a tab switch.
#[test]
fn the_extension_manager_is_gone_after_a_tab_switch() {
    let mut chat = two_tabs();
    chat.open_extension_manager();
    assert!(chat.extension_manager.is_some(), "the manager opened");
    chat.select_tab(1);
    assert!(
        chat.extension_manager.is_none(),
        "the manager belongs to the tab that opened it"
    );
}

// Verifies: gh #333 - an install-consent dialog does not survive a tab switch.
#[test]
fn an_install_consent_is_gone_after_a_tab_switch() {
    let mut chat = two_tabs();
    chat.install_consent = Some(crate::chat_hub::InstallConsent {
        name: "word-count".to_string(),
        version: "1.0.0".to_string(),
        publisher: "Official (misaalanshori/lca)".to_string(),
        source_url: "https://example.invalid/catalog.toml".to_string(),
        artifact_ref: "oci://example.invalid/word-count:1.0.0".to_string(),
        capabilities: vec!["net".to_string()],
    });
    chat.select_tab(1);
    assert!(
        chat.install_consent.is_none(),
        "the consent prompt belongs to the tab that opened it"
    );
}

// ============================================================================
// The one the issue did not name: the sweep found it.
// ============================================================================

// Verifies: gh #333 - `mcp_manager` (#418 Phase G) leaked exactly like the three the
// issue names, because it arrived after `close_pickers` was written. This row is the
// reason the fix is a list rather than three lines: the fourth leak was already there
// when the issue was filed.
#[test]
fn the_mcp_manager_is_gone_after_a_tab_switch() {
    let mut chat = two_tabs();
    chat.mcp_manager = Some(crate::chat_mcp::McpManager::ready(Vec::new(), Vec::new()));
    chat.select_tab(1);
    assert!(
        chat.mcp_manager.is_none(),
        "the MCP manager belongs to the tab that opened it"
    );
}

// Verifies: gh #333 - the overlays that live on `UiState` (the `world`) are closed too.
//
// They are the same kind of thing and were the same kind of omission waiting to happen:
// `wizard` (#397) and `login_waiting` arrived after the picker list, and a tab switch
// used to leave them on screen. One row rather than five, because the structural guard
// is what enumerates them; this row proves the wiring reaches the world at all.
#[test]
fn world_overlays_are_gone_after_a_tab_switch() {
    let mut chat = two_tabs();
    chat.world.login_waiting = Some("signing in to anthropic…".to_string());
    chat.world.picker = Some(crate::state::PickerPrompt {
        options: vec![crate::state::PickerOption {
            provider: "anthropic".to_string(),
            id: "pro".to_string(),
            label: "Claude Pro".to_string(),
            hint: String::new(),
        }],
        selected: 0,
    });
    chat.world.panel_open = true;
    chat.select_tab(1);
    assert!(
        chat.world.login_waiting.is_none(),
        "a waiting step does not follow you to another tab"
    );
    assert!(chat.world.picker.is_none(), "nor a provider picker");
    assert!(!chat.world.panel_open, "nor the side panel's open flag");
}

// ============================================================================
// The no-over-close row: the fix must not clear what a tab owns.
// ============================================================================

// Verifies: gh #333 design point 4 - a tab switch **preserves the draft and the scroll
// position**.
//
// This is the row that keeps a blunt "clear everything on switch" from passing. Draft
// and scroll are per-tab state (`SessionTab::editor` / `SessionTab::scroll`), saved by
// `save_tab` and restored by `enter_tab_session`; if the overlay clearing ever grows to
// touch them, this row goes red.
#[test]
fn a_tab_switch_preserves_the_draft_and_the_scroll() {
    let mut chat = two_tabs();
    chat.editor.set_text("half-written prompt");
    chat.last_scroll = 7;
    chat.select_tab(1);
    assert_eq!(
        chat.tabs[0].editor, "half-written prompt",
        "the draft on the tab left behind survives"
    );
    assert_eq!(chat.tabs[0].scroll, 7, "and so does its scroll position");
    chat.select_tab(0);
    assert_eq!(
        chat.editor.text(),
        "half-written prompt",
        "and the draft comes back when the tab does"
    );
}

// Verifies: gh #333 - the **transcript** is not an overlay: a background turn's content
// is still there after a switch. The neighbouring row to the draft/scroll one, because
// "clear the transient stuff" must not extend to the durable stuff either.
#[test]
fn a_tab_switch_keeps_both_transcripts() {
    let mut chat = two_tabs();
    chat.say("tab one content".to_string());
    chat.select_tab(1);
    chat.say("tab two content".to_string());
    let second = strip(&chat.render(100)).join("\n");
    assert!(second.contains("tab two content"), "{second}");
    chat.select_tab(0);
    let first = strip(&chat.render(100)).join("\n");
    assert!(
        first.contains("tab one content"),
        "the first tab's transcript survived the round trip:\n{first}"
    );
}

// ============================================================================
// The consent-dialog outcome, stated and pinned (design point 5).
// ============================================================================

// Verifies: gh #333 design point 5 - a tab switch **abandons** an open install consent,
// and nothing installs.
// Given `InstallConsent` is a *question*, not a running install: the confirm hook is
// only invoked when the user answers it. So clearing the field is the safe outcome - the
// question is dropped, the answer never happens, and no worker is spawned.
//
// The alternatives are both worse, which is why this is stated rather than left to fall
// out of the implementation:
//
// * **leaving it pending** would pin the dialog over an unrelated tab (the bug);
// * **answering it** on the user's behalf would install a component nobody confirmed.
//
// The user re-opens `/extensions` and is asked again, with the same text. The row proves
// the *install does not start*, not merely that the field is empty.
#[test]
fn a_tab_switch_abandons_an_install_consent_and_installs_nothing() {
    let started = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let sink = started.clone();
    let mut chat = two_tabs();
    chat.world.options.hooks.confirm_install =
        Some(Arc::new(move |consent: crate::chat_hub::InstallConsent| {
            sink.lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(consent.name.clone());
        }));
    chat.install_consent = Some(crate::chat_hub::InstallConsent {
        name: "word-count".to_string(),
        version: "1.0.0".to_string(),
        publisher: "Official (misaalanshori/lca)".to_string(),
        source_url: "https://example.invalid/catalog.toml".to_string(),
        artifact_ref: "oci://example.invalid/word-count:1.0.0".to_string(),
        capabilities: vec!["net".to_string()],
    });
    chat.select_tab(1);
    // Answering is impossible now - the prompt is gone - so a stray Enter must not
    // reach the confirm hook either.
    chat.handle_key("\r");
    assert!(
        started
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_empty(),
        "no install was started by the abandoned consent"
    );
}

// ============================================================================
// Design point 3: `/reload` and session close ask the same question.
//
// The answer, pinned here so it cannot silently change:
//
// * a **tab switch** closes everything the registry lists - a surface that belongs to
//   the tab you left must not be drawn over the tab you arrived at;
// * **`/reload`** closes nothing on this list. It re-reads config, themes and keys; it
//   is not a navigation, the reader is still looking at the same tab, and a
//   half-finished install consent or a permission modal is *more* relevant after a
//   reload than before one (the reload may be what the modal is about). Clearing them
//   would answer questions nobody asked and, for a consent, would silently abandon an
//   install;
// * **session close** (`close_tab`) goes through `enter_tab_session` like a switch, so
//   it closes the same set - the surface belonged to the tab that just closed.
//
// So the three paths are **not** the same, and the difference is deliberate: two are
// navigations and one is not. This row pins that `/reload` leaves a consent standing,
// because that is the one a "unify them" refactor would break.
// ============================================================================

// Verifies: gh #333 design point 3 - `/reload` does **not** dismiss an open consent.
//
// The reload path re-reads config and re-applies the theme; it does not navigate. A
// consent that vanished on a reload would abandon an install the reader was in the
// middle of answering, and a permission modal is no less urgent after one.
#[test]
fn reload_does_not_dismiss_an_open_consent() {
    let mut chat = two_tabs();
    chat.install_consent = Some(crate::chat_hub::InstallConsent {
        name: "word-count".to_string(),
        version: "1.0.0".to_string(),
        publisher: "p".to_string(),
        source_url: "https://example.invalid/c.toml".to_string(),
        artifact_ref: "oci://example.invalid/w:1".to_string(),
        capabilities: vec!["net".to_string()],
    });
    let reloaded = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let hit = reloaded.clone();
    chat.world.options.hooks.reload = Some(Arc::new(move || {
        hit.store(true, std::sync::atomic::Ordering::SeqCst);
        crate::state::ReloadReport {
            notice: "reloaded".to_string(),
            themes: Vec::new(),
            key_bindings: Default::default(),
            keybinding_error: None,
        }
    }));
    // `dispatch_command` is the reload path `/reload` reaches, called directly: the
    // consent owns the keyboard, so typing `/reload` at it would (correctly) be
    // swallowed by the modal rather than reaching the command. The question here is
    // what *reload itself* does to an open consent, not what a keypress does.
    chat.dispatch_command("/reload");
    assert!(
        reloaded.load(std::sync::atomic::Ordering::SeqCst),
        "the row actually reached the reload hook"
    );
    assert!(
        chat.install_consent.is_some(),
        "a reload is not a navigation: the question stays open"
    );
}

// Verifies: gh #333 design point 3 - **closing** a tab closes the set, because it goes
// through the same `enter_tab_session` a switch does. The surface belonged to the tab
// that just went away.
#[test]
fn closing_a_tab_closes_the_overlays() {
    let mut chat = two_tabs();
    chat.dispatch_extensions();
    assert!(chat.extension_hub.is_some(), "the hub opened");
    chat.close_tab();
    assert_eq!(chat.tabs.len(), 1, "the tab closed");
    assert!(chat.extension_hub.is_none(), "and its overlay went with it");
}
