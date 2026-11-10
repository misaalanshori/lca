//! The human seams: what a prompt answers, the swappable slots the engines
//! hold, and the result the authorize flow returns.
//!
//! Split from `lib.rs` for gate 11; the traits, their docs and the
//! deny-when-nothing-is-installed behaviour are unchanged, and `lib.rs`
//! re-exports every `pub` item named here, so `lca_permissions::SharedPrompt`
//! and friends are the same paths they were.

use crate::action::Action;
use crate::store::ProposalDiff;

/// How permission prompts are answered for this process (ADR-0042).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PermissionMode {
    /// Ask the user: the default.
    #[default]
    Ask,
    /// Answer every prompt "always, for this exact pattern" without asking,
    /// and record it like a human answer. Explicit deny rules still deny:
    /// yolo answers prompts, it does not overrule the user's own words.
    Yolo,
}

impl PermissionMode {
    /// The config spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            PermissionMode::Ask => "ask",
            PermissionMode::Yolo => "yolo",
        }
    }

    /// Parse the config spelling.
    pub fn parse(text: &str) -> Option<PermissionMode> {
        match text {
            "ask" => Some(PermissionMode::Ask),
            "yolo" => Some(PermissionMode::Yolo),
            _ => None,
        }
    }
}

/// What the user chose at a prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Allow this call only.
    Once,
    /// Allow this call and persist its pattern.
    Always,
    /// Allow this call and trust the project folder for this session:
    /// later commands that provably stay inside the workspace run without a
    /// prompt (permission-UX plan §3.1). Never persisted.
    TrustFolder,
    /// Refuse the call.
    Denied,
}

/// The approval prompts. The TUI implements this with modals; headless mode
/// implements it by denying (exit code 4, `docs/headless.md`).
pub trait PermissionPrompt: Send {
    /// Ask about one action.
    fn ask(&mut self, action: &Action) -> Decision;
    /// Ask about one action with tool hints attached (#78): the prompt
    /// may show them; the decision path never reads them (a hint is
    /// not a bypass, ADR-0047, which superseded ADR-0045). The default
    /// keeps old behavior for a prompt with no annotated surface.
    fn ask_annotated(
        &mut self,
        action: &Action,
        annotations: Option<&lca_protocol::ToolAnnotations>,
    ) -> Decision {
        let _ = annotations;
        self.ask(action)
    }
    /// Show the proposal difference; true applies the new set.
    fn review_proposals(&mut self, diff: &ProposalDiff) -> bool;
}

/// A swappable prompt slot. Every capability engine the host builds (WASM and
/// native) gets one for its whole life; the interface installs the current
/// turn's real prompt into it, so an extension's own `process`/`pty` commands
/// reach the same modal the model's commands do (capability catalog: each
/// command still asks for approval). With nothing installed it denies, which
/// is the right answer headless and between turns.
#[derive(Default, Clone)]
pub struct SharedPrompt {
    inner: SharedPromptSlot,
}

/// The interior of [`SharedPrompt`]: an optional prompt, swappable at runtime.
type SharedPromptSlot = std::sync::Arc<
    std::sync::Mutex<Option<std::sync::Arc<std::sync::Mutex<dyn PermissionPrompt>>>>,
>;

impl SharedPrompt {
    /// Route subsequent asks to `prompt`.
    pub fn set(&self, prompt: std::sync::Arc<std::sync::Mutex<dyn PermissionPrompt>>) {
        *self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(prompt);
    }

    fn current(&self) -> Option<std::sync::Arc<std::sync::Mutex<dyn PermissionPrompt>>> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

impl PermissionPrompt for SharedPrompt {
    fn ask(&mut self, action: &Action) -> Decision {
        match self.current() {
            Some(prompt) => prompt
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .ask(action),
            None => Decision::Denied,
        }
    }

    /// gh #418 Phase C#C5 (gh #303): forward the tool hints.
    ///
    /// The trait default drops them and calls `ask`, which is right for a
    /// prompt with no annotated surface but wrong for this slot: it is the
    /// object every engine holds, so the default silently flattened every
    /// hinted action on its way to the real prompt. Forwarding keeps the
    /// hints a *display* concern - the decision still comes from the
    /// installed prompt's own rules (a hint is not a bypass, ADR-0047).
    fn ask_annotated(
        &mut self,
        action: &Action,
        annotations: Option<&lca_protocol::ToolAnnotations>,
    ) -> Decision {
        match self.current() {
            Some(prompt) => prompt
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .ask_annotated(action, annotations),
            None => Decision::Denied,
        }
    }

