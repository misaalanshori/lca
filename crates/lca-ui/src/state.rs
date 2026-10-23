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

use crate::state_persona::{PersonaApply, PersonaCheck, PersonaList};
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

/// One `/mcp` verb (`reconnect`, `enable`, `disable`, `exposure`,
/// `login`, `logout`): the target server plus the notice to show.
pub type McpAction = Arc<dyn Fn(&str, &str) -> String + Send + Sync>;

/// Poll a background `/mcp login`: `Some` posts the sign-in outcome
/// as a notice once, `None` keeps waiting (the login-poll shape).
pub type McpPoll = Arc<dyn Fn() -> Option<String> + Send + Sync>;

pub use crate::mcp_types::{
    McpBeginSignIn, McpCancelSignIn, McpDetail, McpList, McpServerDetail, McpServerRow,
    McpSnapshot, McpSubmitRedirect, McpToolRow,
};

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
    /// Summarizing now, for the reason the separator names (gh #388 gap 2).
    Running(crate::CompactionReason),
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

/// A staged `/bug` bundle waiting for the user to inspect and confirm (gh #411
/// gap 2).
///
/// The inspection is the **gate**: nothing is zipped until this is answered, and
/// cancelling removes the staging directory. Holding the plan here means the
/// screen, the confirm, and the cancel all read one already-built artifact, so
/// showing the user what they are about to share costs no extra I/O - and it
/// cannot drift from what actually gets written.
pub struct BugReview {
    /// What the user is about to share: file list with sizes, the staged path,
    /// and the scrub summary. Rendered by the host from the staged plan, so the
    /// screen costs no I/O and cannot drift from what will be written.
    pub inspection: String,
    /// Where the zip will land, to name it after the write.
    pub destination: String,
    /// A handle the host gave us: `true` writes the bundle and returns the
    /// notice, `false` discards it. The UI owns no zip logic and never sees a
    /// secret - it holds a string and a decision.
    pub resolve: BugResolve,
}

/// The host's answer to an inspection: `(confirmed, )` and back comes the notice
/// to show. `false` must leave nothing on disk (gh #411 design point 4).
pub type BugResolve = Arc<dyn Fn(bool) -> String + Send + Sync>;

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

// The login-step application (and the sign-in-URL capture that goes with it)
// lives in `login_step.rs` after the ceiling split; the public path is unchanged.
pub use crate::login_step::apply_login_next;

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
/// Runs an interactive `!` command with the terminal handed over (gh #407):
/// called after the interface has stopped and before it restarts, so the child
/// inherits the real TTY. The `Option<i32>` is its exit code, `None` when it was
/// killed by a signal.
pub type InteractiveShell = Arc<dyn Fn(&str) -> Option<i32> + Send + Sync>;
/// Persists a runtime screen-mode change.
pub type ScreenModePersist = Arc<dyn Fn(bool) + Send + Sync>;
/// Persists a runtime setting choice (`ui.theme`, `thinking`) to the config
/// file (E2); `None` removes the key, and is how `unset` is written.
pub type SettingPersist = Arc<dyn Fn(&str, Option<String>) + Send + Sync>;
/// Persists a **project-scope** setting choice to `<cwd>/.lca/config.toml`
/// (gh #397): `None` removes the key, which is how the picker's `inherit`
/// state is written. A separate hook from [`SettingPersist`] because it is a
/// different file with different consequences - this one states something
/// about this project only, and its presence is what makes the project
/// trust-requiring.
pub type ProjectSettingPersist =
    Arc<dyn Fn(&str, Option<String>) -> Result<(), String> + Send + Sync>;

/// One `/model` picker row: `(raw id, display label)`.
///
/// The label may carry pi's `model (provider)` decoration (issue #3); the
/// id is what resolution, session metadata, and provider calls use and is
/// never decorated (G2: labels decorate display only, both directions
/// tested).
pub type ModelRow = crate::chat_model_search::ModelEntry;

/// One row of the `/scoped-models` checklist (gh #204): the model id,
/// its display label, its context window (0 when the provider says
/// nothing), and whether it is in the quick-cycle rotation now.
#[derive(Debug, Clone)]
pub struct ScopedModelRow {
    /// The raw model id (what the scope saves).
    pub id: String,
    /// The display label (provider badge included).
    pub label: String,
    /// The context window in tokens (0 = unknown).
    pub context: u32,
    /// In the rotation now.
    pub enabled: bool,
}

