//! Slash-command and key behaviors (ceiling split from
//! `chat_tests.rs`): clone, message-copy target, the resume picker,
//! and reload. Shares the `chat_tests` helpers.

use super::{chat, options, strip, tree_row};
use lca_tui::engine::keybindings::KeybindingsManager;

use super::super::Chat;
use crate::state::LoginNext;
use std::sync::Arc;

#[test]
fn clone_duplicates_the_tip_and_switches() {
    let mut options = options();
    options.hooks.clone_session = Some(Arc::new(|name: Option<String>| {
        assert_eq!(name.as_deref(), Some("experimental"));
        Ok(("new-id".to_string(), "experimental".to_string()))
    }));
    options.hooks.switch_session = Some(Arc::new(|id: &str| {
        (id == "new-id").then(|| {
            vec![lca_protocol::Record::User {
                v: lca_protocol::FORMAT_VERSION,
                ts: 1,
                id: "r1".into(),
                parent: None,
                content: "from the clone".into(),
                attachments: Vec::new(),
                queue: None,
            }]
        })
    }));
    let mut chat = Chat::new(options, Arc::new(KeybindingsManager::new()));
    for c in "/clone experimental".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    let notice = chat.world.notice.as_deref().unwrap_or("");
    assert!(
        notice.contains("new-id") && notice.contains("experimental"),
        "the notice names the clone: {notice}"
    );
    let text = strip(&chat.render(80)).join("\n");
    assert!(text.contains("from the clone"), "records replayed: {text}");
}

// Verifies: gh #200 - Ctrl+X is a copy request on the OAuth waiting
// screen (not swallowed as "modal owns the keyboard"), and the target
// is the exact sign-in URL out of the real OSC 8 wrapped label.
#[test]
fn ctrl_x_copies_the_oauth_url_on_the_waiting_screen() {
    let mut chat = chat();
    let url = "https://accounts.example.test/o/oauth2/v2/auth?code=42&state=zz";
    chat.apply_login_next(LoginNext::Waiting {
        label: format!(
            "waiting for browser sign-in… (esc cancels)\n\n\x1b]8;;{url}\x07{url}\x1b]8;;\x07"
        ),
    });
    assert!(
        chat.message_copy_key("\x18"),
        "Ctrl+X copies on the waiting screen"
    );
    assert_eq!(
        chat.message_copy_text().as_deref(),
        Some(url),
        "the exact URL, no OSC 8 wrapper bytes"
    );
}

// Verifies: gh #110 - opening the resume picker lists the host's
// sessions; with none, a notice says so instead of an empty picker.
#[test]
fn open_resume_picker_lists_sessions_or_says_none() {
    let mut opts = options();
    opts.hooks.session_list = Some(Arc::new(|| {
        vec![crate::resume::SessionEntry {
            id: "s1".into(),
            title: "first".into(),
            messages: 3,
            age: "today".into(),
            labels: Vec::new(),
        }]
    }));
    let mut chat = Chat::new(opts, Arc::new(KeybindingsManager::new()));
    chat.open_resume_picker();
    assert!(chat.resume_picker.is_some(), "picker opens with entries");

    let mut opts = options();
    opts.hooks.session_list = Some(Arc::new(Vec::new));
    let mut chat = Chat::new(opts, Arc::new(KeybindingsManager::new()));
    chat.open_resume_picker();
    assert!(chat.resume_picker.is_none(), "no entries, no picker");
    assert_eq!(
        chat.world.notice.as_deref(),
        Some("no sessions yet"),
        "the empty case says so"
    );
}

// Verifies: gh #130 - `/reload` runs the host hook and applies the
// report interface-side: the notice names it, themes refresh, and the
// key manager reloads (refused mid-turn like a switch).
#[test]
fn reload_applies_the_host_report() {
    let mut opts = options();
    opts.hooks.reload = Some(Arc::new(|| crate::state::ReloadReport {
        notice: "reloaded: 3 extensions, settings, prompts, themes, keybindings".to_string(),
        themes: vec!["auto".to_string(), "fresh".to_string()],
        key_bindings: [("app.panel.toggle".to_string(), vec!["ctrl+q".to_string()])]
            .into_iter()
            .collect(),
        keybinding_error: None,
    }));
    let mut chat = Chat::new(opts, Arc::new(KeybindingsManager::new()));
    for c in "/reload".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    let notice = chat.world.notice.as_deref().unwrap_or("");
    assert!(notice.contains("reloaded:"), "the hook names it: {notice}");
    assert!(
        chat.world.options.themes.contains(&"fresh".to_string()),
        "themes refresh"
    );
    assert_eq!(
        chat.keybindings.keys("app.panel.toggle"),
        vec!["ctrl+q".to_string()],
        "keys reload"
    );
}

