//! The runtime pickers, split from `chat.rs` (the 1,200-line ceiling).
//! The `/tree` navigator lives in `chat_tree.rs` (gh #394 split by
//! concern: its layout math outgrew this file), re-exported here so
//! `chat_pickers::TreePicker` still names it.

pub use crate::chat_tree::TreePicker;

/// The row text `lca-session` gives an assistant record that carried no
/// prose at all - only tool calls (gh #394 fix 3).
pub const TEXTLESS_DISPATCH: &str = "(tool call)";

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

/// The `/tree` filter vocabulary (gh #231): pi's `treeFilterMode`
/// order, so `f` cycles default → no-tools → user-only →
/// labeled-only → all → default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeFilter {
    /// User and assistant traffic plus landmarks (tool rows hide).
    Default,
    /// Default minus textless tool-call assistants.
    NoTools,
    /// Prompts only.
    UserOnly,
    /// Bookmarked rows only.
    LabeledOnly,
    /// Every row.
    All,
}

impl TreeFilter {
    /// Parse the `ui.tree_filter_mode` value; anything unknown (or
    /// unset) opens on the default view.
    pub fn parse(raw: &str) -> TreeFilter {
        match raw {
            "no-tools" => TreeFilter::NoTools,
            "user-only" => TreeFilter::UserOnly,
            "labeled-only" => TreeFilter::LabeledOnly,
            "all" => TreeFilter::All,
            _ => TreeFilter::Default,
        }
    }

    /// The config-file spelling.
    pub fn name(self) -> &'static str {
        match self {
            TreeFilter::Default => "default",
            TreeFilter::NoTools => "no-tools",
            TreeFilter::UserOnly => "user-only",
            TreeFilter::LabeledOnly => "labeled-only",
            TreeFilter::All => "all",
        }
    }

    /// One step forward, wrapping (pi's `cycleForward`).
    pub fn cycle_forward(self) -> TreeFilter {
        match self {
            TreeFilter::Default => TreeFilter::NoTools,
            TreeFilter::NoTools => TreeFilter::UserOnly,
            TreeFilter::UserOnly => TreeFilter::LabeledOnly,
            TreeFilter::LabeledOnly => TreeFilter::All,
            TreeFilter::All => TreeFilter::Default,
        }
    }

    /// Whether a row shows under this filter (gh #231, gh #394).
    ///
    /// A textless assistant row - one that was nothing but tool calls - is
    /// noise under both look-at-the-conversation filters, because its tool
    /// rows already say what happened (pi's `applyFilter` rule). The live
    /// leaf is exempt, so the cursor always has a visible home.
    pub(crate) fn passes(self, row: &crate::state::TreeRow) -> bool {
        use crate::state::TreeRowKind;
        let textless_dispatch =
            row.kind == TreeRowKind::Assistant && row.text == TEXTLESS_DISPATCH && !row.live_leaf;
        match self {
            TreeFilter::All => true,
            TreeFilter::UserOnly => row.kind == TreeRowKind::User,
            TreeFilter::LabeledOnly => row.label.is_some(),
            TreeFilter::NoTools => !textless_dispatch && row.kind != TreeRowKind::Tool,
            TreeFilter::Default => !textless_dispatch && row.kind != TreeRowKind::Tool,
        }
    }
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

/// One extension-hub row (#227): an installed extension or a catalog
/// entry, with its provenance attached. Built by the host; the hub
/// only filters, groups, and selects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HubRow {
    /// Extension name.
    pub name: String,
    /// Release version.
    pub version: String,
    /// Capability names for the badge row.
    pub capabilities: Vec<String>,
    /// One-line description.
    pub description: String,
    /// Where the component comes from (OCI ref or archive URL).
    /// Empty for installed rows.
    pub oci_ref: String,
    /// Catalog display name (`Official (misaalanshori/lca)`, ...).
    /// Empty for installed rows.
    pub source_name: String,
    /// The catalog document URL (consent shows the exact origin).
    /// Empty for installed rows.
    pub source_url: String,
    /// True for the in-repo official catalog.
    pub official: bool,
    /// Already installed (the INSTALLED section).
    pub installed: bool,
}

/// The `/model` picker (R9): a searchable list of the provider's models,
/// pi's `model-selector.ts` shape (minus the catalog refresh, which the
/// host does at startup).
pub struct ModelPicker {
    /// Every offered row, with the provider its search needs (gh #396).
    /// Selection returns the id; rendering shows the label (G2: labels are
    /// display-only - which is why the label could not be searched as the
    /// identity).
    pub models: Vec<crate::chat_model_search::ModelEntry>,
    /// The typed search query.
    pub query: String,
    /// The indices into `models` that match.
    pub matches: Vec<usize>,
    /// The highlighted row (an index into `matches`).
    pub selected: usize,
    /// A background discovery is still running (gh #232): the body
    /// shows a loading line until the rows land.
    pub loading: bool,
    /// The spinner frame's clock (bumped by `Chat::tick` on its 80 ms
    /// cadence while `loading` holds).
    pub loading_advanced: std::time::Instant,
    /// The visible spinner frame.
    pub loading_frame: usize,
}

