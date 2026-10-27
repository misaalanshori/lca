//! Gh #371 bug 3: the IME anchor belongs to whoever owns the keyboard.
//!
//! `CURSOR_MARKER` is the engine's side channel for the hardware cursor
//! (`lca_tui::engine::core::extract_cursor_position`): a component emits it at
//! its insertion point, the renderer strips it, and the terminal's cursor is
//! moved there so an IME candidate window opens beside the field being typed
//! into. One marker per frame: the engine scans the whole document and the
//! **last** one wins, so a second marker silently steals the anchor.
//!
//! Before this module the composer always emitted its marker, so with a
//! keyboard-owning overlay open the IME anchored to the inactive bottom editor,
//! or to nothing when the overlay covered that column. The fix is ownership,
//! not emission: [`Chat::keyboard_owner`] is the single answer. The composer
//! asks it before emitting and every overlay field asks it through
//! [`Chat::caret_row`], so both sides read one predicate and cannot disagree
//! about who holds the caret; "two markers" and "none" stop being reachable
//! states.
//!
//! The caret's *painted form* is the composer's own: `\x1b[7m` … `\x1b[27m`
//! (gh #371 bug 2's channel-scoped closer), emitted only while the theme
//! paints a caret at all. On the plain theme the bare marker is all there is,
//! exactly as in the editor, because there the hardware cursor *is* the caret
//! (FR-UI-5). A field that painted differently from the composer would be a
//! bug users report, so both read [`Chat::paints_caret`].

use lca_tui::engine::core::CURSOR_MARKER;

use super::chat::Chat;

/// A one-line text field that takes keystrokes while its surface is open.
///
/// Not every keyboard-owning surface has one: a permission modal, a yes/no
/// `[y/n]` confirm, the trust picker and the `/login` provider list all take
/// keys but never accept text, so they own the keyboard and have no caret.
/// Those frames carry no marker at all, which is pi's behaviour for a focused
/// component that emits none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Field {
    /// `/model` search.
    ModelSearch,
    /// `/resume` search.
    ResumeSearch,
    /// The `/extensions` hub filter.
    ExtensionFilter,
    /// `/settings` type-to-search.
    SettingsSearch,
    /// `/settings` inline edit of a free-text row.
    SettingsEdit,
    /// `/tree` bookmark editing (`e`).
    TreeLabel,
    /// The masked `/login` secret field.
    Secret,
    /// A host dialog's `Input` field.
    DialogInput,
    /// A host dialog's `Select` search.
    DialogSelect,
    /// The `/mcp` sign-in screen's pasted redirect URL.
    McpRedirect,
    /// The `Ctrl+R` transcript search readout.
    TranscriptSearch,
}

/// Who holds the keyboard this frame - the **single** answer both sides read.
///
/// Three cases, because there are three genuinely different frames and a
/// two-way `Option` would blur two of them: an overlay that takes keys but has
/// no text field (a `[y/n]` confirm, a permission modal, the trust picker) owns
/// the keyboard and paints no caret, which is *not* the same frame as an idle
/// composer. Conflating them is how the composer would keep its marker while a
/// confirm dialog had focus - the mis-anchor this bug is about, one surface
/// over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Focus {
    /// Nothing else is open: the composer is the focused widget and emits.
    Composer,
    /// A surface with a text field is open, and *that* field emits.
    Field(Field),
}

impl Chat {
    /// What owns the keyboard this frame.
    ///
    /// The precedence is the **render** precedence, not the key-dispatch one:
    /// ownership follows what the user is actually looking at, because that is
    /// what the IME candidate window must sit beside. `compose_overlays` draws
    /// a picker over everything and returns early, so a picker outranks a
    /// modal, and the transcript search readout (dock chrome, drawn under both)
    /// ranks below them.
    pub(crate) fn keyboard_owner(&self) -> Focus {
        // A picker or modal is open: its field takes the caret. A surface with
        // no field (a `[y/n]` confirm, a permission prompt) answers `Composer`,
        // deliberately - the engine has no markerless-frame contract, and a
        // frame without a marker desynchronises `hardware_cursor_row` (see the
        // module doc). Anchoring an IME popup at an inactive composer is the
        // lesser fault of the two: it is in the wrong place, not a corrupted
        // screen. `MainScreenRenderer`'s no-marker arm is the real fix and lives
        // in a file this cycle does not own.
        if self.picker_open() {
            return self.picker_field().map_or(Focus::Composer, Focus::Field);
        }
        if self.world.modal_active() || self.world.dialog.is_some() || self.world.wizard.is_some() {
            return self.modal_field().map_or(Focus::Composer, Focus::Field);
        }
        // The search readout is dock chrome, so it is last: an overlay covers it.
        if self.search.is_some() {
            return Focus::Field(Field::TranscriptSearch);
        }
        Focus::Composer
    }