// Verifies: gh #203 - bare `/fork` opens a user-message picker (latest
// selected); an empty transcript names it instead of opening.
#[test]
fn bare_fork_lists_user_messages_for_picking() {
    let mut chat = chat();
    for c in "/fork".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    assert_eq!(
        chat.world.notice.as_deref(),
        Some("No messages to fork from"),
        "empty transcript names it"
    );
    chat.transcript.push_user("first question");
    chat.transcript.push_user("second question");
    for c in "/fork".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    let picker = chat.fork_picker.as_ref().expect("picker opens");
    assert_eq!(picker.messages.len(), 2, "every user message lists");
    assert_eq!(picker.selected, 1, "latest selected, pi's default");
    let viewport = strip(&chat.viewport(100, 30, 0)).join("\n");
    assert!(
        viewport.contains("second question") && viewport.contains("first question"),
        "previews render:\n{viewport}"
    );
}

// Verifies: gh #203 - choosing in the picker forks AND switches
// in-process, restoring the message text into the editor.
#[test]
fn fork_choice_switches_and_restores_the_text() {
    let mut opts = options();
    opts.hooks.fork_at = Some(Arc::new(|n: usize| crate::state::ForkReport {
        id: Some("forked-id".to_string()),
        notice: format!("✓ Forked from turn {n} (session: forked-id)"),
    }));
    opts.hooks.switch_session = Some(Arc::new(|_id: &str| Some(Vec::new())));
    let mut chat = Chat::new(opts, Arc::new(KeybindingsManager::new()));
    chat.transcript.push_user("branch me");
    chat.transcript.push_user("stay here");
    for c in "/fork".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    chat.handle_key("\x1b[A"); // up-arrow: back to "branch me"
    chat.handle_key("\r"); // fork + switch
    assert!(chat.fork_picker.is_none(), "the picker closes");
    assert_eq!(chat.editor.text(), "branch me", "the text restores");
    let notice = chat.world.notice.as_deref().unwrap_or("");
    assert!(
        notice.contains("Forked from turn 0") && notice.contains("forked-id"),
        "the switch names the fork: {notice}"
    );
}

// Verifies: gh #203 - `/fork <n>` still works, now switching in-process
// with the text restored (the old exit-and-resume round trip is gone).
#[test]
fn fork_index_switches_in_process() {
    let mut opts = options();
    opts.hooks.fork_at = Some(Arc::new(|n: usize| crate::state::ForkReport {
        id: Some("direct-id".to_string()),
        notice: format!("✓ Forked from turn {n} (session: direct-id)"),
    }));
    opts.hooks.switch_session = Some(Arc::new(|_id: &str| Some(Vec::new())));
    let mut chat = Chat::new(opts, Arc::new(KeybindingsManager::new()));
    chat.transcript.push_user("direct branch");
    for c in "/fork 0".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    assert_eq!(chat.editor.text(), "direct branch", "the text restores");
    assert!(
        chat.world
            .notice
            .as_deref()
            .unwrap_or("")
            .contains("Forked from turn 0"),
        "the switch names the fork: {:?}",
        chat.world.notice
    );
}

// Verifies: gh #204 - `/scoped-models` opens the checklist over every
// offered model, current scope checked.
#[test]
fn scoped_models_opens_the_checklist() {
    let mut opts = options();
    opts.hooks.scoped_models = Some(Arc::new(|| {
        vec![
            crate::state::ScopedModelRow {
                id: "aaa".to_string(),
                label: "aaa (prov)".to_string(),
                context: 128_000,
                enabled: true,
            },
            crate::state::ScopedModelRow {
                id: "bbb".to_string(),
                label: "bbb (prov)".to_string(),
                context: 0,
                enabled: false,
            },
        ]
    }));
    let mut chat = Chat::new(opts, Arc::new(KeybindingsManager::new()));
    for c in "/scoped-models".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    assert!(chat.scoped_models_picker.is_some(), "the checklist opens");
    let viewport = strip(&chat.viewport(100, 30, 0)).join("\n");
    assert!(
        viewport.contains("[x] aaa") && viewport.contains("[ ] bbb"),
        "checked state renders:\n{viewport}"
    );
}

// Verifies: gh #204 - space toggles a row, enter saves the checked set
// (the rotation updates through the save hook), esc discards everything.
#[test]
fn scoped_models_toggle_saves_and_escape_discards() {
    use std::sync::Mutex;
    let mut opts = options();
    opts.hooks.scoped_models = Some(Arc::new(|| {
        vec![
            crate::state::ScopedModelRow {
                id: "aaa".to_string(),
                label: "aaa".to_string(),
                context: 0,
                enabled: true,
            },
            crate::state::ScopedModelRow {
                id: "bbb".to_string(),
                label: "bbb".to_string(),
                context: 0,
                enabled: false,
            },
        ]
    }));
    let saved: Arc<Mutex<Vec<Vec<String>>>> = Arc::new(Mutex::new(Vec::new()));
    let saved_hook = saved.clone();
    opts.hooks.save_scoped_models = Some(Arc::new(move |ids: Vec<String>| {
        saved_hook.lock().unwrap().push(ids);
        "scope saved".to_string()
    }));
    let mut chat = Chat::new(opts, Arc::new(KeybindingsManager::new()));
    for c in "/scoped-models".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    chat.handle_key("\x1b[B"); // down to bbb
    chat.handle_key(" "); // check bbb
    chat.handle_key("\r"); // save
    assert!(chat.scoped_models_picker.is_none(), "save closes");
    assert_eq!(
        saved.lock().unwrap().as_slice(),
        &[vec!["aaa".to_string(), "bbb".to_string()]],
        "the checked set saves"
    );
    assert_eq!(
        chat.world.notice.as_deref(),
        Some("scope saved"),
        "the save names it"
    );
    // Reopen, toggle, then escape: nothing more saves.
    for c in "/scoped-models".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    chat.handle_key(" ");
    chat.handle_key("\x1b");
    assert!(chat.scoped_models_picker.is_none(), "escape closes");
    assert_eq!(saved.lock().unwrap().len(), 1, "escape discards");
}

