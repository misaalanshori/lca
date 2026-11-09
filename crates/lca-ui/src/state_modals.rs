//! The modal and prompt state: what the interface is currently asking, and
//! where the answer goes.
//!
//! Split out of `state.rs` at gate 11's 1,200-line ceiling (gh #574) - the
//! second seam the brief named. These are the structs that describe a screen
//! waiting on a person (`/login`'s secret, picker and grant prompts, the
//! permission and dialog modals, the `/bug` review, the switch prompt) plus
//! the login-history line a finished step leaves behind. They are grouped
//! because they share one sentence: "a worker is blocked on an answer".
//!
//! Every public path is preserved by a re-export from `state.rs`, so
//! `lca_ui::state::PermissionModal` and the crate-root re-export resolve
//! exactly as before; what moved is the definition, not the name.

use std::sync::mpsc::SyncSender;

use crate::state::{BugResolve, PickerOption};

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

/// A finished login step, kept so the flow reads as a transcript (gh #398
/// gap 2).
///
/// [`Self::Answer`] is the only variant that can carry something the user typed,
/// and its value is already redacted where the step declared itself a secret -
/// `login_step`'s recorder is the sole construction path, so a secret cannot
/// reach the screen by being appended.
pub enum LoginHistoryEntry {
    /// A step that showed something and was not answered: the sign-in URL's
    /// page, the device code, a `waiting…` line the provider has moved past.
    Message(String),
    /// A step the user answered. `value` is what to show - the typed text, or
    /// asterisks when [`SecretPrompt::masked`] was set.
    Answer {
        /// The question this answered.
        prompt: String,
        /// What to show as the user's input.
        value: String,
        /// The answer in the clear, kept only for a step that was **not**
        /// masked. A masked step stores `None`, so the secret does not survive
        /// anywhere in the interface once the field is gone.
        plain: Option<String>,
    },
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

/// A yes/no confirm for switching to a freshly signed-in provider.
pub struct SwitchPrompt {
    /// The provider just signed in to.
    pub provider: String,
    /// The question naming it.
    pub prompt: String,
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