/// The full model catalog with current scope flags: the checklist
/// reads every offered model (not the filtered cycle list) so a
/// disabled model can be re-enabled. A hook rather than the startup
/// snapshot, so a login's model discovery shows up.
pub type ScopedModels = Arc<dyn Fn() -> Vec<ScopedModelRow> + Send + Sync>;

/// Apply a checklist save: persist the checked ids to `models.enabled`
/// and update the live rotation. Returns the notice to show.
pub type SaveScopedModels = Arc<dyn Fn(Vec<String>) -> String + Send + Sync>;

/// The prompt templates a `/name` command can expand (gh #58).
/// A hook rather than a snapshot, so a file added mid-session works.
pub type PromptTemplates = Arc<dyn Fn() -> Vec<lca_tools::prompts::PromptTemplate> + Send + Sync>;

/// The double-escape action (gh #132, pi's `doubleEscapeAction`): what
/// Esc Esc with an empty editor does. A hook so a `/settings` cycle
/// applies without a restart; absent means pi's default (`tree`).
pub type DoubleEscapeAction = Arc<dyn Fn() -> String + Send + Sync>;

/// The live display tunables (gh #82, pi's "Terminal and display"
/// inventory): plain values a hook refreshes from the config cell, so
/// a `/settings` cycle applies without a restart. Absent means the
/// product defaults (each mirroring its `lca-config` default).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayTuning {
    /// Autocomplete popup rows, 3-20.
    pub autocomplete_max_visible: u8,
    /// Editor horizontal padding cells, 0-3.
    pub editor_padding_x: u8,
    /// Transcript left margin, 0|1.
    pub output_pad: u8,
    /// Fullscreen scrollbar: `auto`, `always`, or `hidden`.
    pub fullscreen_scrollbar: String,
    /// Copy on text selection in fullscreen.
    pub fullscreen_copy_on_select: bool,
    /// Fullscreen wheel lines: `auto` (three) or 1-100.
    pub fullscreen_wheel_lines: String,
    /// Fullscreen exit output: `transcript` or `resume-hint`.
    pub fullscreen_exit_output: String,
    /// Display inline images.
    pub show_images: bool,
    /// Inline image width cap in cells.
    pub image_width_cells: u16,
    /// OSC 9;4 taskbar progress around turns.
    pub show_progress: bool,
    /// Downscale images before sending.
    pub auto_resize_images: bool,
    /// Code block content indent.
    pub code_block_indent: String,
    /// Mermaid rendering: `off`, `final`, or `streaming`.
    pub mermaid: String,
}

impl Default for DisplayTuning {
    /// Pi-matching defaults (each mirrors its config default).
    fn default() -> Self {
        DisplayTuning {
            autocomplete_max_visible: 5,
            editor_padding_x: 0,
            output_pad: 1,
            fullscreen_scrollbar: "auto".to_string(),
            fullscreen_copy_on_select: true,
            fullscreen_wheel_lines: "auto".to_string(),
            fullscreen_exit_output: "transcript".to_string(),
            show_images: true,
            image_width_cells: 60,
            show_progress: true,
            auto_resize_images: true,
            code_block_indent: "  ".to_string(),
            mermaid: "streaming".to_string(),
        }
    }
}

/// The live display tunables; absent keeps the product defaults.
pub type DisplayTuningHook = Arc<dyn Fn() -> DisplayTuning + Send + Sync>;

/// The current session's id for the fullscreen resume hint (gh #82).
/// A hook so a `/tree` or `/resume` switch is honored live; absent
/// keeps the plain transcript exit.
pub type CurrentSessionId = Arc<dyn Fn() -> String + Send + Sync>;

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
/// The `/thinking` picker's offered levels, resolved for the current
/// model by the host.
pub type ThinkingOffered = Arc<dyn Fn() -> Vec<String> + Send + Sync>;

