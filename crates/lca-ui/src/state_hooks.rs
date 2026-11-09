//! The host-hook and callback `Arc` aliases: the vocabulary a host speaks when
//! it wires the interface to the outside world.
//!
//! Split out of `state.rs` at gate 11's 1,200-line ceiling (gh #574). These are
//! the 55 `pub type` aliases `state.rs` carried; the *structs and enums* they
//! name (`UiHooks`, `UiOptions`, the modal state, the `/settings` rows) stay
//! where they were, because those are state and these are the signatures a host
//! fills in.
//!
//! Nothing moves for a caller: `state.rs` re-exports every name, so
//! `lca_ui::state::CommandInvoker` - and every other path that resolved before -
//! resolves after. This is the same pattern `state.rs` already used for
//! `setting_row.rs` (gh #30), which is what makes the split cheap: a re-export
//! can preserve a path, so a moved alias is not a renamed one.

use std::sync::Arc;

use lca_protocol::CommandEffect;

use crate::state::{
    BugReview, CompactState, DisplayTuning, ForkReport, GrantEntry, LoginNext, ModelRow,
    ReloadReport, ScopedModelRow, ShellEvent, TrustChoice,
};

/// How the input editor invokes a registered slash command.
pub type CommandInvoker = Arc<dyn Fn(&str, &str) -> CommandEffect + Send + Sync>;

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

/// What a background `/compact` is doing. The command's summarization
/// call runs on its own thread (it is a model round-trip, and freezing
/// the interface for it is how pi ended up with a dedicated compaction
/// indicator), and the loop polls this every tick - the same shape as
/// [`LoginPoll`], including the handoff rule: a hook **consumes** its
/// `Done` on read (like `poll_login`'s `take()`), so a finished
/// compaction is reported once rather than on every tick.
pub type CompactPoll = Arc<dyn Fn() -> CompactState + Send + Sync>;

/// The host's answer to an inspection: `(confirmed, )` and back comes the notice
/// to show. `false` must leave nothing on disk (gh #411 design point 4).
pub type BugResolve = Arc<dyn Fn(bool) -> String + Send + Sync>;

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

/// Cancels a running `!`/`!!` command (Escape, R4).
pub type ShellHandle = Arc<dyn Fn() + Send + Sync>;

/// Writes text to the system clipboard, returning `true` only when the
/// write was verified (R6). `None` falls back to OSC 52, which reports
/// unverified.
pub type ClipboardWriter = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// Reads the clipboard's text for a right-button paste (gh #374 gap 4).
pub type ClipboardReader = Arc<dyn Fn() -> Option<String> + Send + Sync>;

/// Opens a URL from an OSC-8 link click (R6/S7). `Err` names the reason
/// (no launcher found, the launcher refused), which the notice shows.
pub type OpenUrl = Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync>;

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

/// Applies a trust choice, returning the notice to show.
pub type TrustApply = Arc<dyn Fn(TrustChoice) -> String + Send + Sync>;

/// Whether the project still needs a trust decision (opens the modal at
/// startup, Pi's `hasTrustRequiringProjectResources`).
pub type TrustNeeded = Arc<dyn Fn() -> bool + Send + Sync>;

/// `/bug`: stage a bundle and return the inspection the user reviews. The host
/// owns the staging and the zip; the interface owns the screen and the key.
pub type StageBug = Arc<dyn Fn() -> Result<BugReview, String> + Send + Sync>;

/// Branches at a record id in place and returns the replayed chain
/// for the transcript (gh #37, FR-UI-16); `None` when the branch
/// cannot land.
pub type BranchHere = Arc<dyn Fn(&str) -> Option<Vec<lca_protocol::Record>> + Send + Sync>;

/// Renames the live session, returning the notice (gh #37).
pub type RenameSession = Arc<dyn Fn(&str) -> Result<String, String> + Send + Sync>;

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
