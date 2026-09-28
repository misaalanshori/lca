//! The ui-world adapter: the inputs the CLI hands the interface, the
//! modal state (permission, the `/login` picker/secret/grant, an extension
//! modal, the side panel), and the effect vocabulary the `ui` world speaks.
//!
//! Editing, the transcript, and key routing live in [`crate::chat`] on the
//! engine's widgets (ADR-0037: one editor, one key vocabulary). This module
//! is single-purpose: it holds no buffer and no rendering.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::SyncSender;

use lca_protocol::CommandEffect;
use lca_protocol::{TurnEvent, TurnOutcome};

/// A one-line description of the running turn (NFR-28: state also has a
/// text cue, never color alone).
#[derive(Debug, Clone)]
pub struct TurnStatusLine {
    /// The text shown on the status line.
    pub text: String,
}

/// How the input editor invokes a registered slash command.
pub type CommandInvoker = Arc<dyn Fn(&str, &str) -> CommandEffect + Send + Sync>;

/// One entry in the `/login` list picker. Display-ready: the host renders
/// it and never learns what the id means (ADR-0033 - provider-shaped data
/// stays in the extension).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickerOption {
    /// The provider this choice belongs to; the picker mixes several.
    pub provider: String,
    /// The id handed back to `login-submit`. The host keeps the universal
    /// "custom" entry's id in [`CUSTOM_OPTION`].
    pub id: String,
    /// Display line, e.g. "OpenRouter".
    pub label: String,
    /// Right-hand hint, e.g. "openrouter.ai".
    pub hint: String,
}

/// The host-owned picker entry: base URL + key + model, no preset.
pub const CUSTOM_OPTION: &str = "__custom__";

/// What `/login <provider>` should do next, decided by the CLI.
#[derive(Debug)]
pub enum LoginNext {
    /// The CLI already handled it (OAuth, already signed in, refusal); show
    /// this text.
    Message(String),
    /// Ask the user for one line through a modal; the answer goes to the
    /// [`LoginComplete`] seam. Masked for a secret, plain for a base URL or
    /// a model id.
    Secret {
        /// The provider the value belongs to.
        provider: String,
        /// The prompt to show, e.g. "API key for openai-compatible".
        label: String,
        /// Render as asterisks (a secret) or as typed (a URL, a model id).
        masked: bool,
    },
    /// Offer the list picker (ADR-0033); the choice goes to the
    /// [`LoginPick`] seam.
    Picker {
        /// The choices to show, in order.
        options: Vec<PickerOption>,
    },
    /// Offer an ad hoc `net` grant for the endpoint host the login flow
    /// just named (FR-PERM-16); the answer goes to the [`LoginConfirm`]
    /// seam.
    Grant {
        /// The provider whose endpoint this is.
        provider: String,
        /// The exact host being added.
        host: String,
        /// The consent text naming the host.
        prompt: String,
    },
}

/// The host's login seam: `/login` calls this to choose between a message,
/// a list picker, and a masked secret prompt. The CLI owns provider
/// knowledge; the interface owns the modal (`docs/deferred_workplan.md` B2).
pub type LoginRequest = Arc<dyn Fn(&str) -> LoginNext + Send + Sync>;

/// The user picked a picker entry; returns the next step (usually the
/// first field to collect). The CLI remembers the choice for
/// [`LoginComplete`].
pub type LoginPick = Arc<dyn Fn(&str, &str) -> LoginNext + Send + Sync>;

/// Hand the CLI one typed line (a secret, a base URL, a model id) for the
/// login flow it is running; returns what to do next - including another
/// [`LoginNext::Secret`], which is how a multi-field login collects its
/// remaining fields. A secret is never echoed into scrollback or history.
pub type LoginComplete = Arc<dyn Fn(&str, &str) -> LoginNext + Send + Sync>;

/// Persist an ad hoc `net` grant the user approved at login; returns the
/// message to show.
pub type LoginConfirm = Arc<dyn Fn(&str, &str) -> String + Send + Sync>;

/// A single-line prompt (the `/login` flow).
pub struct SecretPrompt {
    /// The provider the value belongs to.
    pub provider: String,
    /// What the user is being asked for.
    pub label: String,
    /// The characters typed so far - rendered masked when [`Self::masked`],
    /// never logged.
    pub input: String,
    /// Asterisks instead of the typed characters.
    pub masked: bool,
}

/// The open list picker (the `/login` flow).
pub struct PickerPrompt {
    /// The choices, in order.
    pub options: Vec<PickerOption>,
    /// Which row the cursor is on.
    pub selected: usize,
}

/// A yes/no confirm for an ad hoc `net` grant the login flow offers.
pub struct GrantPrompt {
    /// The provider whose endpoint this is.
    pub provider: String,
    /// The exact host being added.
    pub host: String,
    /// The consent text naming the host.
    pub prompt: String,
}