/// Saves the picker's highlighted thinking level as the default for new sessions
/// (gh #396, pi's `app.thinking.save`), returning the notice to show.
///
/// Mirrors [`ModelSave`] exactly: the host owns the write, because the config file
/// is its file and only it can say whether the write landed. `None` is "unset"
/// (the provider's own default), which is a value the picker can save.
pub type ThinkingSave = Arc<dyn Fn(Option<&str>) -> String + Send + Sync>;
/// A counted cache miss's notice text, if the host has news (gh #42):
/// the loop calls it when a turn ends; `Some` shows once. Absent
/// means silent (misses still aggregate under `/session`).
pub type CacheMissNotice = Arc<dyn Fn() -> Option<String> + Send + Sync>;

/// One row of the `/settings` selector (gh #30): the key, its current
/// value, the layer that value won on (FR-CFG-2's column, kept from the
/// dump), and the values Enter cycles through - empty when the key opens
/// a sub-picker instead.
#[derive(Debug, Clone)]
pub struct SettingRow {
    /// The section the row renders under (gh #174): consecutive rows
    /// sharing it share one `── section ──` divider. Empty renders
    /// unsectioned (the old flat list).
    pub section: String,
    /// The dotted configuration key.
    pub key: String,
    /// The human-readable label the picker paints (gh #235).
    pub label: String,
    /// One-line help for the highlighted row (gh #235).
    pub description: String,
    /// The current value: the live one where the session overrides the file.
    pub value: String,
    /// Where it won: `flag`, `environment`, `project file`, `user file`,
    /// `default`, or `ui.json` for the one setting that lives there.
    pub source: String,
    /// Gh #218: the value resolved a `{env:}`/`{file:}` token and may
    /// be a secret - painters show `********`, while `value` keeps the
    /// real text for cycling and saving.
    pub masked: bool,
    /// The values left/right and Enter cycle through.
    pub values: Vec<String>,
    /// Gh #397: what **this project's** file says about the key, when the row
    /// has a project layer to speak for. `None` (the default) means the row is
    /// not project-overridable and cycles as before.
    ///
    /// The three states are the three meanings a project override has, and
    /// conflating the first two is what made a hand-written override a
    /// one-way door: the picker could not see that the key was present, so it
    /// could not offer to remove it.
    pub project: Option<ProjectOverride>,
}

/// Gh #397: the tri-state a project override actually has.
///
/// The cycle is `Inherit → On → Off → Inherit`, and **only `On`/`Off` write
/// a key**: `Inherit` is the key's absence, never `= false`. Off is a real
/// override (it beats a user-level `true`); inherit is the absence of an
/// opinion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProjectOverride {
    /// The project file does not mention the key: inherit the user file or
    /// the default.
    #[default]
    Inherit,
    /// The project file sets the key to the row's "on" value.
    On,
    /// The project file sets the key to the row's "off" value.
    Off,
}

impl ProjectOverride {
    /// The next state in the cycle, forward or back.
    #[must_use]
    pub fn step(self, forward: bool) -> Self {
        match (self, forward) {
            (Self::Inherit, true) | (Self::Off, false) => Self::On,
            (Self::On, true) | (Self::Inherit, false) => Self::Off,
            (Self::Off, true) | (Self::On, false) => Self::Inherit,
        }
    }

    /// The word the row shows (NFR-28: a word, never colour alone).
    #[must_use]
    pub fn word(self) -> &'static str {
        match self {
            Self::Inherit => "inherit",
            Self::On => "on",
            Self::Off => "off",
        }
    }

    /// The state a project file's value for `on`/`off` means. A value that is
    /// neither (a free-text or enum key) cannot be expressed as on/off, so it
    /// reads as `Inherit` and the row stays a normal cycle.
    #[must_use]
    pub fn from_value(value: Option<&str>, on: &str, off: &str) -> Self {
        match value {
            Some(value) if value == on => Self::On,
            Some(value) if value == off => Self::Off,
            _ => Self::Inherit,
        }
    }
}

impl SettingRow {
    /// The value the `/settings` painter shows (gh #246): human words,
    /// never stored primitives. `ui.fullscreen` reads scrollback /
    /// fullscreen; a two-option true/false row reads off / on;
    /// everything else (numbers, enums, placeholders) passes through.
    /// Display-only: cycling, editing, and persisting keep the raw
    /// value, so the stored TOML never sees these words.
    pub fn display_value(&self) -> String {
        // Gh #218: a substituted secret never reaches the screen.
        if self.masked {
            return "********".to_string();
        }
        if self.key == "ui.fullscreen" {
            return match self.value.as_str() {
                "true" => "fullscreen".to_string(),
                "false" => "scrollback".to_string(),
                _ => self.value.clone(),
            };
        }
        let mut sorted = self.values.clone();
        sorted.sort();
        if sorted == ["false", "true"] {
            return match self.value.as_str() {
                "true" => "on".to_string(),
                "false" => "off".to_string(),
                _ => self.value.clone(),
            };
        }
        self.value.clone()
    }
}