    /// The open picker's text field, in `compose_pickers` order.
    ///
    /// Only the pickers with a text field appear: `/theme`, `/trust`,
    /// `/thinking`, `/fork`, `/scoped-models`, `/grants`, the extension
    /// manager, the install consent and the stepped wizard all navigate with
    /// keys alone. `None` there means the surface owns the keyboard and paints
    /// no field.
    fn picker_field(&self) -> Option<Field> {
        if self
            .tree_picker
            .as_ref()
            .is_some_and(|p| p.editing.is_some())
        {
            return Some(Field::TreeLabel);
        }
        if self.resume_picker.is_some() {
            return Some(Field::ResumeSearch);
        }
        if let Some(picker) = &self.settings_picker {
            if picker.editing.is_some() {
                return Some(Field::SettingsEdit);
            }
            // Gh #371 bug 3: the search row is only painted once something has
            // been typed (`compose_pickers`), so with an empty query the surface
            // owns the keyboard and draws **no** field. Answering
            // `SettingsSearch` there would put the caret on a row that is not on
            // screen - the same defect one layer over.
            return (!picker.query.is_empty()).then_some(Field::SettingsSearch);
        }
        if self.model_picker.is_some() {
            return Some(Field::ModelSearch);
        }
        if self.extension_hub.is_some() {
            return Some(Field::ExtensionFilter);
        }
        // `/mcp` sign-in is the one manager screen with a field; every other
        // screen navigates by key.
        if self.mcp_manager.as_ref().is_some_and(|manager| {
            matches!(manager.screen, crate::chat_mcp::McpScreen::SignIn { .. })
        }) {
            return Some(Field::McpRedirect);
        }
        None
    }

    /// The open modal's text field, in `compose_modals` order (`None` for a
    /// modal that takes keys and paints no field).
    fn modal_field(&self) -> Option<Field> {
        if let Some(dialog) = &self.world.dialog {
            use lca_protocol::UiDialog;
            return match &dialog.exchange.dialog {
                UiDialog::Input { .. } => Some(Field::DialogInput),
                // A select filters as you type, but its query row only exists
                // once something has been typed (`compose_dialog`). With an
                // empty query the surface owns the keyboard and draws no
                // field, so the frame carries no marker - anchoring to nothing
                // beats anchoring to a field nobody is typing into.
                UiDialog::Select { .. } if !dialog.query.is_empty() => Some(Field::DialogSelect),
                UiDialog::Select { .. } | UiDialog::Confirm { .. } | UiDialog::Notify { .. } => {
                    None
                }
            };
        }
        if self.world.secret.is_some() {
            return Some(Field::Secret);
        }
        None
    }

    /// Whether the caret is painted into the row rather than left to the
    /// hardware cursor (FR-UI-5, ADR-0037): the plain theme renders no escapes
    /// at all, so there the terminal's own cursor is the caret.
    pub(crate) fn paints_caret(&self) -> bool {
        self.theme.colored
    }

    /// The zero-width marker plus, when the caret is painted, the reverse-video
    /// cell that is the caret. Both forms close with `27m` and nothing else
    /// (gh #371 bug 2): a full reset would strip the colour and decoration of
    /// everything after the caret in the row.
    fn caret_cell(&self) -> String {
        if self.paints_caret() {
            format!("{CURSOR_MARKER}\x1b[7m \x1b[27m")
        } else {
            CURSOR_MARKER.to_string()
        }
    }

    /// `value` with this field's caret at its insertion point - the end of what
    /// has been typed - when `field` owns the keyboard.
    ///
    /// The caret is a cell *appended* to the value rather than one replacing the
    /// last grapheme, which is pi's `input.ts` rule for a single-line field and
    /// the reason the anchor lands exactly on the insertion point. The editor
    /// replaces instead (`layoutText`), because its caret is usually mid-line;
    /// the painted form is identical either way. The row's caller pads from the
    /// measured width afterwards, so the extra cell costs no layout.
    pub(crate) fn caret_field(&self, value: &str, field: Field) -> String {
        if self.keyboard_owner() != Focus::Field(field) {
            return value.to_string();
        }
        format!("{value}{}", self.caret_cell())
    }

    /// One field's painted row: `prefix`, then the caret and the typed value,
    /// or - while nothing has been typed - the caret followed by `placeholder`.
    ///
    /// `placeholder` is empty for the pickers, whose search row is meaningful
    /// when empty (`search: ` with the caret after it), and non-empty for
    /// masked or unlabelled inputs, where the caret sits over the placeholder's
    /// first cell exactly as pi's `Input` renders a focused empty field.
    pub(crate) fn caret_row(
        &self,
        prefix: &str,
        value: &str,
        placeholder: &str,
        field: Field,
    ) -> String {
        if self.keyboard_owner() != Focus::Field(field) {
            return format!("{prefix}{value}");
        }
        if value.is_empty() {
            format!("{prefix}{}{placeholder}", self.caret_cell())
        } else {
            format!("{prefix}{value}{}", self.caret_cell())
        }
    }
}
