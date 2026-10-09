//! Slash-command and key behaviors (ceiling split from
//! `chat_tests.rs`): clone, message-copy target, the resume picker,
//! and reload. Shares the `chat_tests` helpers.

use super::{chat, options, strip};
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
    let notice = chat.world.notice.as_deref().expect("a notice shows");
    assert!(
        notice.contains("0.6.0") && !notice.contains("[Unreleased]"),
        "the latest release, not the work in progress: {notice:.200}"
    );
}

// Verifies: gh #132 - Esc Esc with an empty editor opens the tree
// (pi's double-escape, 500 ms window); one Esc only arms.
#[test]
fn double_escape_opens_the_tree() {
    let mut opts = options();
    opts.hooks.session_tree = Some(Arc::new(|| {
        vec![("root".to_string(), "root (session)".to_string())]
    }));
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
    opts.hooks.session_tree = Some(Arc::new(|| {
        vec![("root".to_string(), "root (session)".to_string())]
    }));
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
    chat.tree_picker = Some(crate::chat_pickers::TreePicker {
        entries: vec![("r1".into(), "first".into())],
        selected: 0,
    });
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
