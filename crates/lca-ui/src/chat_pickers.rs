//! The runtime pickers, split from `chat.rs` (the 1,200-line ceiling).

/// A running `!`/`!!` command (R4): streamed output, a cancel handle, and
/// the command text for the finishing notice.
pub struct ShellRun {
    /// Output events from the shell worker.
    pub output: std::sync::mpsc::Receiver<crate::state::ShellEvent>,
    /// Cancels the command (Escape).
    pub cancel: crate::state::ShellHandle,
    /// The command text.
    pub command: String,
    /// Whether the command is excluded from the model's context (`!!`).
    pub excluded: bool,
}

/// The `/tree` branch selector (FR-UI-16).
pub struct TreePicker {
    /// `(session id, display label)` entries.
    pub entries: Vec<(String, String)>,
    /// The highlighted row.
    pub selected: usize,
}

impl GrantPicker {
    /// The body-row map for mouse hit-testing (gh #167): `Some(index)`
    /// for entry rows, `None` for the header, blanks, and group
    /// dividers. Mirrors the grants arm of `compose_pickers` one row at
    /// a time - the click-each-row guards fail if they drift apart.
    pub fn mouse_rows(&self) -> Vec<Option<usize>> {
        let mut rows = vec![None, None];
        let mut group: Option<bool> = None;
        // The body opens with a header plus a blank line.
        let mut last_blank = true;
        for (index, entry) in self.entries.iter().enumerate() {
            if group != Some(entry.install_consent) {
                group = Some(entry.install_consent);
                if !last_blank {
                    rows.push(None);
                }
                rows.push(None);
            }
            rows.push(Some(index));
            last_blank = false;
        }
        rows
    }
}

/// The `/grants` picker (S8): the project's grants in two groups.
pub struct GrantPicker {
    /// The rows, install-consent first.
    pub entries: Vec<crate::state::GrantEntry>,
    /// The highlighted row.
    pub selected: usize,
}

/// The `/theme` picker: a live preview that restores on cancel (FR-UI-17).
pub struct ThemePicker {
    /// The highlighted row.
    pub selected: usize,
    /// The theme name to restore when the picker is cancelled.
    pub original: String,
}

/// The `/trust` picker (ADR-0039): trust the project folder, so safe
/// in-workspace commands run without a prompt. The four rows mirror Pi's
/// trust selector.
pub struct TrustPicker {
    /// The highlighted row.
    pub selected: usize,
}

/// The `/trust` rows, in order.
pub const TRUST_OPTIONS: &[&str] = &[
    "Trust this project (remember)",
    "Trust this project (this session only)",
    "Do not trust (remember)",
    "Do not trust (this session only)",
];

/// The `/thinking` picker: the session's reasoning level (R1). Row 0 is
/// "unset" (the provider's own default); the rest are pi's levels with
/// their cost/latency descriptions.
pub struct ThinkingPicker {
    /// The highlighted row (0 = unset).
    pub selected: usize,
    /// The offered level names in canonical order (gh #41): a model
    /// without `high` never shows it. Every level when the host names
    /// no set.
    pub offered: Vec<String>,
}

/// The canonical level names in picker order.
pub fn thinking_offered_all() -> Vec<String> {
    THINKING_LEVELS
        .iter()
        .map(|(name, _)| (*name).to_string())
        .collect()
}

/// The description for one level name, for the picker's rows.
pub fn thinking_description(name: &str) -> &'static str {
    THINKING_LEVELS
        .iter()
        .find(|(candidate, _)| *candidate == name)
        .map(|(_, description)| *description)
        .unwrap_or("")
}

/// The `/model` picker (R9): a searchable list of the provider's models,
/// pi's `model-selector.ts` shape (minus the catalog refresh, which the
/// host does at startup).
pub struct ModelPicker {
    /// Every offered row: `(raw id, display label)`. Selection returns the
    /// id; rendering shows the label (G2: labels are display-only).
    pub models: Vec<crate::state::ModelRow>,
    /// The typed search query.
    pub query: String,
    /// The indices into `models` that match.
    pub matches: Vec<usize>,
    /// The highlighted row (an index into `matches`).
    pub selected: usize,
}

impl ModelPicker {
    /// Open the picker over a list of `(id, label)` rows.
    pub fn new(models: Vec<crate::state::ModelRow>) -> ModelPicker {
        let matches = (0..models.len()).collect();
        ModelPicker {
            models,
            query: String::new(),
            matches,
            selected: 0,
        }
    }