// Verifies: gh #204 - an empty checklist refuses to save (an empty
// scope means no restriction, so saving it would re-enable all).
#[test]
fn scoped_models_empty_scope_refuses_to_save() {
    let mut opts = options();
    opts.hooks.scoped_models = Some(Arc::new(|| {
        vec![crate::state::ScopedModelRow {
            id: "aaa".to_string(),
            label: "aaa".to_string(),
            context: 0,
            enabled: true,
        }]
    }));
    opts.hooks.save_scoped_models =
        Some(Arc::new(|_: Vec<String>| panic!("must not save nothing")));
    let mut chat = Chat::new(opts, Arc::new(KeybindingsManager::new()));
    for c in "/scoped-models".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    chat.handle_key(" "); // uncheck the only row
    chat.handle_key("\r"); // refuse
    assert!(chat.scoped_models_picker.is_some(), "the picker stays open");
    assert!(
        chat.world
            .notice
            .as_deref()
            .unwrap_or("")
            .contains("at least one"),
        "the refusal names the rule: {:?}",
        chat.world.notice
    );
}

// Verifies: gh #204 - `a` flips the whole checklist (all on, then all
// off refusing the save like any other empty set).
#[test]
fn scoped_models_a_flips_the_whole_list() {
    let mut opts = options();
    opts.hooks.scoped_models = Some(Arc::new(|| {
        ["aaa", "bbb"]
            .iter()
            .map(|id| crate::state::ScopedModelRow {
                id: id.to_string(),
                label: id.to_string(),
                context: 0,
                enabled: false,
            })
            .collect()
    }));
    opts.hooks.save_scoped_models =
        Some(Arc::new(|ids: Vec<String>| format!("saved {}", ids.len())));
    let mut chat = Chat::new(opts, Arc::new(KeybindingsManager::new()));
    for c in "/scoped-models".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    chat.handle_key("a");
    let viewport = strip(&chat.viewport(100, 30, 0)).join("\n");
    assert!(
        viewport.contains("[x] aaa") && viewport.contains("[x] bbb"),
        "all on:\n{viewport}"
    );
    chat.handle_key("a");
    chat.handle_key("\r");
    assert!(
        chat.scoped_models_picker.is_some(),
        "all off refuses like an empty set"
    );
}

// Verifies: gh #58 - a template command expands into the editor
// (reviewable, never auto-submitted) with arguments substituted.
#[test]
fn a_prompt_template_expands_into_the_editor() {
    let mut opts = options();
    opts.hooks.prompt_templates = Some(Arc::new(|| {
        vec![lca_tools::prompts::PromptTemplate {
            name: "review".to_string(),
            description: "Review staged git changes".to_string(),
            argument_hint: "[focus]".to_string(),
            body: "Review. Focus on ${1:-correctness}.".to_string(),
        }]
    }));
    let mut chat = Chat::new(opts, Arc::new(KeybindingsManager::new()));
    for c in "/review concurrency".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    assert_eq!(
        chat.editor.text(),
        "Review. Focus on concurrency.",
        "the expansion lands in the editor"
    );
    assert!(
        chat.world.notice.is_none(),
        "no notice, no submit: {:?}",
        chat.world.notice
    );
}

// Verifies: gh #58 - the template completes with its description and
// hint, and an unknown `/name` still refuses.
#[test]
fn a_prompt_template_completes_and_unknown_names_refuse() {
    let mut opts = options();
    opts.hooks.prompt_templates = Some(Arc::new(|| {
        vec![lca_tools::prompts::PromptTemplate {
            name: "review".to_string(),
            description: "Review staged git changes".to_string(),
            argument_hint: "[focus]".to_string(),
            body: "Review $1.".to_string(),
        }]
    }));
    use lca_tui::widgets::autocomplete::AutocompleteProvider;
    let provider = super::provider_for(&opts);
    let suggestions = provider.get_suggestions("/rev", false).expect("offers");
    let item = suggestions
        .items
        .iter()
        .find(|item| item.label == "/review")
        .expect("the template completes");
    let description = item.description.as_deref().unwrap_or("");
    assert!(
        description.contains("[focus]") && description.contains("Review staged"),
        "hint and description show: {description}"
    );
    let mut chat = Chat::new(opts, Arc::new(KeybindingsManager::new()));
    for c in "/nope".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    assert_eq!(
        chat.world.notice.as_deref(),
        Some("unknown command /nope"),
        "no template, no command"
    );
}

