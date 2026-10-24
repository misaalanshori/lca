//! Slash-command and key behaviors (ceiling split from
//! `chat_tests.rs`): clone, message-copy target, the resume picker,
//! and reload. Shares the `chat_tests` helpers.

use super::{chat, options, strip, tree_row};
use lca_tui::engine::keybindings::KeybindingsManager;

use super::super::Chat;
use crate::state::LoginNext;
use std::sync::Arc;

/// The role a `chain.toml` naming only `accent` should move through its
/// children; `mdLink` is the one the live pane showed.
fn lca_ui_role() -> crate::theme::Role {
    crate::theme::Role::MdLink
}

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
    let notice = chat.last_notice().unwrap_or_default();
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
            modified_ms: 0,
            parent: None,
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
        chat.last_notice().as_deref(),
        Some("no sessions yet"),
        "the empty case says so"
    );
}

// Verifies: gh #130 - `/reload` runs the host hook and applies the
// report interface-side: the notice names it, themes refresh, and the
// key manager reloads (refused mid-turn like a switch).
// Verifies: gh #387 gap 5 - `/reload` re-reads the active theme file, not just
// the list of names.
//
// Found while doing the live verification for this batch: a theme that sets one
// parent role had its children following correctly at startup, then editing the
// file and running `/reload` changed nothing on screen - while the notice said
// "reloaded: ... themes ...". The claim was false. The host hook reports *names*
// for the picker; the interface owns its state, so re-applying belongs here.
//
// The row proves the theme object itself is rebuilt, not merely that the file
// changed on disk: `set_theme` reads the file, so a same-process edit is picked
// up exactly as a restart would.
#[test]
fn reload_reapplies_the_active_theme() {
    let dir = std::env::temp_dir().join(format!("lca-reload-theme-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    let theme_path = dir.join("chain.toml");
    std::fs::write(&theme_path, "accent = \"#00ff88\"\n").expect("write theme");

    let mut opts = options();
    // The shared fixture is `plain: true` (most rows here do not care about
    // colour); a plain theme would make this row vacuous.
    opts.plain = false;
    opts.theme = "chain".to_string();
    opts.theme_extra_dirs = vec![dir.clone()];
    opts.hooks.reload = Some(Arc::new(|| crate::state::ReloadReport {
        notice: "reloaded: 0 extensions, settings, prompts, themes, keybindings".to_string(),
        themes: vec!["chain".to_string()],
        key_bindings: Default::default(),
        keybinding_error: None,
    }));
    let mut chat = Chat::new(opts, Arc::new(KeybindingsManager::new()));
    let first = chat.theme.palette.get(lca_ui_role());
    assert_eq!(
        first,
        crate::theme::Color::Rgb(0x00, 0xff, 0x88),
        "the theme loads at construction"
    );

    // Edit the file, then reload.
    std::fs::write(&theme_path, "accent = \"#ff5577\"\n").expect("rewrite theme");
    for c in "/reload".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    assert_eq!(
        chat.theme.palette.get(lca_ui_role()),
        crate::theme::Color::Rgb(0xff, 0x55, 0x77),
        "`/reload` must re-read the active theme file: the notice says themes \
         were reloaded, so the colours on screen have to change with it"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

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
    let notice = chat.last_notice().unwrap_or_default();
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
        chat.last_notice().as_deref(),
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
    let notice = chat.last_notice().unwrap_or_default();
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
        chat.last_notice()
            .as_deref()
            .unwrap_or("")
            .contains("Forked from turn 0"),
        "the switch names the fork: {:?}",
        chat.last_notice()
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
        chat.last_notice().as_deref(),
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
        chat.last_notice()
            .as_deref()
            .unwrap_or("")
            .contains("at least one"),
        "the refusal names the rule: {:?}",
        chat.last_notice()
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
        chat.last_notice().is_none(),
        "no notice, no submit: {:?}",
        chat.last_notice()
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
        chat.last_notice().as_deref(),
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
        chat.toasts.is_empty(),
        "the dock stays clean: {:?}",
        chat.toasts
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
    // Gh #399 changed this premise: the constructor now syncs the tuning once, so
    // the hook's value is in place **before** the first frame rather than only
    // after a tick. The startup header reads its version and rung from the tuning,
    // and a caller that renders a fresh `Chat` without ticking (a screenshot, a
    // resumed session, a test) was otherwise drawing the defaults.
    assert_eq!(
        chat.editor.max_visible, 8,
        "the hook wins from construction, not from the first tick"
    );
    chat.tick();
    assert_eq!(chat.editor.max_visible, 8, "and the tick keeps it");
    assert_eq!(chat.editor.padding_x, 2, "padding follows");
    // The live behaviour the row was written for still holds: a hook that answers
    // differently on a later tick reaches the editor without a restart. Driven
    // through a cell rather than a test-only setter, so this is the real path.
    let visible = Arc::new(std::sync::atomic::AtomicUsize::new(6));
    let cell = visible.clone();
    let mut opts = options();
    opts.hooks.display_tuning = Some(Arc::new(move || crate::state::DisplayTuning {
        autocomplete_max_visible: cell.load(std::sync::atomic::Ordering::SeqCst) as u8,
        editor_padding_x: 3,
        ..crate::state::DisplayTuning::default()
    }));
    let mut live = Chat::new(opts, Arc::new(KeybindingsManager::new()));
    assert_eq!(live.editor.max_visible, 6, "the initial answer");
    visible.store(7, std::sync::atomic::Ordering::SeqCst);
    live.tick();
    assert_eq!(live.editor.max_visible, 7, "a live change still lands");
    assert_eq!(live.editor.padding_x, 3, "padding too");
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
        chat.last_notice()
            .as_deref()
            .unwrap()
            .contains("not available in this host"),
        "{:?}",
        chat.last_notice()
    );
    for c in "/rename new name".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    assert!(
        chat.last_notice()
            .as_deref()
            .unwrap()
            .contains("not available in this host"),
        "{:?}",
        chat.last_notice()
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
        chat.last_notice()
            .as_deref()
            .unwrap()
            .contains("fresh title"),
        "{:?}",
        chat.last_notice()
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
        vec![crate::ModelRow::from(("m".to_string(), "M".to_string()))]
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
        vec![crate::ModelRow::from(("m".to_string(), "M".to_string()))]
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
        vec![crate::ModelRow::from(("m".to_string(), "M".to_string()))]
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
        chat.toasts.is_empty(),
        "the dock stays clean: {:?}",
        chat.toasts
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

// Verifies: gh #239 - the dock catch-all is retired: short
// confirmations and errors land in the transcript like every other
// status line (pi's `showStatus`), superseding #234's dock split.
#[test]
fn short_confirmations_land_in_the_transcript() {
    let mut opts = options();
    opts.hooks.list_labels = Some(Arc::new(Vec::new));
    let mut chat = Chat::new(opts, Arc::new(KeybindingsManager::new()));
    for c in "/labels".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    assert_eq!(
        chat.last_notice().as_deref(),
        Some("no bookmarks yet; /label <name> marks the latest message"),
        "the one-line hint lands in history"
    );
    assert_eq!(
        chat.transcript.entries().len(),
        2,
        "the seed hint line plus the labels hint"
    );
    for c in "/nope".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    assert_eq!(
        chat.last_notice().as_deref(),
        Some("unknown command /nope"),
        "errors land in history"
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
        vec![crate::ModelRow::from(("m".to_string(), "M".to_string()))]
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
        vec![crate::ModelRow::from(("m".to_string(), "M".to_string()))]
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
        chat.last_notice().as_deref(),
        Some("renamed to 'my title'"),
        "the host seam answers"
    );
}

// Verifies: gh #242 - a model advertising no reasoning levels gets a
// notice naming it, not an empty picker.
#[test]
fn a_model_without_reasoning_gets_a_notice_not_a_picker() {
    let mut chat = chat();
    chat.world.options.hooks.thinking_offered = Some(std::sync::Arc::new(Vec::new));
    chat.open_thinking_picker();
    assert!(chat.thinking_picker.is_none(), "no picker opens");
    let notice = chat.last_notice().unwrap_or_default();
    assert!(
        notice.contains("does not support thinking"),
        "the notice names the gap: {notice:?}"
    );
}

// Verifies: gh #254 - Enter on open forks immediately at a forkable
// message. After an `all`-mode flush (one joined record), the default
// cursor is index 0 — before the fix the phantom second entry made
// Enter fork index 1, and the host answered "no user message at
// index 1".
#[test]
fn fork_enter_on_open_forks_the_latest_record() {
    use std::sync::Mutex;
    let mut opts = options();
    let forked_at = Arc::new(Mutex::new(Vec::new()));
    let seen = forked_at.clone();
    opts.hooks.fork_at = Some(Arc::new(move |n: usize| {
        seen.lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(n);
        crate::state::ForkReport {
            id: Some("forked-id".to_string()),
            notice: format!("✓ Forked from turn {n} (session: forked-id)"),
        }
    }));
    opts.hooks.switch_session = Some(Arc::new(|_id: &str| Some(Vec::new())));
    let mut chat = Chat::new(opts, Arc::new(KeybindingsManager::new()));
    chat.follow_up_mode = lca_protocol::QueueMode::All;
    chat.queue_submit("first".into(), lca_protocol::SubmitMode::FollowUp);
    chat.queue_submit("second".into(), lca_protocol::SubmitMode::FollowUp);
    assert_eq!(chat.take_next_pending().as_deref(), Some("first\n\nsecond"));
    for c in "/fork".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    let picker = chat.fork_picker.as_ref().expect("picker opens");
    assert_eq!(picker.selected, 0, "the one record is the default");
    chat.handle_key("\r");
    assert_eq!(
        forked_at
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_slice(),
        &[0],
        "Enter on open forks index 0"
    );
    assert_eq!(chat.editor.text(), "first\n\nsecond", "the text restores");
}

// Verifies: gh #408 side-by-side - **a command is offered once.** The host's
// `slash_commands()` inserts `/settings` explicitly and also derives it from
// `HANDLED_NAMES`, so the popup showed two identical rows (pi shows one). The
// duplication is upstream in `lca-cli` and that tree is out of this cycle's
// boundary, so the dedupe lands where the list becomes suggestions - which is also
// the layer that must never offer the same thing twice.
#[test]
fn a_command_appears_in_the_popup_only_once() {
    let mut opts = options();
    opts.slash_commands = vec![
        "/help".into(),
        "/settings".into(),
        "/settings".into(),
        "/model".into(),
        "/model".into(),
        "/model".into(),
    ];
    use lca_tui::widgets::autocomplete::AutocompleteProvider;
    let provider = super::provider_for(&opts);
    let suggestions = provider.get_suggestions("/set", false).expect("offers");
    assert_eq!(
        suggestions.items.len(),
        1,
        "one row per command, however many times the host lists it: {:?}",
        suggestions
            .items
            .iter()
            .map(|i| &i.label)
            .collect::<Vec<_>>()
    );
    let model = provider.get_suggestions("/mod", false).expect("offers");
    assert_eq!(model.items.len(), 1, "and the same for a repeated name");
}