    fn review_proposals(&mut self, diff: &ProposalDiff) -> bool {
        match self.current() {
            Some(prompt) => prompt
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .review_proposals(diff),
            None => false,
        }
    }
}

/// One host-rendered question an extension asks (gh #124, gh #172): the
/// answers cross back as plain data, and the chrome is always the
/// host's own (spoof-proof by construction).
///
/// The swappable slot below mirrors [`SharedPrompt`]: with nothing
/// installed every question answers its denied value (`false`/`None`,
/// notify drops), which is the right answer headless and between
/// sessions.
pub trait DialogPrompt: Send {
    /// Yes/No buttons; `false` is no or dismissed.
    fn confirm(&mut self, title: &str, message: &str) -> bool;
    /// A picker over options; `None` is dismissed.
    fn select(&mut self, title: &str, options: &[String]) -> Option<String>;
    /// One line of text; `None` is dismissed.
    fn input(&mut self, label: &str, placeholder: Option<&str>) -> Option<String>;
    /// A transient notice; fire and forget.
    fn notify(&mut self, message: &str, level: &str);
}

/// A swappable dialog slot (the [`SharedPrompt`] pattern for questions).
#[derive(Default, Clone)]
pub struct SharedDialogs {
    /// The installed prompter, when any.
    inner: SharedDialogsSlot,
}

/// The interior of [`SharedDialogs`].
type SharedDialogsSlot =
    std::sync::Arc<std::sync::Mutex<Option<std::sync::Arc<std::sync::Mutex<dyn DialogPrompt>>>>>;

impl SharedDialogs {
    /// Route subsequent questions to `prompt`.
    pub fn set(&self, prompt: std::sync::Arc<std::sync::Mutex<dyn DialogPrompt>>) {
        *self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(prompt);
    }

    /// The installed prompter, when any.
    fn current(&self) -> Option<std::sync::Arc<std::sync::Mutex<dyn DialogPrompt>>> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

impl DialogPrompt for SharedDialogs {
    fn confirm(&mut self, title: &str, message: &str) -> bool {
        match self.current() {
            Some(prompt) => prompt
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .confirm(title, message),
            None => false,
        }
    }

    fn select(&mut self, title: &str, options: &[String]) -> Option<String> {
        match self.current() {
            Some(prompt) => prompt
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .select(title, options),
            None => None,
        }
    }

    fn input(&mut self, label: &str, placeholder: Option<&str>) -> Option<String> {
        match self.current() {
            Some(prompt) => prompt
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .input(label, placeholder),
            None => None,
        }
    }

    fn notify(&mut self, message: &str, level: &str) {
        if let Some(prompt) = self.current() {
            prompt
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .notify(message, level);
        }
    }
}

/// Result of one authorize call.
#[derive(Debug, Clone)]
pub struct Outcome {
    /// Whether the action may run.
    pub allowed: bool,
    /// Whether the action prompt was shown.
    pub prompted: bool,
    /// Whether a proposal review was shown.
    pub reviewed: bool,
    /// The pattern persisted, when the user chose always.
    pub stored_pattern: Option<String>,
    /// Whether a deny rule refused the action (no prompt was shown).
    pub denied_by_rule: bool,
    /// Whether an allow rule allowed the action (gh #259): no prompt
    /// was shown, but the caller records it — a rule decision belongs
    /// in the log even when nobody was asked.
    pub allowed_by_rule: bool,
    /// Whether yolo mode answered for the user (ADR-0042). The caller
    /// records these like a human "always" answer, so the audit trail is
    /// complete even though no prompt was shown.
    pub yolo: bool,
}

impl Outcome {
    /// Convenience: allowed.
    pub fn allowed(&self) -> bool {
        self.allowed
    }

    /// Convenience: denied.
    pub fn denied(&self) -> bool {
        !self.allowed
    }
}