impl ModelPicker {
    /// Open the picker over `(id, label, provider)` rows (gh #396).
    pub fn new(models: Vec<crate::state::ModelRow>) -> ModelPicker {
        let matches = (0..models.len()).collect();
        ModelPicker {
            models,
            query: String::new(),
            matches,
            selected: 0,
            loading: false,
            loading_advanced: std::time::Instant::now(),
            loading_frame: 0,
        }
    }

    /// Advance the loading spinner (gh #232): one frame per 80 ms
    /// cadence while a background discovery runs. Returns `true` when
    /// the frame moved, so the loop knows to repaint.
    pub fn tick(&mut self) -> bool {
        if !self.loading || self.loading_advanced.elapsed() < crate::separator::FRAME_MS {
            return false;
        }
        self.loading_frame += 1;
        self.loading_advanced = std::time::Instant::now();
        true
    }

    /// Re-filter after a query change, keeping the selection in range.
    ///
    /// Gh #396: the match is over the model's **identity** (provider, `provider/id`,
    /// bare id) and the result is **scored**, not merely filtered. The old
    /// `label.contains(query)` could not tell `openai/gpt-5` from
    /// `openrouter/openai/gpt-5` - the proxy's label contains the direct entry's
    /// whole name - and it could not see a transposition.
    pub fn refilter(&mut self) {
        self.matches = crate::chat_model_search::rank(&self.models, &self.query);
        if self.selected >= self.matches.len() {
            self.selected = self.matches.len().saturating_sub(1);
        }
    }

    /// Apply the defined unfiltered order (current, then default, then
    /// alphabetical) and clear the query (gh #396 rule 4).
    ///
    /// Called when the picker opens and when a query is cleared, so the order a
    /// reader sees is defined rather than "whatever order the catalog arrived in".
    pub fn reset_order(&mut self, current: Option<&str>, default: Option<&str>) {
        let ordered = crate::chat_model_search::default_order(&self.models, current, default);
        let mut reordered: Vec<crate::chat_model_search::ModelEntry> =
            Vec::with_capacity(ordered.len());
        for index in &ordered {
            if let Some(entry) = self.models.get(*index) {
                reordered.push(entry.clone());
            }
        }
        self.models = reordered;
        self.query.clear();
        self.refilter();
        // The current model leads the unfiltered list, so its row is the start.
        self.selected = 0;
    }