/// Which extensions draw in a region: the CLI's view over the registry
/// (ADR-0003's pull model - the host asks, the extension answers).
pub type RegionRenderer =
    Arc<dyn Fn(&str) -> Vec<(String, lca_protocol::WidgetTree)> + Send + Sync>;

/// Deliver one user interaction to the extensions registered for that
/// region; the first `Some` answers and the host applies its effect.
/// Effects only ever originate here - from real user input - which is
/// what makes FR-UI-6 ("no modal without the user") enforceable.
pub type RegionInteractor = Arc<
    dyn Fn(&str, &lca_protocol::UiInput) -> Option<(String, lca_protocol::UiEffect)> + Send + Sync,
>;

/// Escape sequences and other control characters become visible text
/// before anything is drawn: spans carry data, never control codes
/// (FR-UI-2, ADR-0003). Tabs and newlines collapse to spaces because
/// the host owns layout.
pub fn sanitize_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '\n' | '\t' | '\r' => out.push(' '),
            c if (c as u32) < 0x20 || c == '\x7f' => out.push_str(&format!("\\x{:02x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// Like [`sanitize_text`], but keeps newlines: for block output whose
/// layout the host decided (a command's multi-line result, a notice). Tabs
/// and carriage returns become spaces; other control characters become
/// visible text, so an escape sequence still cannot reach the terminal.
pub fn sanitize_block(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '\n' => out.push('\n'),
            '\t' | '\r' => out.push(' '),
            c if (c as u32) < 0x20 || c == '\x7f' => out.push_str(&format!("\\x{:02x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// One node's lines, arena-style: node0 is the root and children are
/// indices. Every text node passes through [`sanitize_text`] - the one
/// choke point for FR-UI-2.
///
/// A node is rendered at most once per call. The arena is supplied by an
/// untrusted extension, so a child index that points at an ancestor (or
/// at itself) must not recurse forever or expand exponentially; the
/// `visited` set is the guard the widget-shaped sibling attack needs.
pub fn widget_lines(nodes: &[lca_protocol::Widget]) -> Vec<String> {
    fn walk(
        nodes: &[lca_protocol::Widget],
        index: usize,
        out: &mut Vec<String>,
        visited: &mut [bool],
    ) {
        use lca_protocol::Widget;
        let Some(node) = nodes.get(index) else { return };
        if visited.get(index) == Some(&true) {
            return;
        }
        if let Some(seen) = visited.get_mut(index) {
            *seen = true;
        }
        match node {
            Widget::Text { content, .. } => out.push(sanitize_text(content)),
            Widget::Image { media_type, bytes } => {
                out.push(format!("[image {media_type}, {} bytes]", bytes.len()))
            }
            Widget::Boxed { title, child } => {
                if let Some(title) = title {
                    out.push(format!("[{title}]"));
                }
                walk(nodes, *child as usize, out, visited);
            }
            Widget::Row(children) => {
                // Side by side, first line of each (v1 layout; ponytail:
                // a real row shaper when an extension needs wrapping).
                let parts: Vec<String> = children
                    .iter()
                    .filter_map(|child| {
                        let mut lines = Vec::new();
                        walk(nodes, *child as usize, &mut lines, visited);
                        lines.into_iter().next()
                    })
                    .collect();
                out.push(parts.join(" | "));
            }
            Widget::Column(children) => {
                for child in children {
                    walk(nodes, *child as usize, out, visited);
                }
            }
            Widget::Spinner { frames } => {
                let ticks = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|since| since.subsec_millis())
                    .unwrap_or(0);
                let count = frames.chars().count();
                if count > 0 {
                    let pick = (ticks / 80) as usize % count;
                    out.push(frames.chars().nth(pick).unwrap_or(' ').to_string());
                } else {
                    out.push(" ".to_string());
                }
            }
            Widget::Progress { label, fill } => {
                let fill = (*fill).clamp(0.0, 1.0);
                let width = 20;
                let done = (fill * width as f32).round() as usize;
                out.push(format!(
                    "{label} [{}{}] {:>3}%",
                    "#".repeat(done),
                    "-".repeat(width - done),
                    (fill * 100.0).round() as u32
                ));
            }
            Widget::KeyValue(pairs) => {
                for (key, value) in pairs {
                    out.push(format!("{key}: {}", sanitize_text(value)));
                }
            }
            Widget::Vendor(kind) => out.push(format!("[vendor {kind}]")),
        }
    }
    let mut out = Vec::new();
    if !nodes.is_empty() {
        let mut visited = vec![false; nodes.len()];
        walk(nodes, 0, &mut out, &mut visited);
    }
    out
}

/// Static inputs for the interface.
pub struct UiOptions {
    /// `provider/model` for the status line - a cell, because the
    /// status line shows the session's model and `/model` rewrites it.
    pub model_label: std::sync::Arc<std::sync::Mutex<String>>,
    /// Conversation lines already resolved for display (resume).
    pub initial_lines: Vec<String>,
    /// Plain-text rendering (FR-UI-5).
    pub plain: bool,
    /// Invoke a registered slash command (the registry supplies the
    /// table; `/stats` fills its built-in slot through an extension,
    /// ADR-0019). Arguments are the bare typed name and its argument
    /// text.
    pub invoke_command: CommandInvoker,
    /// Slash commands offered by completion.
    pub slash_commands: Vec<String>,
    /// Model ids offered by `/model <Tab>` argument completion.
    pub models: Vec<String>,
    /// Workspace root for path completion.
    pub workspace: PathBuf,
    /// Extension trees per region (`None`: no ui-capable extension is
    /// registered, which is the default).
    pub render_regions: Option<RegionRenderer>,
    /// User interactions routed to extensions (FR-UI-6's only source).
    pub ui_events: Option<RegionInteractor>,
    /// The background update check's finding, set once a check finds a
    /// newer release (FR-CFG-6; the status line reads it every frame).
    pub update_notice: Option<std::sync::Arc<std::sync::OnceLock<String>>>,
    /// The host's `/login` seam; `None` falls back to the registry's own
    /// identity command.
    pub login: Option<LoginRequest>,
    /// Stores a secret the user typed for `/login`.
    pub complete_login: Option<LoginComplete>,
    /// The user chose a `/login` picker entry.
    pub pick_login: Option<LoginPick>,
    /// Persists an ad hoc `net` grant the user approved at login.
    pub confirm_login_grant: Option<LoginConfirm>,
}

/// The permission modal: what is being asked, and how to answer.
pub struct PermissionModal {
    /// The exact command or path (FR-UI-4).
    pub action: String,
    /// The worker waiting for the decision, when live.
    pub respond: Option<SyncSender<lca_permissions::Decision>>,
}

/// The ui-world state: the options, the open modals, and the small flags
/// the interface tracks across keys. No buffer, no transcript, no editing.
pub struct UiState {
    /// Static inputs.
    pub options: UiOptions,
    /// A transient notice (command results, unknown commands).
    pub notice: Option<String>,
    /// The open permission modal, if any.
    pub permission: Option<PermissionModal>,
    /// The open masked secret prompt, if any (`/login`).
    pub secret: Option<SecretPrompt>,
    /// The open list picker, if any (`/login`).
    pub picker: Option<PickerPrompt>,
    /// The open ad hoc-grant confirm, if any (`/login`).
    pub grant: Option<GrantPrompt>,
    /// Ctrl+C seen once on an idle prompt.
    pub ctrl_c_armed: bool,
    /// The extension side panel is open.
    pub panel_open: bool,
    /// An extension modal is open (one at a time, user-dismissible:
    /// capability catalog `ui`).
    pub modal_open: bool,
    /// Terminal size, tracked across resizes (FR-UI-3).
    pub size: (u16, u16),
}

impl UiState {
    /// Build the initial ui-world state.
    pub fn new(options: UiOptions) -> UiState {
        let workspace = if options.workspace.as_os_str().is_empty() {
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
        } else {
            options.workspace.clone()
        };
        let mut options = options;
        options.workspace = workspace;
        UiState {
            options,
            notice: None,
            permission: None,
            secret: None,
            picker: None,
            grant: None,
            ctrl_c_armed: false,
            panel_open: false,
            modal_open: false,
            size: (80, 24),
        }
    }

    /// Record a terminal resize (FR-UI-3: nothing is lost).
    pub fn resize(&mut self, width: u16, height: u16) {
        self.size = (width, height);
    }

    /// Open the permission modal (FR-UI-4).
    pub fn show_permission(&mut self, action: String) {
        self.permission = Some(PermissionModal {
            action,
            respond: None,
        });
    }

    /// Whether any modal owns the screen this frame.
    pub fn modal_active(&self) -> bool {
        self.picker.is_some()
            || self.grant.is_some()
            || self.secret.is_some()
            || self.permission.is_some()
            || self.modal_open
    }
}

/// What a key press asks the loop to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Nothing external; keep editing.
    Continue,
    /// Submit the buffer as a user turn.
    Submit,
    /// Cancel the running turn (FR-CORE-5).
    CancelTurn,
    /// Exit the interface.
    Exit,
}

/// Map a key identifier (`engine::keys::parse_key`'s vocabulary) to the
/// `ui` world's interaction input.
pub fn key_input(key: &str) -> lca_protocol::UiInput {
    match key {
        "enter" => lca_protocol::UiInput::Submit {
            text: String::new(),
        },
        "escape" => lca_protocol::UiInput::Cancel,
        other => lca_protocol::UiInput::Key {
            key: other.to_string(),
        },
    }
}

/// Apply the CLI's next login step: one modal at a time, and a
/// [`LoginNext::Secret`] mid-flow is the next field of a multi-field
/// login, not a refusal.
pub fn apply_login_next(state: &mut UiState, next: LoginNext) {
    match next {
        LoginNext::Message(text) => state.notice = Some(sanitize_block(&text)),
        LoginNext::Secret {
            provider,
            label,
            masked,
        } => {
            state.secret = Some(SecretPrompt {
                provider,
                label,
                input: String::new(),
                masked,
            });
        }
        LoginNext::Picker { options } => {
            if options.is_empty() {
                state.notice = Some("nothing to sign in to".to_string());
            } else {
                state.picker = Some(PickerPrompt {
                    options,
                    selected: 0,
                });
            }
        }
        LoginNext::Grant {
            provider,
            host,
            prompt,
        } => {
            state.grant = Some(GrantPrompt {
                provider,
                host,
                prompt,
            });
        }
    }
}

/// Channels between the UI loop and the worker thread running a turn.
pub struct TurnChannels {
    /// Turn events flow to the UI.
    pub events: SyncSender<TurnEvent>,
    /// Permission requests flow to the UI.
    pub prompt: SyncSender<PromptRequest>,
    /// Messages the interface queues while the turn runs; the worker
    /// drains it at each model-call boundary (ADR-0038).
    pub steer: lca_protocol::SteerQueue,
}

/// A worker asking for permission (FR-UI-4: `action` is the exact command
/// or path).
pub struct PromptRequest {
    /// The exact action display.
    pub action: String,
    /// Where the decision goes.
    pub respond: SyncSender<lca_permissions::Decision>,
}

/// Starts one turn on a worker thread and hands back its join handle; the
/// binary supplies this so the crate stays free of provider wiring. The
/// runner is callable once per submission.
pub type TurnRunner = Box<
    dyn Fn(String, TurnChannels, lca_tools::CancelFlag) -> std::thread::JoinHandle<TurnOutcome>
        + Send
        + Sync,
>;

#[cfg(test)]
mod tests {
    use super::*;
    use lca_protocol::{StopReason, TurnStatus, Usage};

    fn options() -> UiOptions {
        UiOptions {
            model_label: Arc::new(std::sync::Mutex::new("p/m".into())),
            initial_lines: Vec::new(),
            plain: true,
            invoke_command: Arc::new(|_, _| CommandEffect::None),
            slash_commands: Vec::new(),
            models: Vec::new(),
            workspace: PathBuf::from("."),
            render_regions: None,
            ui_events: None,
            update_notice: None,
            login: None,
            complete_login: None,
            pick_login: None,
            confirm_login_grant: None,
        }
    }

    #[test]
    fn sanitize_strips_control_characters() {
        assert_eq!(sanitize_text("a\x1b[31mb"), "a\\x1b[31mb");
        assert_eq!(sanitize_block("a\nb\tc"), "a\nb c");
    }

    #[test]
    fn widget_lines_renders_a_column_and_key_values() {
        use lca_protocol::Widget;
        let nodes = vec![
            Widget::Column(vec![1, 2]),
            Widget::Text {
                content: "hello".into(),
                role: "default".into(),
            },
            Widget::KeyValue(vec![("k".into(), "v".into())]),
        ];
        assert_eq!(widget_lines(&nodes), vec!["hello", "k: v"]);
    }

    #[test]
    fn widget_lines_guards_against_a_self_referential_child() {
        use lca_protocol::Widget;
        let nodes = vec![Widget::Boxed {
            title: None,
            child: 0,
        }];
        assert!(widget_lines(&nodes).is_empty());
    }

    #[test]
    fn login_next_secret_opens_the_secret_prompt() {
        let mut state = UiState::new(options());
        apply_login_next(
            &mut state,
            LoginNext::Secret {
                provider: "p".into(),
                label: "key".into(),
                masked: true,
            },
        );
        assert!(state.secret.is_some());
        assert!(state.modal_active());
    }

    #[test]
    fn key_input_maps_the_vocabulary() {
        assert!(matches!(
            key_input("enter"),
            lca_protocol::UiInput::Submit { .. }
        ));
        assert!(matches!(key_input("escape"), lca_protocol::UiInput::Cancel));
        assert!(matches!(key_input("up"), lca_protocol::UiInput::Key { .. }));
    }

    #[test]
    fn usage_reexports_compile() {
        let _ = Usage::default();
        let _ = TurnStatus::Ok;
        let _ = StopReason::Stop;
    }
}