// Verifies: gh #131 - `/changelog` shows the latest released section,
// never the Unreleased block.
#[test]
fn changelog_shows_the_latest_released_section() {
    let mut chat = chat();
    for c in "/changelog".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    // gh #234: the section rides the transcript now, not the dock.
    assert!(
        chat.world.notice.is_none(),
        "the dock stays clean: {:?}",
        chat.world.notice
    );
    let text: String = chat
        .transcript
        .entries()
        .iter()
        .filter_map(|entry| match entry {
            crate::transcript::Entry::Notice(body) => Some(body.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        text.contains("0.6.0") && !text.contains("[Unreleased]"),
        "the latest release, not the work in progress: {text:.200}"
    );
}

// Verifies: gh #132 - Esc Esc with an empty editor opens the tree
// (pi's double-escape, 500 ms window); one Esc only arms.
#[test]
fn double_escape_opens_the_tree() {
    let mut opts = options();
    opts.hooks.session_tree = Some(Arc::new(|| vec![tree_row("root", 0, "root (session)")]));
    opts.hooks.double_escape_action = Some(Arc::new(|| "tree".to_string()));
    let mut chat = Chat::new(opts, Arc::new(KeybindingsManager::new()));
    chat.handle_key("\x1b");
    assert!(chat.tree_picker.is_none(), "the first escape only arms");
    chat.handle_key("\x1b");
    assert!(chat.tree_picker.is_some(), "the second escape acts");
}

// Verifies: gh #132 - the fork action opens the fork picker instead,
// and none does nothing at all.
#[test]
fn double_escape_fork_and_none() {
    let mut opts = options();
    opts.hooks.double_escape_action = Some(Arc::new(|| "fork".to_string()));
    let mut chat = Chat::new(opts, Arc::new(KeybindingsManager::new()));
    chat.transcript.push_user("branch me");
    chat.handle_key("\x1b");
    chat.handle_key("\x1b");
    assert!(chat.fork_picker.is_some(), "fork opens the message picker");

    let mut opts = options();
    opts.hooks.session_tree = Some(Arc::new(|| vec![tree_row("root", 0, "root (session)")]));
    opts.hooks.double_escape_action = Some(Arc::new(|| "none".to_string()));
    let mut chat = Chat::new(opts, Arc::new(KeybindingsManager::new()));
    chat.handle_key("\x1b");
    chat.handle_key("\x1b");
    assert!(
        chat.tree_picker.is_none() && chat.fork_picker.is_none(),
        "none opens nothing"
    );
}

// Verifies: gh #82 - the tick syncs tunables live: a hook change
// reaches the editor and transcript without a restart.
#[test]
fn tick_syncs_display_tuning_live() {
    let mut opts = options();
    opts.hooks.display_tuning = Some(Arc::new(|| crate::state::DisplayTuning {
        autocomplete_max_visible: 8,
        editor_padding_x: 2,
        ..crate::state::DisplayTuning::default()
    }));
    let mut chat = Chat::new(opts, Arc::new(KeybindingsManager::new()));
    assert_eq!(chat.editor.max_visible, 5, "defaults before the tick");
    chat.tick();
    assert_eq!(chat.editor.max_visible, 8, "the hook wins");
    assert_eq!(chat.editor.padding_x, 2, "padding follows");
}

// Verifies: FR-UI-16 - `/tree` selection without a host stays a
// notice, and `/rename` names the session through the host (gh #37).
#[test]
fn tree_selection_and_rename_refuse_without_a_host() {
    let mut chat = chat();
    chat.tree_picker = Some(crate::chat_pickers::TreePicker::new(
        vec![tree_row("r1", 0, "first")],
        crate::chat_pickers::TreeFilter::Default,
    ));
    chat.handle_key("\r");
    assert!(
        chat.world
            .notice
            .as_deref()
            .unwrap()
            .contains("not available in this host"),
        "{:?}",
        chat.world.notice
    );
    for c in "/rename new name".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    assert!(
        chat.world
            .notice
            .as_deref()
            .unwrap()
            .contains("not available in this host"),
        "{:?}",
        chat.world.notice
    );
}

// Verifies: FR-UI-16 - `/rename <name>` renames through the host
// (gh #37).
#[test]
fn rename_names_the_session_through_the_host() {
    let mut options = options();
    options.hooks.rename_session = Some(Arc::new(|name: &str| {
        assert_eq!(name, "fresh title");
        Ok("renamed to 'fresh title'".to_string())
    }));
    let mut chat = Chat::new(options, Arc::new(KeybindingsManager::new()));
    for c in "/rename fresh title".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    assert!(
        chat.world
            .notice
            .as_deref()
            .unwrap()
            .contains("fresh title"),
        "{:?}",
        chat.world.notice
    );
}

// Verifies: gh #233 - dispatching a non-model command never touches
// the model catalog hook (dispatch stays instant).
#[test]
fn dispatching_help_never_enumerates_models() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    let mut options = options();
    options.hooks.models = Some(Arc::new(move || {
        counted.fetch_add(1, Ordering::SeqCst);
        vec![("m".to_string(), "M".to_string())]
    }));
    let mut chat = Chat::new(options, Arc::new(KeybindingsManager::new()));
    for command in [
        "/help",
        "/theme",
        "/trust",
        "/settings",
        "/hotkeys",
        "/session",
    ] {
        for c in command.chars() {
            chat.handle_key(&c.to_string());
        }
        chat.handle_key("\r");
        // gh #232: pickers own the keyboard while open; closing keeps
        // every command dispatching from a clean editor.
        chat.handle_key("\x1b");
        chat.editor.set_text("");
    }
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "no enumeration off the model arm"
    );
}