/// The rows `/settings` offers, re-read after every edit so value and
/// source move with it. `None` keeps the host's read-only dump.
pub type SettingsRows = Arc<dyn Fn() -> Vec<SettingRow> + Send + Sync>;

/// Applies a trust choice, returning the notice to show.
pub type TrustApply = Arc<dyn Fn(TrustChoice) -> String + Send + Sync>;
/// Whether the project still needs a trust decision (opens the modal at
/// startup, Pi's `hasTrustRequiringProjectResources`).
pub type TrustNeeded = Arc<dyn Fn() -> bool + Send + Sync>;

/// `/bug`: stage a bundle and return the inspection the user reviews. The host
/// owns the staging and the zip; the interface owns the screen and the key.
pub type StageBug = Arc<dyn Fn() -> Result<BugReview, String> + Send + Sync>;
pub use crate::chat_models::{LabelRecord, SessionTree, TreeFilterMode, TreeRow, TreeRowKind};
pub use crate::chat_pickers::TreeFilter;
pub use crate::chat_tree::TreePicker;

/// Open a `/tree` navigator over rows under a filter's config spelling
/// (gh #394): the seam the regression suite opens the navigator through,
/// so a shipped-defect guard can assert the painted frame without a host.
pub fn tree_picker(rows: Vec<TreeRow>, filter: &str) -> TreePicker {
    TreePicker::from_filter_name(rows, filter)
}
pub use crate::ui_state::UiState;

/// Branches at a record id in place and returns the replayed chain
/// for the transcript (gh #37, FR-UI-16); `None` when the branch
/// cannot land.
pub type BranchHere = Arc<dyn Fn(&str) -> Option<Vec<lca_protocol::Record>> + Send + Sync>;
/// Renames the live session, returning the notice (gh #37).
pub type RenameSession = Arc<dyn Fn(&str) -> Result<String, String> + Send + Sync>;
/// Forks at the nth user message, returning the new branch's id.
/// What a fork call reports (gh #203): the new session id when the
/// fork landed (so the interface can switch in-process), plus the
/// notice text either way.
#[derive(Debug, Clone)]
pub struct ForkReport {
    /// The forked session's id; `None` when the fork failed.
    pub id: Option<String>,
    /// The notice to show (success names the fork, failure the cause).
    pub notice: String,
}

/// Fork the live session at the nth user message.
pub type ForkAt = Arc<dyn Fn(usize) -> ForkReport + Send + Sync>;
/// Fork the live session at a record id (gh #37): `/jump` resolves a
/// bookmark name, then branches exactly where the mark points.
pub type ForkAtRecord = Arc<dyn Fn(&str) -> ForkReport + Send + Sync>;
/// Bookmark the nth user message (0-based) under a name, returning the
/// labeled record's id (gh #37, FR-SESS-10).
pub type SetLabel = Arc<dyn Fn(usize, &str) -> Result<String, String> + Send + Sync>;
/// Trash a session by id, returning the notice to show (gh #75).
/// The store keeps the directory under trash for recovery.
pub type DeleteSession = Arc<dyn Fn(&str) -> Result<String, String> + Send + Sync>;
/// Every live bookmark as `(name, record id)` pairs, sorted by name
/// (gh #37, FR-SESS-10).
pub type ListLabels = Arc<dyn Fn() -> Vec<(String, String)> + Send + Sync>;
/// Clones the live session at its tip under the optional name,
/// returning the new session's `(id, title)` (gh #205). The host
/// switches through [`SwitchSession`]; this hook only duplicates.
pub type CloneSession =
    Arc<dyn Fn(Option<String>) -> Result<(String, String), String> + Send + Sync>;