    /// The highlighted row's raw id, when any: what selection passes on.
    pub fn selected_model(&self) -> Option<&str> {
        self.matches
            .get(self.selected)
            .and_then(|index| self.models.get(*index))
            .map(|entry| entry.id.as_str())
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
        if !self.query.is_empty() {
            // The search line and its blank (gh #272 parity pass): the
            // painter shows them only while a query is set.
            rows.push(None);
            rows.push(None);
        }
        let mut section = "";
        for index in self.visible() {
            let row = &self.rows[index];
            if !row.section.is_empty() && row.section != section {
                section = row.section.as_str();
                // Divider plus its padding blank (gh #235): both
                // chrome, mirroring the painter one row at a time.
                rows.push(None);
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
    /// Type-to-search over the rows (pi's `ConfigSelector` filter, gh
    /// #272 parity pass). Empty means every row shows. Matched against
    /// the key, the label, and the description, case-insensitively; the
    /// selection indexes `visible`, never `rows`, so filtering can never
    /// select a row that is not on screen.
    pub query: String,
    /// Gh #397: the custom values the cycle has passed through, by key.
    ///
    /// This is the "remembered slot" the round trip needs. The list alone cannot
    /// carry it: a `default` stop **removes the key**, so the next read of the
    /// rows finds no value at all and the user's hand-set `4` is unreachable
    /// again - the cycle would run `4 → 5 → 10 → default → 1 → 2 → 3 → 5` and
    /// never come back. Remembering the custom value for as long as the picker is
    /// open keeps it a stop *across* the removal, which is what makes
    /// `custom → default → a → b → custom` close.
    ///
    /// Cleared when the picker closes, because its whole meaning is "a value this
    /// visit passed through": persisting it across sessions would offer a stale
    /// number the file no longer holds.
    pub remembered: std::collections::BTreeMap<String, String>,
}

impl SettingsPicker {
    /// This build's answer to pi's `SettingsListOptions::enableSearch`. pi's own
    /// `/settings` passes `{ enableSearch: true }` (`settings-selector.ts:985`), and
    /// so does LCA's list - the type-to-search line has been part of it since gh
    /// #272. Named rather than written as a bare `true` at the call site so the
    /// `Space` precedence reads as a rule with a switch rather than as a constant.
    pub const SEARCH_ENABLED: bool = true;

    /// The row indices the query admits, in display order. An empty query
    /// admits everything, so the unfiltered list is the same code path.
    pub fn visible(&self) -> Vec<usize> {
        if self.query.is_empty() {
            return (0..self.rows.len()).collect();
        }
        let needle = self.query.to_lowercase();
        self.rows
            .iter()
            .enumerate()
            .filter(|(_, row)| {
                row.key.to_lowercase().contains(&needle)
                    || row.label.to_lowercase().contains(&needle)
                    || row.description.to_lowercase().contains(&needle)
            })
            .map(|(index, _)| index)
            .collect()
    }

    /// The row the cursor is on, if any.
    pub fn selected_row(&self) -> Option<&crate::state::SettingRow> {
        self.visible()
            .get(self.selected)
            .and_then(|index| self.rows.get(*index))
    }

    /// Keep the cursor inside the filtered list after the query changed.
    pub fn clamp_selection(&mut self) {
        let len = self.visible().len();
        self.selected = self.selected.min(len.saturating_sub(1));
    }

    /// Whether a `Space` keystroke **activates** the highlighted row rather than
    /// being typed into the search box (gh #386 gap 3).
    ///
    /// pi's rule, verbatim from `settings-list.ts::handleInput`:
    ///
    /// ```text
    /// (data === " " && (!this.searchEnabled || this.searchInput?.getValue().length === 0))
    /// ```
    ///
    /// The whole rule is a **precedence**: `Space` toggles only when there is
    /// nothing to type into. A space is part of a search term once a search term
    /// exists; before that it has no term to belong to, so the row's value is what
    /// the reader meant.
    pub fn space_activates(query: &str, search_enabled: bool) -> bool {
        !search_enabled || query.is_empty()
    }

    /// Whether a `Space` keystroke activates the highlighted row, for this picker.
    ///
    /// The call site's form of [`Self::space_activates`]: the rule is written once,
    /// above.
    pub fn space_activates_row(&self) -> bool {
        Self::space_activates(&self.query, Self::SEARCH_ENABLED)
    }
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

/// One step of a [`SteppedWizard`] (gh #397): a key, the value it starts at, and
/// the values it may take.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WizardStep {
    /// The configuration key this step writes.
    pub key: String,
    /// The step's paint label ("Model", "Thinking level").
    pub label: String,
    /// The value the step starts at - the applied one when the wizard opens.
    pub value: String,
    /// The values the step offers. Empty means the caller derives them from an
    /// earlier pending choice ([`SteppedWizard::set_step_values`]).
    pub values: Vec<String>,
}

/// A stepped settings wizard (gh #397): a related group walked as ordered steps.
///
/// The alternative - toggling each key one at a time in the flat list - cannot
/// express a group whose later options depend on an earlier answer (a model's
/// allowed thinking levels), and it cannot offer atomicity: every key press lands.
///
/// # Atomicity is the whole contract
///
/// Choices accumulate in `pending` and **nothing is written** until [`commit`].
/// So `Esc` at the last step applies nothing at all - not "the steps you already
/// confirmed", which would leave a half-finished configuration the user cannot
/// recognise as half-finished. Every step is therefore a proposal, and the
/// wizard's only power is to make the set of proposals visible before they land.
///
/// A later step reads `pending` ([`pending_value`]), never the applied value: the
/// applied value describes what the session *was* on, and computing an option
/// list from it would offer the levels of a model the user has already stepped
/// away from.
///
/// [`commit`]: SteppedWizard::commit
/// [`pending_value`]: SteppedWizard::pending_value
pub struct SteppedWizard {
    /// Which row of `/settings` opened it, so closing restores the cursor there
    /// (pi's parent-restore: a submenu must not dump the reader at the top).
    pub opener_key: String,
    /// The steps, in the order they are walked.
    pub steps: Vec<WizardStep>,
    /// The step being edited.
    pub current: usize,
    /// Choices made so far, by key. The wizard's only state that could be
    /// written - and it is not written until [`commit`](SteppedWizard::commit).
    pub pending: std::collections::BTreeMap<String, String>,
}

impl SteppedWizard {
    /// A wizard over `steps`, opened from `opener_key`, on the first step.
    pub fn new(opener_key: &str, steps: Vec<WizardStep>) -> SteppedWizard {
        SteppedWizard {
            opener_key: opener_key.to_string(),
            steps,
            current: 0,
            pending: std::collections::BTreeMap::new(),
        }
    }

    /// The step being edited, if the wizard has any.
    pub fn current_step(&self) -> Option<&WizardStep> {
        self.steps.get(self.current)
    }

    /// The step being edited, as the tests and the painter name it.
    pub fn current(&self) -> Option<&WizardStep> {
        self.current_step()
    }

    /// Move to the next step. Past the last step there is nothing to walk to, so
    /// the caller commits instead; this saturates rather than wrapping, because a
    /// wizard that silently returned to step one would read as "start over".
    pub fn step_forward(&mut self) {
        self.current = (self.current + 1).min(self.steps.len().saturating_sub(1));
    }

    /// Back one step: the settings picker's existing `left` binding, not a new
    /// one (gh #397 design point 5).
    pub fn step_back(&mut self) {
        self.current = self.current.saturating_sub(1);
    }

    /// Record a choice for the current step. **Pending only** - see the type's
    /// doc for why nothing is written here.
    pub fn choose(&mut self, value: &str) {
        if let Some(step) = self.steps.get(self.current) {
            self.pending.insert(step.key.clone(), value.to_string());
        }
    }

    /// The value chosen so far for `key`: the pending choice when there is one,
    /// else the step's starting value.
    ///
    /// This is what a later step must compute its options from. Reading the
    /// *applied* value instead is the defect the type exists to prevent: on
    /// cancel the session would be left describing a model it is not on.
    pub fn pending_value(&self, key: &str) -> Option<&str> {
        self.pending.get(key).map(String::as_str).or_else(|| {
            self.steps
                .iter()
                .find(|step| step.key == key)
                .map(|step| step.value.as_str())
        })
    }

    /// Fill a step's option list once an earlier pending choice is known (a
    /// model's supported thinking levels).
    pub fn set_step_values(&mut self, index: usize, values: &[&str]) {
        if let Some(step) = self.steps.get_mut(index) {
            step.values = values.iter().map(|value| (*value).to_string()).collect();
        }
    }

    /// Cancel the whole wizard: the pending choices go with it, and the caller
    /// writes nothing. Returns the opener key so the picker can put the cursor
    /// back on the row that opened it.
    pub fn cancel(&mut self) -> Option<String> {
        let had_pending = !self.pending.is_empty();
        self.pending.clear();
        had_pending.then(|| self.opener_key.clone())
    }

    /// The same cancel, always reporting - for a caller whose flow expects a
    /// return value even when no choice was made yet.
    pub fn take_cancel(&mut self) -> Option<String> {
        self.pending.clear();
        Some(self.opener_key.clone())
    }

    /// Finish: the `(key, value)` pairs to persist, in walk order.
    ///
    /// Only a step the user actually moved through contributes; a step left at
    /// its starting value is not a change, and writing it would be a no-op that
    /// still churns the file (and, for a `default` stop, could remove a key the
    /// user meant to keep).
    pub fn commit(&self) -> Vec<(String, Option<String>)> {
        self.steps
            .iter()
            .filter_map(|step| {
                let chosen = self.pending.get(&step.key)?;
                Some((step.key.clone(), Some(chosen.clone())))
            })
            .collect()
    }

    /// The hook the wizard commits through (gh #397 design point 3): the same
    /// `persist_setting` `/settings` uses, so there is one writer. Named as a
    /// string rather than held as a function so the wizard cannot accidentally
    /// grow its own config handle.
    pub fn persist_through(&self) -> &'static str {
        "persist_setting"
    }

    /// The hint row: existing keys only (gh #397 design point 5).
    pub fn hint(&self) -> String {
        format!(
            "↑↓ choose · enter next · left back · esc cancel · step {}/{}",
            self.current + 1,
            self.steps.len().max(1)
        )
    }

    /// The overlay's `(title, body)`: every step with its pending value, and the
    /// current one marked.
    pub fn body(&self) -> Vec<String> {
        let mut rows = vec![String::new()];
        for (index, step) in self.steps.iter().enumerate() {
            let mark = if index == self.current { '>' } else { ' ' };
            let shown = self
                .pending
                .get(&step.key)
                .map_or_else(|| step.value.clone(), Clone::clone);
            // Gh #397: an empty value must say *why* it is empty rather than
            // printing a blank after the colon. A provider that declares no
            // thinking levels is a real state, and a blank reads as a rendering
            // fault (found in a live pane with a model that supports none).
            let shown = if shown.is_empty() {
                "(nothing offered for this model)".to_string()
            } else {
                shown
            };
            rows.push(format!(" {mark} {}: {shown}", step.label));
        }
        rows.push(String::new());
        rows.push(format!(
            " Step {}/{}",
            self.current + 1,
            self.steps.len().max(1)
        ));
        rows
    }
}