// Verifies: gh #233 - the model catalog snapshots: the first bare
// `/model` enumerates once, the second reuses the snapshot, and a
// login step invalidates it.
#[test]
fn model_catalog_snapshots_and_login_invalidates() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    let mut options = options();
    options.hooks.models = Some(Arc::new(move || {
        counted.fetch_add(1, Ordering::SeqCst);
        vec![("m".to_string(), "M".to_string())]
    }));
    let mut chat = Chat::new(options, Arc::new(KeybindingsManager::new()));
    // gh #232: the drain makes each background arrival deterministic.
    for c in "/model".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    assert!(chat.model_picker.is_some());
    chat.drain_model_refresh();
    chat.handle_key("\x1b");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    for c in "/model".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    chat.drain_model_refresh();
    chat.handle_key("\x1b");
    assert_eq!(calls.load(Ordering::SeqCst), 1, "snapshot reused");
    chat.apply_login_next(crate::state::LoginNext::Message("signed in".to_string()));
    for c in "/model".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    chat.drain_model_refresh();
    assert_eq!(calls.load(Ordering::SeqCst), 2, "login invalidates");
}

// Verifies: gh #233 - switch, reload, and revoke events invalidate
// the snapshot, so the next `/model` re-enumerates.
#[test]
fn switch_reload_and_revoke_invalidate_the_snapshot() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    let mut options = options();
    options.hooks.models = Some(Arc::new(move || {
        counted.fetch_add(1, Ordering::SeqCst);
        vec![("m".to_string(), "M".to_string())]
    }));
    options.hooks.switch_session = Some(Arc::new(|_: &str| Some(vec![])));
    options.hooks.reload = Some(Arc::new(|| crate::state::ReloadReport {
        notice: "reloaded".to_string(),
        themes: Vec::new(),
        key_bindings: Default::default(),
        keybinding_error: None,
    }));
    options.hooks.revoke_grant = Some(Arc::new(|_: &crate::state::GrantEntry| {
        "revoked".to_string()
    }));
    let mut chat = Chat::new(options, Arc::new(KeybindingsManager::new()));
    fn model(chat: &mut Chat) {
        for c in "/model".chars() {
            chat.handle_key(&c.to_string());
        }
        chat.handle_key("\r");
        // gh #232: the drain makes each background arrival deterministic.
        chat.drain_model_refresh();
        chat.handle_key("\x1b");
    }
    model(&mut chat);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    chat.switch_or_announce("other");
    model(&mut chat);
    assert_eq!(calls.load(Ordering::SeqCst), 2, "switch invalidates");
    for c in "/reload".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    model(&mut chat);
    assert_eq!(calls.load(Ordering::SeqCst), 3, "reload invalidates");
    chat.revoke_grant(&crate::state::GrantEntry {
        install_consent: false,
        subject: "s".to_string(),
        detail: "d".to_string(),
        revocable: true,
    });
    model(&mut chat);
    assert_eq!(calls.load(Ordering::SeqCst), 4, "revoke invalidates");
}