/// What a `/reload` refreshed (gh #130): the notice names it, the
/// themes and keybindings apply interface-side (the host owns the
/// files, the interface owns its state).
#[derive(Debug, Clone, Default)]
pub struct ReloadReport {
    /// One line for the transcript notice.
    pub notice: String,
    /// Fresh theme names for the `/theme` picker.
    pub themes: Vec<String>,
    /// Fresh key bindings for the key manager.
    pub key_bindings: std::collections::BTreeMap<String, Vec<String>>,
    /// A key file problem, when the file failed (defaults hold).
    pub keybinding_error: Option<String>,
}
/// Re-runs discovery without restarting (gh #130).
pub type Reload = Arc<dyn Fn() -> ReloadReport + Send + Sync>;
/// Lists the project's sessions, newest first (`/resume`, R2).
pub type SessionList = Arc<dyn Fn() -> Vec<crate::resume::SessionEntry> + Send + Sync>;
/// Switches the live session to `id` and returns its records (R3); `None`
/// when the session cannot be opened. Records rather than lines: the
/// transcript replays them with the live rendering (FR-UI-7), not a
/// plain-text dump.
pub type SwitchSession = Arc<dyn Fn(&str) -> Option<Vec<lca_protocol::Record>> + Send + Sync>;
/// Creates a fresh session in the working directory and returns its
/// id (gh #209); `None` when the host cannot (a tab reports it,
/// never invents an id).
pub type NewSession = Arc<dyn Fn() -> Option<String> + Send + Sync>;
/// Resolves a user attachment hash to `(media type, bytes)` for the
/// replayed transcript; `None` when the file is gone.
pub type LoadAttachment = Arc<dyn Fn(&str) -> Option<(String, Vec<u8>)> + Send + Sync>;

