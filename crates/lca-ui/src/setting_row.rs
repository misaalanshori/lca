//! The `/settings` row types (gh #30), split out of `state.rs` at gate 11's
//! 1,200-line ceiling. Definitions and docs unchanged; the public path
//! (`lca_ui::state::SettingRow`) is re-exported, so no user of them moves.

use std::sync::Arc;

use crate::state::ProjectOverride;

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