    /// Re-filter after a query change, keeping the selection in range. The
    /// label starts with the id, so matching on it covers both.
    pub fn refilter(&mut self) {
        let needle = self.query.to_lowercase();
        self.matches = self
            .models
            .iter()
            .enumerate()
            .filter(|(_, (_, label))| label.to_lowercase().contains(&needle))
            .map(|(index, _)| index)
            .collect();
        if self.selected >= self.matches.len() {
            self.selected = self.matches.len().saturating_sub(1);
        }
    }

    /// The highlighted row's raw id, when any: what selection passes on.
    pub fn selected_model(&self) -> Option<&str> {
        self.matches
            .get(self.selected)
            .and_then(|index| self.models.get(*index))
            .map(|(id, _)| id.as_str())
    }
}

/// The `/fork` user-message picker (gh #203): the transcript's user
/// messages in order, pi's `UserMessageSelector` shape (one-line preview
/// + `Message N of M`, latest selected) in our picker chrome.
pub struct ForkPicker {
    /// The user message texts, oldest first.
    pub messages: Vec<String>,
    /// The highlighted row (defaults to the latest).
    pub selected: usize,
}

impl ForkPicker {
    /// Open over the transcript's user messages, latest selected.
    pub fn new(messages: Vec<String>) -> ForkPicker {
        let selected = messages.len().saturating_sub(1);
        ForkPicker { messages, selected }
    }
}

/// The `/scoped-models` checklist (gh #204): every offered model with
/// its checked state, pi's `scoped-models-selector` shape (checkbox +
/// label + context) in our picker chrome. Enter saves, escape
/// discards, `a` flips the whole list.
pub struct ScopedModelsPicker {
    /// The rows in catalog order.
    pub rows: Vec<crate::state::ScopedModelRow>,
    /// The checked ids (starts as the ids flagged enabled).
    pub checked: Vec<String>,
    /// The highlighted row.
    pub selected: usize,
}

impl ScopedModelsPicker {
    /// Open over the catalog rows, current scope checked.
    pub fn new(rows: Vec<crate::state::ScopedModelRow>) -> ScopedModelsPicker {
        let checked = rows
            .iter()
            .filter(|row| row.enabled)
            .map(|row| row.id.clone())
            .collect();
        ScopedModelsPicker {
            rows,
            checked,
            selected: 0,
        }
    }

    /// Whether the row's id is checked.
    pub fn is_checked(&self, id: &str) -> bool {
        self.checked.iter().any(|checked| checked == id)
    }

    /// Flip the highlighted row's checked state.
    pub fn toggle_selected(&mut self) {
        let Some(row) = self.rows.get(self.selected) else {
            return;
        };
        if let Some(at) = self.checked.iter().position(|id| id == &row.id) {
            self.checked.remove(at);
        } else {
            self.checked.push(row.id.clone());
        }
    }

    /// Check every row, or clear the whole list when all are checked.
    pub fn toggle_all(&mut self) {
        if self.checked.len() == self.rows.len() {
            self.checked.clear();
        } else {
            self.checked = self.rows.iter().map(|row| row.id.clone()).collect();
        }
    }
}

/// The `/settings` selector (gh #30, EFG-030): pi's interactive list of
/// configurable keys, each row showing key, value, and the winning
/// source (our FR-CFG-2 column, which pi's list does not carry).
impl SettingsPicker {
    /// The body-row map for mouse hit-testing (gh #167): `Some(index)`
    /// for setting rows, `None` for the header, blanks, and section
    /// dividers. Mirrors the settings arm of `compose_pickers` - the
    /// click-each-row guards fail if they drift apart.
    pub fn mouse_rows(&self) -> Vec<Option<usize>> {
        let mut rows = vec![None, None];
        let mut section = "";
        for (index, row) in self.rows.iter().enumerate() {
            if !row.section.is_empty() && row.section != section {
                section = row.section.as_str();
                rows.push(None);
            }
            rows.push(Some(index));
        }
        rows
    }
}

pub struct SettingsPicker {
    /// The rows: key, current value, winning source, cycle values.
    pub rows: Vec<crate::state::SettingRow>,
    /// The highlighted row.
    pub selected: usize,
    /// The inline-edit buffer while a free-text row edits (gh #174):
    /// `Some` means keystrokes land here, not in navigation.
    pub editing: Option<String>,
}

/// The thinking levels and their descriptions, from pi's
/// `thinking-selector.ts` (`LEVEL_DESCRIPTIONS`).
pub const THINKING_LEVELS: &[(&str, &str)] = &[
    ("off", "No reasoning"),
    ("minimal", "Very brief reasoning (~1k tokens)"),
    ("low", "Light reasoning (~2k tokens)"),
    ("medium", "Moderate reasoning (~8k tokens)"),
    ("high", "Deep reasoning (~16k tokens)"),
    ("xhigh", "Extra-high reasoning (~32k tokens)"),
    ("max", "Maximum reasoning"),
];
