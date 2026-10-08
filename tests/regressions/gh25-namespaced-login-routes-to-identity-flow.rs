//! GitHub issue #25: `/antigravity.login` opened the preset picker instead
//! of antigravity's OAuth flow, because the interface intercepted *any*
//! command ending in `.login` and handed it to the `/login` options seam -
//! which the CLI's namespaced identity handler (`commands.rs`) is written
//! to own but could never reach.
//!
//! The routing table this pins, at the interface seam where the choice is
//! made (ADR-0033, FR-PROV-10/11):
//!
//! | input | seam |
//! |---|---|
//! | `/login` (bare) | the options seam |
//! | `/login <option-id>` | the options seam (the scripted journey) |
//! | `/login <provider>` | the options seam - the host's resolver decides |
//! | `/<provider>.login` | the host's command dispatch (identity export) |
//! | `/<unknown>.login` | the unknown-command error, never either seam |
//!
//! The options seam's own half of the table - a provider with **no** picker
//! options must run its identity `login` export rather than an empty picker
//! whose only row is the host's universal `Custom endpoint…` - lives with
//! the resolver in `crates/lca-cli/src/tui/login.rs` and is guarded beside
//! it (`login_scoped_to_a_provider_with_no_options_asks_for_its_identity_flow`),
//! because that is where the option count is known. The end-to-end receipt
//! for both halves is the tmux drive in the closing report.
//!
//! Verifies: NFR-24 (a released defect's guard), gh #25.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use lca_protocol::CommandEffect;
use lca_tui::engine::keybindings::KeybindingsManager;
use lca_ui::{Chat, LoginNext, UiHooks, UiOptions};

/// One shared call log: every seam records what it was handed, in order.
#[derive(Clone, Default)]
struct Seams {
    calls: Arc<Mutex<Vec<String>>>,
}

impl Seams {
    fn record(&self, entry: String) {
        self.calls
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(entry);
    }

    fn log(&self) -> Vec<String> {
        self.calls.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// The options seam's arguments, in order (`login:<arg>` entries).
    fn options_args(&self) -> Vec<String> {
        self.log()
            .into_iter()
            .filter_map(|entry| entry.strip_prefix("options:").map(str::to_string))
            .collect()
    }

    /// The command-dispatch seam's calls (`invoke:<name>:<arg>` entries).
    fn invocations(&self) -> Vec<String> {
        self.log()
            .into_iter()
            .filter_map(|entry| entry.strip_prefix("invoke:").map(str::to_string))
            .collect()
    }
}

fn chat(seams: &Seams) -> Chat {
    let options_seams = seams.clone();
    let invoke_seams = seams.clone();
    let options = UiOptions {
        prompt_slot: Default::default(),
        dialog_slot: Default::default(),
        pending_models: None,
        model_label: Arc::new(Mutex::new("p/m".into())),
        context_window: Arc::new(Mutex::new(0)),
        thinking: Arc::new(Mutex::new(None)),
        theme: "auto".to_string(),
        theme_dir: PathBuf::new(),
        themes: lca_ui::theme::THEMES
            .iter()
            .map(|s| s.to_string())
            .collect(),
        initial_lines: Vec::new(),
        initial_records: Vec::new(),
        initial_tail_lines: Vec::new(),
        initial_messages: Vec::new(),
        open_resume_picker: false,
        yolo: false,
        thinking_visibility: Default::default(),
        codeblock_border: Default::default(),
        plain: true,
        invoke_command: Arc::new(move |name, argument| {
            invoke_seams.record(format!("invoke:{name}:{argument}"));
            CommandEffect::ShowWidget("identity flow started".to_string())
        }),
        // What the real host registers: the built-ins plus every extension
        // command, including each provider's auto-namespaced identity
        // exports (FR-PROV-10).
        slash_commands: vec!["/login".into(), "/antigravity.login".into()],
        models: Vec::new(),
        workspace: PathBuf::from("."),
        keybinding_overrides: Default::default(),
        keybinding_error: None,
        render_regions: None,
        ui_events: None,
        update_notice: None,
        login: Some(Arc::new(move |target: &str| {
            options_seams.record(format!("options:{target}"));
            LoginNext::Picker {
                options: Vec::new(),
            }
        })),
        complete_login: None,
        pick_login: None,
        confirm_login_grant: None,
        confirm_switch: None,
        hooks: UiHooks::default(),
        fullscreen: false,
    };
    Chat::new(options, Arc::new(KeybindingsManager::new()))
}

/// Type a command line and submit it, the way the editor does.
fn run(chat: &mut Chat, line: &str) {
    for character in line.chars() {
        chat.handle_key(&character.to_string());
    }
    chat.handle_key("\r");
}

// Verifies: gh #25 - `/<provider>.login` is the provider's identity `login`
// export, so it reaches the host's command dispatch (which runs it) and
// never the preset-picker seam. This is the row that was red.
#[test]
fn namespaced_login_reaches_the_host_command_dispatch() {
    let seams = Seams::default();
    let mut chat = chat(&seams);

    run(&mut chat, "/antigravity.login");

    assert_eq!(
        seams.invocations(),
        vec!["antigravity.login:".to_string()],
        "the host's dispatch owns the identity export"
    );
    assert!(
        seams.options_args().is_empty(),
        "the options picker never sees a namespaced login: {:?}",
        seams.options_args()
    );
    assert_eq!(
        chat.world.notice.as_deref(),
        Some("identity flow started"),
        "the identity handler's answer is what the user sees"
    );
}

// Verifies: gh #25 (unchanged row) - the bare `/login` still belongs to the
// options seam, with no target.
#[test]
fn bare_login_still_reaches_the_options_seam() {
    let seams = Seams::default();
    let mut chat = chat(&seams);

    run(&mut chat, "/login");

    assert_eq!(seams.options_args(), vec!["".to_string()]);
    assert!(seams.invocations().is_empty(), "{:?}", seams.invocations());
}

// Verifies: gh #25 (unchanged row) - `/login <provider>` still hands the
// provider name to the options seam, where the host's resolver chooses
// between that provider's picker and its identity flow.
#[test]
fn login_scoped_to_a_provider_reaches_the_options_seam() {
    let seams = Seams::default();
    let mut chat = chat(&seams);

    run(&mut chat, "/login antigravity");

    assert_eq!(seams.options_args(), vec!["antigravity".to_string()]);
    assert!(seams.invocations().is_empty(), "{:?}", seams.invocations());
}

// Verifies: gh #25 (unchanged row) - `/login <option-id>` still reaches the
// options seam, where the scripted single-option journey resolves it
// (flows.md: "The login flow").
#[test]
fn login_named_for_an_option_id_reaches_the_options_seam() {
    let seams = Seams::default();
    let mut chat = chat(&seams);

    run(&mut chat, "/login opencode-go");

    assert_eq!(seams.options_args(), vec!["opencode-go".to_string()]);
    assert!(seams.invocations().is_empty(), "{:?}", seams.invocations());
}

// Verifies: gh #25 - a namespaced login for a provider nobody registered is
// an unknown command, not a trip through the picker seam. Red before the
// fix: every `*.login` was intercepted.
#[test]
fn an_unknown_namespaced_login_reports_an_unknown_command() {
    let seams = Seams::default();
    let mut chat = chat(&seams);

    run(&mut chat, "/bogus.login");

    assert_eq!(
        chat.world.notice.as_deref(),
        Some("unknown command /bogus.login")
    );
    assert!(
        seams.options_args().is_empty(),
        "an unknown name never reaches the picker: {:?}",
        seams.options_args()
    );
}
