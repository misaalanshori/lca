//! The ui-world adapter: the inputs the CLI hands the interface, the
//! modal state (permission, the `/login` picker/secret/grant, an extension
//! modal, the side panel), and the effect vocabulary the `ui` world speaks.
//!
//! Editing, the transcript, and key routing live in [`crate::chat`] on the
//! engine's widgets (ADR-0037: one editor, one key vocabulary). This module
//! is single-purpose: it holds no buffer and no rendering.

use std::fmt::Write as _;
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
    /// The id handed back to `login-submit`: the extension's own
    /// preset id, whatever it declares (gh #188).
    pub id: String,
    /// Display line, e.g. "OpenRouter".
    pub label: String,
    /// Right-hand hint, e.g. "openrouter.ai".
    pub hint: String,
}

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
    /// Offer to switch this session to the provider just signed in to
    /// (gh #177); the answer goes to the [`SwitchConfirm`] seam. The
    /// surviving half of the old stranded message: instead of reporting
    /// that the session still uses the old provider, the flow asks.
    ConfirmSwitch {
        /// The provider just signed in to.
        provider: String,
        /// The question to show, e.g. "switch this session to `codex`?".
        prompt: String,
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
    /// A background step is running (R4): the interface shows a
    /// cancellable waiting state and polls [`LoginPoll`] each frame, so a
    /// slow OAuth callback can never freeze the app. `label` names the
    /// step and, once known, carries the auth URL.
    Waiting {
        /// The line the waiting modal shows.
        label: String,
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

/// Switch this session to a freshly signed-in provider; returns the
/// message to show.
pub type SwitchConfirm = Arc<dyn Fn(&str) -> String + Send + Sync>;

/// Poll a background login/identity step (R4): `Some` applies the next
/// [`LoginNext`], `None` keeps the waiting state.
pub type LoginPoll = Arc<dyn Fn() -> Option<LoginNext> + Send + Sync>;

/// Cancel the background login/identity step (R4).
pub type LoginCancel = Arc<dyn Fn() + Send + Sync>;

/// What a background `/compact` is doing. The command's summarization
/// call runs on its own thread (it is a model round-trip, and freezing
/// the interface for it is how pi ended up with a dedicated compaction
/// indicator), and the loop polls this every tick - the same shape as
/// [`LoginPoll`], including the handoff rule: a hook **consumes** its
/// `Done` on read (like `poll_login`'s `take()`), so a finished
/// compaction is reported once rather than on every tick.
pub type CompactPoll = Arc<dyn Fn() -> CompactState + Send + Sync>;

/// The background compaction's state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum CompactState {
    /// Nothing in flight.
    #[default]
    Idle,
    /// Summarizing now.
    Running,
    /// Finished - or failed; the string is the notice to show either way.
    Done(String),
}

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

/// A yes/no confirm for switching to a freshly signed-in provider.
pub struct SwitchPrompt {
    /// The provider just signed in to.
    pub provider: String,
    /// The question naming it.
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
            c if (c as u32) < 0x20 || c == '\x7f' => {
                let _ = write!(out, "\\x{:02x}", c as u32);
            }
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
            c if (c as u32) < 0x20 || c == '\x7f' => {
                let _ = write!(out, "\\x{:02x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out
}

/// Strip Windows' verbatim (`\\?\`) path prefix for **display only**.
/// The canonical form is load-bearing: the `fs` scope-escape check and
/// process spawn both compare against it, so this must never be used
/// there. `\\?\UNC\server\share` keeps its meaning as `\\server\share`.
/// Off Windows (or for an already-plain path) the string is unchanged.
pub fn display_path(path: &str) -> String {
    if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{rest}");
    }
    if let Some(rest) = path.strip_prefix(r"\\?\") {
        return rest.to_string();
    }
    path.to_string()
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

/// A streamed `!`/`!!` shell event (R4).
#[derive(Debug, Clone)]
pub enum ShellEvent {
    /// A chunk of combined output.
    Chunk(String),
    /// The command finished; the shell-convention exit code (`None` when
    /// signalled).
    Done(Option<i32>),
}

/// Cancels a running `!`/`!!` command (Escape, R4).
pub type ShellHandle = Arc<dyn Fn() + Send + Sync>;

/// Writes text to the system clipboard, returning `true` only when the
/// write was verified (R6). `None` falls back to OSC 52, which reports
/// unverified.
pub type ClipboardWriter = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// Opens a URL from an OSC-8 link click (R6/S7). `Err` names the reason
/// (no launcher found, the launcher refused), which the notice shows.
pub type OpenUrl = Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync>;

/// One row in the grants view (S8).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GrantEntry {
    /// `true` for the install-consent group (extension enablement and the
    /// project's approved proposal set), `false` for the ad hoc group.
    pub install_consent: bool,
    /// The extension or subject the grant belongs to.
    pub subject: String,
    /// The granted pattern, or the enablement state.
    pub detail: String,
    /// Whether the grants view can revoke it in place (the store's own write
    /// path); the install-consent group names its manual path instead.
    pub revocable: bool,
}

/// Lists the project's grants for the view (S8).
pub type GrantList = Arc<dyn Fn() -> Vec<GrantEntry> + Send + Sync>;

/// Revokes one grant, returning the notice to show (S8).
pub type GrantRevoke = Arc<dyn Fn(&GrantEntry) -> String + Send + Sync>;

/// Starts a `!`/`!!` shell command, streaming output through the sender and
/// returning a cancel handle (R4). The `bool` is `true` for `!!`.
pub type ShellRunner =
    Arc<dyn Fn(&str, bool, std::sync::mpsc::SyncSender<ShellEvent>) -> ShellHandle + Send + Sync>;
/// Opens the external editor on the prompt text.
pub type ExternalEditor = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;
/// Persists a runtime screen-mode change.
pub type ScreenModePersist = Arc<dyn Fn(bool) + Send + Sync>;
/// Persists a runtime setting choice (`ui.theme`, `thinking`) to the config
/// file (E2); `None` removes the key, and is how `unset` is written.
pub type SettingPersist = Arc<dyn Fn(&str, Option<String>) + Send + Sync>;
/// One `/model` picker row: `(raw id, display label)`.
///
/// The label may carry pi's `model (provider)` decoration (issue #3); the
/// id is what resolution, session metadata, and provider calls use and is
/// never decorated (G2: labels decorate display only, both directions
/// tested).
pub type ModelRow = (String, String);

/// The models the `/model` picker should offer *now*. A hook rather than the
/// startup snapshot, so a login's model discovery (which can only succeed
/// after the endpoint's ad-hoc grant) reaches the picker without a restart.
pub type ModelList = Arc<dyn Fn() -> Vec<ModelRow> + Send + Sync>;

/// The user's trust choice from `/trust` or the startup prompt (ADR-0039).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustChoice {
    /// Remember this decision for the project.
    Persist(bool),
    /// Apply it only for this session.
    Session(bool),
}

/// Cycles the session's model one step (`"forward"` / `"backward"`),
/// returning the notice to show (gh #8, pi's `cycleForward`/`cycleBackward`).
/// The host owns the switch, so a cycle is a `/model` switch in every
/// respect: same cells, same footer, same `model-change` record.
pub type ModelCycle = Arc<dyn Fn(bool) -> String + Send + Sync>;

/// Saves the picker's highlighted model as the default for new sessions
/// (gh #8, pi's `app.models.save`), returning the notice to show. The
/// host owns the write: the config file is its file, and only it can say
/// whether the write landed.
pub type ModelSave = Arc<dyn Fn(&str) -> String + Send + Sync>;

/// Sets the session's thinking level (gh #8 phase 4), returning the
/// notice to show. The host owns it: only it knows the current model's
/// allowed levels, so it clamps, writes the cell, persists the effective
/// value, and says what actually landed.
pub type ThinkingSetter = Arc<dyn Fn(Option<&str>) -> String + Send + Sync>;

/// One row of the `/settings` selector (gh #30): the key, its current
/// value, the layer that value won on (FR-CFG-2's column, kept from the
/// dump), and the values Enter cycles through - empty when the key opens
/// a sub-picker instead.
#[derive(Debug, Clone)]
pub struct SettingRow {
    /// The dotted configuration key.
    pub key: String,
    /// The current value: the live one where the session overrides the file.
    pub value: String,
    /// Where it won: `flag`, `environment`, `project file`, `user file`,
    /// `default`, or `ui.json` for the one setting that lives there.
    pub source: String,
    /// The values left/right and Enter cycle through.
    pub values: Vec<String>,
}

/// The rows `/settings` offers, re-read after every edit so value and
/// source move with it. `None` keeps the host's read-only dump.
pub type SettingsRows = Arc<dyn Fn() -> Vec<SettingRow> + Send + Sync>;

/// Applies a trust choice, returning the notice to show.
pub type TrustApply = Arc<dyn Fn(TrustChoice) -> String + Send + Sync>;
/// Whether the project still needs a trust decision (opens the modal at
/// startup, Pi's `hasTrustRequiringProjectResources`).
pub type TrustNeeded = Arc<dyn Fn() -> bool + Send + Sync>;
/// Returns the session's branch tree as `(id, label)` entries.
pub type SessionTree = Arc<dyn Fn() -> Vec<(String, String)> + Send + Sync>;
/// Forks at the nth user message, returning the new branch's id.
pub type ForkAt = Arc<dyn Fn(usize) -> String + Send + Sync>;
/// Lists the project's sessions, newest first (`/resume`, R2).
pub type SessionList = Arc<dyn Fn() -> Vec<crate::resume::SessionEntry> + Send + Sync>;
/// Switches the live session to `id` and returns its records (R3); `None`
/// when the session cannot be opened. Records rather than lines: the
/// transcript replays them with the live rendering (FR-UI-7), not a
/// plain-text dump.
pub type SwitchSession = Arc<dyn Fn(&str) -> Option<Vec<lca_protocol::Record>> + Send + Sync>;
/// Resolves a user attachment hash to `(media type, bytes)` for the
/// replayed transcript; `None` when the file is gone.
pub type LoadAttachment = Arc<dyn Fn(&str) -> Option<(String, Vec<u8>)> + Send + Sync>;

/// Optional host hooks the interface calls (P6): an external editor, the
/// `!`/`!!` shell path, and screen-mode persistence. Default: all absent,
/// so a host that wires none still gets the in-app behavior.
#[derive(Default)]
pub struct UiHooks {
    /// Run a shell command for `!`/`!!` mode; the `bool` is `true` for
    /// `!!` (excluded from the model's context). Returns the combined
    /// output to show in the transcript.
    pub run_shell: Option<ShellRunner>,
    /// Open `$EDITOR`/`$VISUAL` on the prompt text; `None` aborts. The
    /// terminal is restored around the call.
    pub external_editor: Option<ExternalEditor>,
    /// Persist a runtime screen-mode change (fullscreen = `true`).
    pub persist_screen_mode: Option<ScreenModePersist>,
    /// Persist a runtime setting choice to the config file (E2):
    /// `/theme` writes `ui.theme`, `/thinking` writes `thinking`.
    pub persist_setting: Option<SettingPersist>,
    /// The live model list for `/model` (falls back to the startup
    /// `UiOptions::models` when absent).
    pub models: Option<ModelList>,
    /// One step of the model cycle for the cycle keys (`true` = forward).
    pub cycle_model: Option<ModelCycle>,
    /// `Ctrl+S` in the model picker: persist the highlighted model as the
    /// default (`model` in the user config), returning the notice.
    pub save_default_model: Option<ModelSave>,
    /// `/thinking`'s Enter: clamp the chosen level to the current model's
    /// set, store it, and return the notice.
    pub set_thinking: Option<ThinkingSetter>,
    /// The `/settings` selector's rows (gh #30); absent keeps the
    /// read-only `settings` command dump.
    pub settings_rows: Option<SettingsRows>,
    /// Applies a `/trust` choice.
    pub trust_apply: Option<TrustApply>,
    /// Whether the project needs a trust decision at startup.
    pub trust_needed: Option<TrustNeeded>,
    /// The session's branch tree: `(session id, display label)` entries, the
    /// current branch included (FR-UI-16).
    pub session_tree: Option<SessionTree>,
    /// Fork at the nth user message (0-based), returning the new branch's id
    /// (FR-UI-16).
    pub fork_at: Option<ForkAt>,
    /// List the project's sessions for `/resume` (R2).
    pub session_list: Option<SessionList>,
    /// Switch the live session in place, returning its records (R3).
    pub switch_session: Option<SwitchSession>,
    /// Resolve an attachment hash to media type and bytes, so a replayed
    /// message shows its image (FR-UI-13).
    pub load_attachment: Option<LoadAttachment>,
    /// Write a selection to the system clipboard, verified (R6).
    pub copy_to_clipboard: Option<ClipboardWriter>,
    /// Open a URL a click landed on (R6).
    pub open_url: Option<OpenUrl>,
    /// List the project's grants (S8).
    pub grants: Option<GrantList>,
    /// Revoke one grant (S8).
    pub revoke_grant: Option<GrantRevoke>,
    /// Poll a background login/identity step (R4); `Some` applies the next
    /// step, `None` keeps waiting. The loop calls it every tick.
    pub poll_login: Option<LoginPoll>,
    /// Poll a background `/compact`: `Running` raises the working state,
    /// `Done` posts the summary (or the refusal) as a notice and rests it.
    pub poll_compact: Option<CompactPoll>,
    /// Cancel the background login/identity step (R4), called on Escape
    /// while the waiting modal is open.
    pub cancel_login: Option<LoginCancel>,
    /// Pre-parse markdown transforms in registration order (gh #12),
    /// collected by the host from native extensions. Empty by default -
    /// no consumer, no rewriting.
    pub markdown_transformers: Vec<lca_tui::widgets::markdown::MarkdownTransformer>,
}

/// Static inputs for the interface.
pub struct UiOptions {
    /// `provider/model` for the status line - a cell, because the
    /// status line shows the session's model and `/model` rewrites it.
    pub model_label: std::sync::Arc<std::sync::Mutex<String>>,
    /// The active model's context window in tokens (0 = unknown), a cell so
    /// `/model` updates it and the footer refreshes every frame (FR-UI-20).
    pub context_window: std::sync::Arc<std::sync::Mutex<u64>>,
    /// The session's thinking level (`thinking`, R1): `None` is the
    /// provider's own default. A cell, because `/thinking` rewrites it and
    /// the footer shows it every frame.
    pub thinking: std::sync::Arc<std::sync::Mutex<Option<String>>>,
    /// The configured theme setting (`ui.theme`, S5): a built-in name, a
    /// custom theme's name, or `auto` for the detected terminal scheme.
    pub theme: String,
    /// Where custom theme files live (`<config>/themes`).
    pub theme_dir: PathBuf,
    /// The `/theme` picker's names (built-ins plus custom files).
    pub themes: Vec<String>,
    /// Conversation lines already resolved for display: warnings and
    /// status notices that surround the transcript (resume).
    pub initial_lines: Vec<String>,
    /// The resumed session's records, replayed with the live rendering
    /// (FR-UI-7) - user band, markdown, tool cards, image labels.
    pub initial_records: Vec<lca_protocol::Record>,
    /// Lines drawn after the transcript (the missing-provider report).
    pub initial_tail_lines: Vec<String>,
    /// Positional CLI messages (#109): the first is submitted when the
    /// interface opens, the rest queue as follow-ups in order.
    pub initial_messages: Vec<String>,
    /// Plain-text rendering (FR-UI-5).
    pub plain: bool,
    /// Permission prompts are auto-approved this session (ADR-0042); the
    /// footer says so every frame.
    pub yolo: bool,
    /// How much of a thinking run the transcript shows by default (R6).
    pub thinking_visibility: crate::transcript::ThinkingVisibility,
    /// How fenced code blocks are framed (gh #32): `full` keeps the
    /// shipped frame, `horizontal` drops the side pipes so a terminal
    /// copy is clean, `none` draws nothing.
    pub codeblock_border: lca_tui::widgets::markdown::CodeBlockBorder,
    /// Invoke a registered slash command (the registry supplies the
    /// table; `/stats` fills its built-in slot through an extension,
    /// ADR-0019). Arguments are the bare typed name and its argument
    /// text.
    pub invoke_command: CommandInvoker,
    /// Slash commands offered by completion.
    pub slash_commands: Vec<String>,
    /// Model rows (`(id, label)`) offered by `/model <Tab>` argument
    /// completion: the menu shows the label, the insert is the id.
    pub models: Vec<crate::state::ModelRow>,
    /// The session's permission-prompt sender, published by `run` so the
    /// host's own consent can ask *outside* a turn too (gh #31 review: the
    /// picker's endpoint consent). The interface owns the channel for the
    /// whole session; `None` before `run` starts.
    pub prompt_slot:
        std::sync::Arc<std::sync::Mutex<Option<std::sync::mpsc::SyncSender<PromptRequest>>>>,
    /// Rows the host has ready for the `/model` picker after its consent
    /// finished (that flow's second step): a non-empty list opens the
    /// picker once; the host clears its cell as it hands them over.
    pub pending_models: Option<ModelList>,
    /// Workspace root for path completion.
    pub workspace: PathBuf,
    /// User key overrides from `keybindings.toml` (gh #66): action →
    /// keys, loaded by the host before `run`. Empty means defaults.
    pub keybinding_overrides: std::collections::BTreeMap<String, Vec<String>>,
    /// A `keybindings.toml` load failure, if any (gh #66): `run` shows
    /// it loud and keeps defaults. `None` means the file parsed (or is
    /// absent).
    pub keybinding_error: Option<String>,
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
    /// Switches the session to a freshly signed-in provider.
    pub confirm_switch: Option<SwitchConfirm>,
    /// Host hooks (P6).
    pub hooks: UiHooks,
    /// Whether the session starts in the fullscreen (alt-screen) renderer.
    pub fullscreen: bool,
}

/// The permission modal: what is being asked, and how to answer.
pub struct PermissionModal {
    /// The exact command or path (FR-UI-4).
    pub action: String,
    /// The worker waiting for the decision, when live.
    pub respond: Option<SyncSender<lca_permissions::Decision>>,
    /// When the auto-approve countdown fires (FR-UI-18), if any. The
    /// countdown is visible and any key cancels it; it never fires
    /// silently.
    pub deadline: Option<std::time::Instant>,
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
    /// The open provider-switch confirm, if any (`/login`, gh #177).
    pub switch_confirm: Option<SwitchPrompt>,
    /// A background login/identity step is running (R4); the label names
    /// it (and carries the auth URL once the provider has asked for one).
    pub login_waiting: Option<String>,
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
            switch_confirm: None,
            login_waiting: None,
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
            deadline: None,
        });
    }

    /// Whether any modal owns the screen this frame.
    pub fn modal_active(&self) -> bool {
        self.picker.is_some()
            || self.grant.is_some()
            || self.secret.is_some()
            || self.login_waiting.is_some()
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
    /// Open the external editor on the prompt text (FR-UI-15).
    ExternalEditor,
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
    // One step owns the screen at a time: entering one clears the others.
    // `Waiting` is the exception that must not clear a manual-callback
    // secret field, so it only sets the waiting label.
    match next {
        LoginNext::Message(text) => {
            state.login_waiting = None;
            state.notice = Some(sanitize_block(&text));
        }
        LoginNext::Secret {
            provider,
            label,
            masked,
        } => {
            state.login_waiting = None;
            state.secret = Some(SecretPrompt {
                provider,
                label,
                input: String::new(),
                masked,
            });
        }
        LoginNext::Picker { options } => {
            state.login_waiting = None;
            if options.is_empty() {
                state.notice = Some("nothing to sign in to".to_string());
            } else {
                state.picker = Some(PickerPrompt {
                    options,
                    selected: 0,
                });
            }
        }
        LoginNext::ConfirmSwitch { provider, prompt } => {
            state.login_waiting = None;
            state.switch_confirm = Some(SwitchPrompt { provider, prompt });
        }
        LoginNext::Grant {
            provider,
            host,
            prompt,
        } => {
            state.login_waiting = None;
            state.grant = Some(GrantPrompt {
                provider,
                host,
                prompt,
            });
        }
        LoginNext::Waiting { label } => {
            state.login_waiting = Some(sanitize_block(&label));
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
    /// How the submitted prompt was queued (`None` for an ordinary
    /// submit): the ADR-0038 marker the turn's user record carries.
    pub queue: Option<lca_protocol::SubmitMode>,
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
            prompt_slot: Default::default(),
            pending_models: None,
            model_label: Arc::new(std::sync::Mutex::new("p/m".into())),
            context_window: Arc::new(std::sync::Mutex::new(0)),
            thinking: Arc::new(std::sync::Mutex::new(None)),
            theme: "auto".to_string(),
            theme_dir: std::path::PathBuf::new(),
            themes: crate::theme::THEMES.iter().map(|s| s.to_string()).collect(),
            initial_lines: Vec::new(),
            initial_records: Vec::new(),
            initial_tail_lines: Vec::new(),
            initial_messages: Vec::new(),
            yolo: false,
            thinking_visibility: Default::default(),
            codeblock_border: Default::default(),
            plain: true,
            invoke_command: Arc::new(|_, _| CommandEffect::None),
            slash_commands: Vec::new(),
            models: Vec::new(),
            workspace: PathBuf::from("."),
            keybinding_overrides: Default::default(),
            keybinding_error: None,
            render_regions: None,
            ui_events: None,
            update_notice: None,
            login: None,
            complete_login: None,
            pick_login: None,
            confirm_login_grant: None,
            confirm_switch: None,
            hooks: UiHooks::default(),
            fullscreen: false,
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