/// Optional host hooks the interface calls (P6): an external editor, the
/// `!`/`!!` shell path, and screen-mode persistence. Default: all absent,
/// so a host that wires none still gets the in-app behavior.
#[derive(Default)]
pub struct UiHooks {
    /// `/bug`: stage a scrubbed bundle and hand back the inspection screen plus
    /// the confirm/cancel decision (gh #411 gaps 2 and 3). Absent keeps the
    /// one-shot bundle `lca bug` uses, with no gate.
    pub stage_bug: Option<StageBug>,
    /// Run a shell command for `!`/`!!` mode; the `bool` is `true` for
    /// `!!` (excluded from the model's context). Returns the combined
    /// output to show in the transcript.
    pub run_shell: Option<ShellRunner>,
    /// Open `$EDITOR`/`$VISUAL` on the prompt text; `None` aborts. The
    /// terminal is restored around the call.
    pub external_editor: Option<ExternalEditor>,
    /// Run an interactive `!` command with the terminal handed to it (gh #407).
    pub run_interactive: Option<InteractiveShell>,
    /// Persist a runtime screen-mode change (fullscreen = `true`).
    pub persist_screen_mode: Option<ScreenModePersist>,
    /// Persist a runtime setting choice to the config file (E2):
    /// `/theme` writes `ui.theme`, `/thinking` writes `thinking`.
    pub persist_setting: Option<SettingPersist>,
    /// Persist a setting to the **project** file (gh #397), where `None`
    /// removes the key. Absent means the host cannot write a project layer,
    /// and the picker then shows the tri-state read-only rather than
    /// pretending an edit landed.
    pub persist_project_setting: Option<ProjectSettingPersist>,
    /// Whether the project layer is live in this session (FR-PERM-9): a
    /// project file is ignored until the project is trusted. The picker uses
    /// this to tell the user that a write landed in the file but will not take
    /// effect yet, rather than pretending it applied (gh #397).
    pub project_layer_trusted: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
    /// The live model list for `/model` (falls back to the startup
    /// `UiOptions::models` when absent).
    pub models: Option<ModelList>,
    /// The prompt templates for `/name` commands (gh #58): collected
    /// live, so a new file works without a restart. Absent keeps the
    /// unknown-command refusal.
    pub prompt_templates: Option<PromptTemplates>,
    /// What Esc Esc does with an empty editor (gh #132).
    pub double_escape_action: Option<DoubleEscapeAction>,
    /// The live display tunables (gh #82).
    pub display_tuning: Option<DisplayTuningHook>,
    /// The current session's id (gh #82's resume hint).
    pub current_session_id: Option<CurrentSessionId>,
    /// One step of the model cycle for the cycle keys (`true` = forward).
    pub cycle_model: Option<ModelCycle>,
    /// `Ctrl+S` in the model picker: persist the highlighted model as the
    /// default (`model` in the user config), returning the notice.
    pub save_default_model: Option<ModelSave>,
    /// `/thinking`'s Enter: clamp the chosen level to the current model's
    /// set, store it, and return the notice.
    pub set_thinking: Option<ThinkingSetter>,
    /// `Ctrl+S` in the thinking picker: persist the highlighted level as the
    /// default for new sessions (gh #396). Mirrors `save_default_model`.
    pub save_default_thinking: Option<ThinkingSave>,
    /// `/persona`: validate a bundle against this session before applying any of it
    /// (gh #412). Absent means the host cannot reach the model set or the tool
    /// registry, so `/persona` refuses rather than applying blind.
    pub persona_check: Option<PersonaCheck>,
    /// `/persona`: apply an already-validated bundle.
    pub persona_apply: Option<PersonaApply>,
    /// `/persona`: every persona that applies in this session, in name order. An
    /// untrusted project's personas are **absent** here rather than listed as
    /// unusable - the loader does not parse an untrusted file, so there is nothing
    /// to list (FR-PERM-9).
    pub persona_list: Option<PersonaList>,
    /// The levels the `/thinking` picker offers (gh #41): the current
    /// model's allowed set in canonical order. Absent means every
    /// level (a host without per-model sets); unsupported levels are
    /// hidden, never merely clamped after the fact.
    pub thinking_offered: Option<ThinkingOffered>,
    /// The counted-miss notice supplier (gh #42); `None` stays silent.
    pub cache_miss_notice: Option<CacheMissNotice>,
    /// The `/scoped-models` checklist rows with scope flags (gh #204).
    pub scoped_models: Option<ScopedModels>,
    /// The checklist's Enter: persist the checked ids and update the
    /// live rotation, returning the notice.
    pub save_scoped_models: Option<SaveScopedModels>,
    /// The `/settings` selector's rows (gh #30); absent keeps the
    /// read-only `settings` command dump.
    pub settings_rows: Option<SettingsRows>,
    /// Applies a `/trust` choice.
    pub trust_apply: Option<TrustApply>,
    /// Whether the project needs a trust decision at startup.
    pub trust_needed: Option<TrustNeeded>,
    /// The live session's entry tree as structured rows (FR-UI-16,
    /// gh #37, gh #231).
    pub session_tree: Option<SessionTree>,
    /// Creates a fresh session for a new tab (gh #209).
    pub new_session: Option<NewSession>,
    /// The configured tree filter mode for `/tree` (gh #231).
    pub tree_filter_mode: Option<TreeFilterMode>,
    /// Bookmark a tree row by record id for `/tree` label editing
    /// (gh #231).
    pub label_record: Option<LabelRecord>,
    /// Branch at a tree row in place (gh #37, FR-UI-16).
    pub branch_here: Option<BranchHere>,
    /// Rename the live session (gh #37).
    pub rename_session: Option<RenameSession>,
    /// Fork at the nth user message (0-based), reporting the new branch
    /// (FR-UI-16, gh #203).
    pub fork_at: Option<ForkAt>,
    /// Clone the live session at its tip under an optional name (gh #205).
    pub clone_session: Option<CloneSession>,
    /// Fork at a bookmark's record for `/jump` (gh #37).
    pub fork_record: Option<ForkAtRecord>,
    /// Bookmark a message for `/label` (gh #37).
    pub set_label: Option<SetLabel>,
    /// Trash a session from `/resume` (gh #75).
    pub delete_session: Option<DeleteSession>,
    /// List bookmarks for `/labels` (gh #37).
    pub list_labels: Option<ListLabels>,
    /// Re-run discovery without restarting (gh #130).
    pub reload: Option<Reload>,
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
    /// One `/mcp` verb (gh #53); absent with the status.
    pub mcp_action: Option<McpAction>,
    /// Poll a background `/mcp login` (gh #53); absent with the rest.
    pub poll_mcp: Option<McpPoll>,
    /// The `/mcp` modal's server list (gh #418 Phase G). Absent where the modal
    /// cannot exist (headless), which is what keeps the verb form working there.
    pub mcp_list: Option<McpList>,
    /// The modal's per-server detail fetch (gh #418 Phase G).
    pub mcp_detail: Option<McpDetail>,
    /// Start an interactive sign-in and answer its URL (gh #418 §3.4 item 3).
    pub mcp_begin_sign_in: Option<McpBeginSignIn>,
    /// Deliver a pasted redirect URL to a waiting flow (the SSH case).
    pub mcp_submit_redirect: Option<McpSubmitRedirect>,
    /// Abandon a waiting sign-in flow on `Esc`.
    pub mcp_cancel_sign_in: Option<McpCancelSignIn>,
    /// Fetch hub rows: installed plus catalogs, merged (#227). The
    /// chat runs it on a worker, never the interface thread.
    pub fetch_extensions: Option<crate::chat_hub::FetchExtensions>,
    /// Confirm an install the consent modal approved (#227): fire the
    /// worker now, report through `poll_install`.
    pub confirm_install: Option<crate::chat_hub::ConfirmInstall>,
    /// Poll a background install: `Some` posts its notice once (#227).
    pub poll_install: Option<crate::chat_hub::InstallPoll>,
    /// List managed extensions plus the strategy (gh #3): the
    /// manager reads local state, so the fetch is synchronous.
    pub list_managed: Option<crate::chat_extman::ListManaged>,
    /// Toggle one managed extension, persist + apply (gh #3).
    pub toggle_managed: Option<crate::chat_extman::ToggleManaged>,
    /// Pick the compaction strategy (gh #3).
    pub set_managed_strategy: Option<crate::chat_extman::SetManagedStrategy>,
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
    /// Where custom theme files resolve, in order (gh #70):
    /// explicit `--theme` dirs first, then `<config>/themes` unless
    /// `--no-themes` skipped it.
    pub theme_extra_dirs: Vec<PathBuf>,
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
    /// Open the session picker when the interface opens (gh #110:
    /// bare `-r`), instead of starting on a fresh session.
    pub open_resume_picker: bool,
    /// Plain-text rendering (FR-UI-5).
    pub plain: bool,
    /// Permission prompts are auto-approved this session (ADR-0042); the
    /// footer says so every frame.
    pub yolo: bool,
    /// Input captured before this loop existed (gh #400, type-while-loading).
    ///
    /// The host starts raw mode and its reader *before* the expensive startup
    /// load, so a keystroke typed in the first few hundred milliseconds is
    /// captured rather than interpreted by the shell's line discipline. The
    /// loop adopts the terminal this owns (no second reader, no handover) and
    /// replays the buffer in arrival order through its own handler. `None`
    /// keeps the old behaviour: the loop makes its own terminal.
    pub early_input: Option<crate::run_early_input::EarlyInput>,
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
    /// The session's dialog sender (gh #124): a worker asking a
    /// host-rendered question. Published by `run` like `prompt_slot`;
    /// `None` before `run` starts.
    pub dialog_slot:
        std::sync::Arc<std::sync::Mutex<Option<std::sync::mpsc::SyncSender<DialogExchange>>>>,
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
    /// How queued follow-ups flush at turn end (gh #76,
    /// `queue.follow_up_mode`): the host sets it from configuration.
    pub follow_up_mode: lca_protocol::QueueMode,
}

/// A worker asking a host-rendered question (gh #124): the question
/// plus where the answer goes. The TUI loop drains these and opens
/// native chrome; headless answers the denied values without asking.
#[derive(Debug)]
pub struct DialogExchange {
    /// The question.
    pub dialog: lca_protocol::UiDialog,
    /// Where the answer goes.
    pub respond: SyncSender<lca_protocol::DialogAnswer>,
}

/// The open dialog modal: the exchange plus per-kind UI state (the
/// select filter and highlight, the input buffer).
#[derive(Debug)]
pub struct DialogModal {
    /// The waiting worker.
    pub exchange: DialogExchange,
    /// The select filter text.
    pub query: String,
    /// The select matches (indices into the options).
    pub matches: Vec<usize>,
    /// The select highlight (an index into `matches`).
    pub selected: usize,
    /// The input buffer.
    pub input: String,
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
    /// Hand the terminal to an interactive `!` command (gh #407): the loop
    /// stops the interface, runs it with inherited stdio, and restarts - the
    /// same shape as [`Action::ExternalEditor`], because a fullscreen program on
    /// a pipe either refuses or blocks waiting for keys.
    InteractiveShell(String),
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