// Verifies: gh #234 - informational multi-line outputs ride the
// transcript as scrollable entries; the dock stays a 1-2 line anchor.
#[test]
fn informational_outputs_route_to_the_transcript_not_the_dock() {
    let mut opts = options();
    opts.hooks.list_labels = Some(Arc::new(|| {
        vec![
            ("aaa".to_string(), "r1".to_string()),
            ("bbb".to_string(), "r2".to_string()),
        ]
    }));
    let mut chat = Chat::new(opts, Arc::new(KeybindingsManager::new()));
    for command in ["/help", "/hotkeys", "/changelog", "/labels"] {
        for c in command.chars() {
            chat.handle_key(&c.to_string());
        }
        chat.handle_key("\r");
    }
    assert!(
        chat.world.notice.is_none(),
        "the dock stays clean: {:?}",
        chat.world.notice
    );
    let text: String = chat
        .transcript
        .entries()
        .iter()
        .filter_map(|entry| match entry {
            crate::transcript::Entry::Notice(body) => Some(body.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    for marker in ["commands:", "keys:", "0.6.0", "aaa -> r1"] {
        assert!(text.contains(marker), "the transcript carries {marker}");
    }
}

// Verifies: gh #234 - short confirmations and errors stay dock
// notices; only unbounded outputs move to the transcript.
#[test]
fn short_confirmations_stay_dock_notices() {
    let mut opts = options();
    opts.hooks.list_labels = Some(Arc::new(Vec::new));
    let mut chat = Chat::new(opts, Arc::new(KeybindingsManager::new()));
    for c in "/labels".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    assert_eq!(
        chat.world.notice.as_deref(),
        Some("no bookmarks yet; /label <name> marks the latest message"),
        "the one-line hint stays a notice"
    );
    assert_eq!(
        chat.transcript.entries().len(),
        1,
        "nothing scrollable was said (the seed hint line stands alone)"
    );
    for c in "/nope".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    assert_eq!(
        chat.world.notice.as_deref(),
        Some("unknown command /nope"),
        "errors stay notices"
    );
}

// Verifies: gh #232 - the picker opens instantly while discovery runs:
// dispatch returns on the same frame with a loading state up, and the
// rows fill in when the background enumeration lands.
#[test]
fn model_picker_opens_instantly_while_discovery_runs() {
    use std::sync::Barrier;
    use std::sync::atomic::{AtomicBool, Ordering};
    let gate = Arc::new(Barrier::new(2));
    let held = gate.clone();
    let started = Arc::new(AtomicBool::new(false));
    let flag = started.clone();
    let mut opts = options();
    opts.models = Vec::new();
    opts.hooks.models = Some(Arc::new(move || {
        flag.store(true, Ordering::SeqCst);
        held.wait();
        vec![("m".to_string(), "M".to_string())]
    }));
    let mut chat = Chat::new(opts, Arc::new(KeybindingsManager::new()));
    for c in "/model".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    let picker = chat.model_picker.as_ref().expect("opens on the same frame");
    assert!(picker.loading, "the loading state shows");
    assert!(picker.models.is_empty(), "no rows yet: nothing blocked");
    let frame = chat.viewport(80, 24, 0).join("\n");
    assert!(
        frame.contains("Loading"),
        "the painter says so:\n{frame:.500}"
    );
    gate.wait();
    chat.drain_model_refresh();
    let picker = chat.model_picker.as_ref().expect("still open");
    assert!(!picker.loading, "loading clears");
    assert_eq!(picker.models.len(), 1, "the rows filled in");
    assert!(started.load(Ordering::SeqCst), "the hook ran off-thread");
}

// Verifies: gh #232 + #233 - a cached catalog opens with rows and no
// refresh thread: the snapshot IS the instant path.
#[test]
fn cached_catalog_opens_with_rows_and_no_refresh_thread() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    let mut opts = options();
    opts.models = Vec::new();
    opts.hooks.models = Some(Arc::new(move || {
        counted.fetch_add(1, Ordering::SeqCst);
        vec![("m".to_string(), "M".to_string())]
    }));
    let mut chat = Chat::new(opts, Arc::new(KeybindingsManager::new()));
    for c in "/model".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    chat.drain_model_refresh();
    chat.handle_key("\x1b");
    assert_eq!(calls.load(Ordering::SeqCst), 1, "one enumeration");
    for c in "/model".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    let picker = chat.model_picker.as_ref().expect("opens");
    assert!(!picker.loading, "no refresh: the snapshot answers");
    assert_eq!(picker.models.len(), 1, "rows on the same frame");
    assert_eq!(calls.load(Ordering::SeqCst), 1, "still one enumeration");
}

// Verifies: gh #232 - an empty discovery closes the loading picker and
// runs the extension `model` command (the gh #31 consent path survives
// the async turn).
#[test]
fn empty_discovery_falls_through_to_consent() {
    use std::sync::Mutex;
    let invoked = Arc::new(Mutex::new(Vec::new()));
    let recorded = invoked.clone();
    let mut opts = options();
    opts.models = Vec::new();
    opts.hooks.models = Some(Arc::new(Vec::new));
    opts.invoke_command = Arc::new(move |name: &str, argument: &str| {
        recorded
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push((name.to_string(), argument.to_string()));
        lca_protocol::CommandEffect::None
    });
    let mut chat = Chat::new(opts, Arc::new(KeybindingsManager::new()));
    for c in "/model".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    assert!(chat.model_picker.is_some(), "loading opens first");
    chat.drain_model_refresh();
    assert!(
        chat.model_picker.is_none(),
        "empty discovery closes the loader"
    );
    assert_eq!(
        invoked
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_slice(),
        &[("model".to_string(), String::new())],
        "the extension command runs consent"
    );
}

// Verifies: gh #231 - the DAG navigator paints branch connectors: a
// fork shows `├─`/`└─` with a `│` gutter, role markers name each row.
#[test]
fn tree_paints_connectors_and_role_markers() {
    use crate::chat_pickers::{TreeFilter, TreePicker};
    use crate::state::{TreeRow, TreeRowKind};
    let row = |id: &str, depth: usize, kind: TreeRowKind, text: &str| TreeRow {
        id: id.to_string(),
        depth,
        kind,
        text: text.to_string(),
        label: None,
        live: false,
    };
    let picker = TreePicker::new(
        vec![
            row("u1", 0, TreeRowKind::User, "first"),
            row("a1", 1, TreeRowKind::Assistant, "reply"),
            row("u2", 1, TreeRowKind::User, "second path"),
        ],
        TreeFilter::Default,
    );
    let painted = picker.paint_rows().join("\n");
    assert!(painted.contains("user: first"), "role markers:\n{painted}");
    assert!(
        painted.contains("assistant: reply"),
        "role markers:\n{painted}"
    );
    assert!(
        painted.contains("├─") && painted.contains("└─"),
        "fork connectors:\n{painted}"
    );
}

// Verifies: gh #231 - filter modes hide by kind (default drops tool
// rows, no-tools drops tool-call assistants too, user-only keeps
// prompts, labeled-only keeps marks, all shows everything).
#[test]
fn tree_filters_by_kind_and_label() {
    use crate::chat_pickers::{TreeFilter, TreePicker};
    use crate::state::{TreeRow, TreeRowKind};
    let row = |id: &str, kind: TreeRowKind, text: &str, label: Option<&str>| TreeRow {
        id: id.to_string(),
        depth: 0,
        kind,
        text: text.to_string(),
        label: label.map(str::to_string),
        live: false,
    };
    let rows = vec![
        row("u", TreeRowKind::User, "ask", None),
        row("a", TreeRowKind::Assistant, "answer", Some("mark")),
        row("t", TreeRowKind::Tool, "read", None),
        row("c", TreeRowKind::Assistant, "(tool call)", None),
    ];
    let texts = |filter: TreeFilter| {
        TreePicker::new(rows.clone(), filter)
            .paint_rows()
            .join("\n")
    };
    let default = texts(TreeFilter::Default);
    assert!(
        default.contains("ask") && default.contains("answer") && !default.contains("tool: read"),
        "default hides tool rows:\n{default}"
    );
    let no_tools = texts(TreeFilter::NoTools);
    assert!(
        !no_tools.contains("tool: read") && !no_tools.contains("(tool call)"),
        "no-tools hides tool traffic:\n{no_tools}"
    );
    let user_only = texts(TreeFilter::UserOnly);
    assert!(
        user_only.contains("ask") && !user_only.contains("answer"),
        "user-only keeps prompts:\n{user_only}"
    );
    let labeled = texts(TreeFilter::LabeledOnly);
    assert!(
        labeled.contains("answer") && !labeled.contains("ask"),
        "labeled-only keeps marks:\n{labeled}"
    );
    let all = texts(TreeFilter::All);
    assert!(
        all.contains("tool: read") && all.contains("(tool call)"),
        "all shows everything:\n{all}"
    );
}

// Verifies: gh #231 - folding hides a subtree and unfolding restores
// it; the selection survives a filter cycle by record id.
#[test]
fn tree_fold_hides_and_filter_cycle_keeps_selection() {
    use crate::chat_pickers::{TreeFilter, TreePicker};
    use crate::state::{TreeRow, TreeRowKind};
    let row = |id: &str, depth: usize| TreeRow {
        id: id.to_string(),
        depth,
        kind: TreeRowKind::User,
        text: id.to_string(),
        label: None,
        live: false,
    };
    let mut picker = TreePicker::new(
        vec![row("a", 0), row("b", 1), row("c", 1), row("d", 0)],
        TreeFilter::Default,
    );
    assert_eq!(picker.visible_len(), 4);
    picker.toggle_fold_at(0); // fold "a": its children hide
    assert_eq!(picker.visible_len(), 2, "b and c hide");
    picker.toggle_fold_at(0); // unfold: the subtree returns
    assert_eq!(picker.visible_len(), 4, "b and c return");
    picker.selected = 2; // "c"
    picker.set_filter(TreeFilter::UserOnly);
    assert_eq!(
        picker.selected_id().as_deref(),
        Some("c"),
        "selection rides the id"
    );
    picker.set_filter(TreeFilter::LabeledOnly); // nothing labeled
    assert_eq!(picker.visible_len(), 0, "empty views hold");
}

// Verifies: gh #231 - Left folds a subtree (Right unfolds), `f`
// cycles the filter forward with the selection riding its record.
#[test]
fn tree_fold_and_filter_cycle_through_keys() {
    let mut opts = options();
    opts.hooks.session_tree = Some(Arc::new(|| {
        vec![
            tree_row("a", 0, "first"),
            tree_row("b", 1, "child one"),
            tree_row("c", 1, "child two"),
        ]
    }));
    let mut chat = Chat::new(opts, Arc::new(KeybindingsManager::new()));
    for c in "/tree".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    let picker = chat.tree_picker.as_ref().expect("open");
    assert_eq!(picker.visible_len(), 3);
    chat.handle_key("\x1b[D"); // Left: fold "a"
    assert_eq!(
        chat.tree_picker.as_ref().expect("open").visible_len(),
        1,
        "the children hide"
    );
    chat.handle_key("\x1b[C"); // Right on a folded node opens it
    assert_eq!(
        chat.tree_picker.as_ref().expect("open").visible_len(),
        3,
        "the children return"
    );
    chat.handle_key("j");
    chat.handle_key("j"); // on "c"
    chat.handle_key("f"); // default -> no-tools
    let picker = chat.tree_picker.as_ref().expect("open");
    assert_eq!(
        picker.filter,
        crate::chat_pickers::TreeFilter::NoTools,
        "the filter cycled"
    );
    assert_eq!(
        picker.selected_id().as_deref(),
        Some("c"),
        "the selection rides its record"
    );
    let viewport = strip(&chat.viewport(100, 30, 0)).join("\n");
    assert!(
        viewport.contains("no-tools"),
        "the header names it:\n{viewport}"
    );
}

// Verifies: gh #231 - `e` edits the selected row's bookmark through
// the host hook: typing lands in the buffer, Enter commits and the
// rebuilt tree carries the mark.
#[test]
fn tree_label_edit_commits_through_the_hook() {
    use std::sync::Mutex;
    let saved = Arc::new(Mutex::new(Vec::new()));
    let written = saved.clone();
    let mut opts = options();
    opts.hooks.session_tree = Some(Arc::new(|| vec![tree_row("r1", 0, "first")]));
    opts.hooks.label_record = Some(Arc::new(move |id: &str, name: &str| {
        written
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push((id.to_string(), name.to_string()));
        Ok(format!("bookmarked '{name}'"))
    }));
    let mut chat = Chat::new(opts, Arc::new(KeybindingsManager::new()));
    for c in "/tree".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    chat.handle_key("e");
    assert!(
        chat.tree_picker.as_ref().expect("open").editing.is_some(),
        "the buffer opens"
    );
    for c in "mine".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    assert_eq!(
        saved
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_slice(),
        &[("r1".to_string(), "mine".to_string())],
        "the hook bookmarks by record id"
    );
    assert_eq!(
        chat.world.notice.as_deref(),
        Some("bookmarked 'mine'"),
        "the hook's notice shows"
    );
    assert!(
        chat.tree_picker.as_ref().expect("open").editing.is_none(),
        "the buffer closed"
    );
}

// Verifies: FR-UI-16 - `/tree` browses the live session's entry
// tree and selecting a row branches there in place (gh #37 phase 3:
// record rows replace the fork-directory rows; fork switching lives
// on `/resume`, forking on `/fork`).
#[test]
fn tree_browses_entry_branches_and_selection_branches_there() {
    let mut options = options();
    options.hooks.session_tree = Some(Arc::new(|| {
        vec![
            tree_row("r1", 0, "first question"),
            crate::state::TreeRow {
                id: "r2".to_string(),
                depth: 1,
                kind: crate::state::TreeRowKind::User,
                text: "second question".to_string(),
                label: None,
                live: true,
            },
        ]
    }));
    options.hooks.branch_here = Some(Arc::new(|id: &str| {
        assert_eq!(id, "r2");
        Some(vec![])
    }));
    options.hooks.fork_at = Some(Arc::new(|n: usize| crate::state::ForkReport {
        id: Some("newbranch".to_string()),
        notice: format!("forked at {n}: newbranch"),
    }));
    let mut chat = Chat::new(options, Arc::new(KeybindingsManager::new()));
    for c in "/tree".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    assert!(chat.tree_picker.is_some());
    let viewport = strip(&chat.viewport(100, 30, 0)).join("\n");
    assert!(viewport.contains("first question"), "{viewport}");
    assert!(viewport.contains("second question"), "{viewport}");
    chat.handle_key("j");
    chat.handle_key("\r");
    assert!(
        chat.world.notice.as_deref().unwrap().contains("branched"),
        "{:?}",
        chat.world.notice
    );
    for c in "/fork 1".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    assert!(
        chat.world
            .notice
            .as_deref()
            .unwrap()
            .contains("forked at 1"),
        "{:?}",
        chat.world.notice
    );
}

// Verifies: gh #75 (pi's `/name`) - `/name` renames the live session
// like `/rename`, through the same host seam.
#[test]
fn slash_name_renames_through_the_host() {
    let mut opts = options();
    opts.hooks.rename_session = Some(Arc::new(|name: &str| Ok(format!("renamed to '{name}'"))));
    let mut chat = Chat::new(opts, Arc::new(KeybindingsManager::new()));
    for c in "/name my title".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    assert_eq!(
        chat.world.notice.as_deref(),
        Some("renamed to 'my title'"),
        "the host seam answers"
    );
}
