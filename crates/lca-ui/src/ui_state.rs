//! The interface's mutable world state (ceiling split from
//! `state.rs`): `UiState` plus its constructor. Behavior unchanged.

use std::path::PathBuf;

use crate::state::{
    BugReview, DialogModal, GrantPrompt, LoginHistoryEntry, PermissionModal, PickerPrompt,
    SecretPrompt, SwitchPrompt, UiOptions,
};
/// The ui-world state: the options, the open modals, and the small flags
/// the interface tracks across keys. No buffer, no transcript, no editing.
pub struct UiState {
    /// Static inputs.
    pub options: UiOptions,
    /// The loop's wake channel (gh #370 flaw 1): a producer that enqueues something
    /// visible signals this **after** the enqueue, so the loop does not have to wait
    /// out its tick. `None` until the loop installs it, which is what a test or an
    /// embedder with no loop looks like - the tick then covers everything.
    pub waker: Option<crate::run_wake::Waker>,
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
    /// A staged `/bug` bundle awaiting the user's review (gh #411 gap 2): the
    /// inspection is the gate, so nothing is zipped until this is answered.
    pub bug_review: Option<BugReview>,
    /// A background login/identity step is running (R4); the label names
    /// it (and carries the auth URL once the provider has asked for one).
    pub login_waiting: Option<String>,
    /// The sign-in URL the current step shows, if any (gh #398 gap 1).
    ///
    /// Kept beside `login_waiting` rather than parsed out of it at render
    /// time, because the URL is the copy target and the label is prose: one
    /// is bytes the user pastes into a browser, the other is a sentence. The
    /// copy path never reads the rendered rows, so wrap padding and the
    /// modal's borders cannot reach the clipboard.
    pub auth_url: Option<String>,
    /// When the in-place `Copied URL to clipboard` confirmation expires
    /// (gh #398, `None` when nothing was copied this step).
    ///
    /// A timestamp rather than a bool because the confirmation is
    /// **transient** - it reuses the toast window so the two copy notices
    /// fade on the same clock instead of inventing a second timer.
    pub url_copied_at: Option<std::time::Instant>,
    /// The steps of a multi-step login that have already finished, oldest
    /// first (gh #398 gap 2).
    ///
    /// A step that completes becomes **history** and the next prompt appears
    /// beneath it, so the modal reads as a transcript of the flow. Before this,
    /// each step cleared the previous one and a failure left the user without
    /// the URL they were told to visit or the code they were told to paste - the
    /// one thing that makes a failure debuggable is being able to point at the
    /// step that failed.
    ///
    /// One list rather than one field per kind, because every value a step
    /// collects is the same shape once it is behind you (a prompt and what was
    /// typed), and a step kind added later costs nothing here.
    pub login_history: Vec<LoginHistoryEntry>,
    /// Ctrl+C seen once on an idle prompt.
    pub ctrl_c_armed: bool,
    /// The open stepped settings wizard, if any (gh #397): a related group
    /// walked as ordered steps, committing only when finished.
    pub wizard: Option<crate::chat_pickers::SteppedWizard>,
    /// The extension side panel is open.
    pub panel_open: bool,
    /// An extension modal is open (one at a time, user-dismissible:
    /// capability catalog `ui`).
    pub modal_open: bool,
    /// A host-rendered dialog is open (gh #124): one modal at a time,
    /// so this and `permission` never stack (the drains check both).
    pub dialog: Option<DialogModal>,
    /// Scroll offsets by region (gh #172): the wheel moves them, and
    /// scroll containers clip from them on the next frame.
    pub ext_scroll: std::collections::HashMap<String, usize>,
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
            waker: None,
            permission: None,
            secret: None,
            picker: None,
            grant: None,
            switch_confirm: None,
            bug_review: None,
            login_waiting: None,
            auth_url: None,
            url_copied_at: None,
            login_history: Vec::new(),
            ctrl_c_armed: false,
            wizard: None,
            panel_open: false,
            modal_open: false,
            dialog: None,
            ext_scroll: std::collections::HashMap::new(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{
        DisplayTuning, LoginNext, UiHooks, apply_login_next, key_input, sanitize_block,
        sanitize_text,
    };
    use crate::widget_lines;
    use lca_protocol::CommandEffect;
    use lca_protocol::{StopReason, TurnStatus, Usage};
    use std::sync::Arc;

    fn options() -> UiOptions {
        UiOptions {
            prompt_slot: Default::default(),
            dialog_slot: Default::default(),
            pending_models: None,
            model_label: Arc::new(std::sync::Mutex::new("p/m".into())),
            context_window: Arc::new(std::sync::Mutex::new(0)),
            thinking: Arc::new(std::sync::Mutex::new(None)),
            theme: "auto".to_string(),
            theme_extra_dirs: Vec::new(),
            themes: crate::theme::THEMES.iter().map(|s| s.to_string()).collect(),
            initial_lines: Vec::new(),
            initial_header: (Vec::new(), Vec::new()),
            initial_records: Vec::new(),
            initial_tail_lines: Vec::new(),
            initial_messages: Vec::new(),
            open_resume_picker: false,
            yolo: false,
            early_input: None,
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
            follow_up_mode: lca_protocol::QueueMode::OneAtATime,
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
        assert_eq!(
            widget_lines(
                &nodes,
                &crate::ext_widgets::WidgetCtx {
                    theme: &crate::theme::Theme::colored(),
                    width: 60,
                    region: "panel",
                    offsets: &std::collections::HashMap::new(),
                },
            ),
            vec!["hello", "k: v"]
        );
    }

    #[test]
    fn widget_lines_guards_against_a_self_referential_child() {
        use lca_protocol::Widget;
        let nodes = vec![Widget::Boxed {
            title: None,
            border: None,
            background: None,
            child: 0,
        }];
        assert!(
            widget_lines(
                &nodes,
                &crate::ext_widgets::WidgetCtx {
                    theme: &crate::theme::Theme::colored(),
                    width: 60,
                    region: "panel",
                    offsets: &std::collections::HashMap::new(),
                },
            )
            .is_empty()
        );
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

    // Verifies: gh #82 - the tuning defaults mirror the config
    // defaults (the hook only overrides what the file sets).
    #[test]
    fn display_tuning_defaults_mirror_config() {
        let tuning = DisplayTuning::default();
        assert_eq!(tuning.autocomplete_max_visible, 5);
        assert_eq!(tuning.editor_padding_x, 0);
        assert_eq!(tuning.output_pad, 1);
        assert_eq!(tuning.fullscreen_scrollbar, "auto");
        assert!(tuning.fullscreen_copy_on_select);
        assert_eq!(tuning.fullscreen_wheel_lines, "auto");
        assert_eq!(tuning.fullscreen_exit_output, "transcript");
        assert!(tuning.show_images);
        assert_eq!(tuning.image_width_cells, 60);
        assert!(tuning.show_progress);
        assert!(tuning.auto_resize_images);
        assert_eq!(tuning.code_block_indent, "  ");
        assert_eq!(tuning.mermaid, "streaming");
    }

    #[test]
    fn usage_reexports_compile() {
        let _ = Usage::default();
        let _ = TurnStatus::Ok;
        let _ = StopReason::Stop;
    }
}
