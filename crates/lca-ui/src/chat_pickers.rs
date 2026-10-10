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

    /// Whether a row shows under this filter (gh #231).
    fn passes(self, row: &crate::state::TreeRow) -> bool {
        use crate::state::TreeRowKind;
        match self {
            TreeFilter::All => true,
            TreeFilter::UserOnly => row.kind == TreeRowKind::User,
            TreeFilter::LabeledOnly => row.label.is_some(),
            TreeFilter::NoTools => {
                row.kind != TreeRowKind::Tool
                    && !(row.kind == TreeRowKind::Assistant && row.text == "(tool call)")
            }
            TreeFilter::Default => row.kind != TreeRowKind::Tool,
        }
    }
}

/// One visible row: the source index plus the recomputed visual
/// structure (gh #231). Hidden chains collapse, so `depth` counts
/// visible ancestors and connectors sit on the visible tree.
#[derive(Debug, Clone, PartialEq, Eq)]
struct VisibleRow {
    /// Index into the picker's full row list.
    index: usize,
    /// Visible parent's row index (`None` at the visible roots).
    vparent: Option<usize>,
    /// Visible ancestor count (the indent).
    depth: usize,
    /// `true` for `└─`, `false` for `├─`; `None` on single chains.
    last: Option<bool>,
    /// Per ancestor level: `true` paints `│`, `false` paints spaces.
    gutters: Vec<bool>,
}

/// The `/tree` DAG navigator (FR-UI-16, gh #231): pi's tree-selector
/// shape on S37C's rolling-window chrome - connectors, role markers,
/// fold/unfold, filter cycling, and label editing.
pub struct TreePicker {
    /// Every structured row, pre-order (the hook's order).
    pub rows: Vec<crate::state::TreeRow>,
    /// The highlighted row (an index into the visible list).
    pub selected: usize,
    /// The active filter (pi's `filterMode`).
    pub filter: TreeFilter,
    /// Folded record ids (their subtrees hide).
    pub folded: std::collections::BTreeSet<String>,
    /// The label input buffer while `e` edits (`None` navigates).
    pub editing: Option<String>,
    /// The visible rows, recomputed on every state change.
    visible: Vec<VisibleRow>,
}

impl TreePicker {
    /// Open the navigator over structured rows under a filter.
    pub fn new(rows: Vec<crate::state::TreeRow>, filter: TreeFilter) -> TreePicker {
        let mut picker = TreePicker {
            rows,
            selected: 0,
            filter,
            folded: std::collections::BTreeSet::new(),
            editing: None,
            visible: Vec::new(),
        };
        picker.rebuild();
        picker
    }

    /// The visible row count.
    pub fn visible_len(&self) -> usize {
        self.visible.len()
    }

    /// The visible row's bookmark, when one names it.
    pub fn visible_label(&self, at: usize) -> Option<String> {
        self.visible
            .get(at)
            .and_then(|shown| self.rows[shown.index].label.clone())
    }

    /// The selected record's id, when a row shows.
    pub fn selected_id(&self) -> Option<String> {
        self.visible
            .get(self.selected)
            .map(|shown| self.rows[shown.index].id.clone())
    }

    /// Switch filters (pi clears folds here): the selection rides its
    /// record id, else clamps to the view.
    pub fn set_filter(&mut self, filter: TreeFilter) {
        let id = self.selected_id();
        self.filter = filter;
        self.folded.clear();
        self.rebuild();
        self.restore(id);
    }

    /// Fold (or unfold) the selected row's subtree; returns `false`
    /// when the row has no visible children to fold.
    pub fn toggle_fold_at(&mut self, at: usize) -> bool {
        let Some(id) = self
            .visible
            .get(at)
            .map(|shown| self.rows[shown.index].id.clone())
        else {
            return false;
        };
        if !self.folded.remove(&id) {
            if !self.has_visible_children(&id) {
                return false;
            }
            self.folded.insert(id);
        }
        let current = self.selected_id();
        self.rebuild();
        self.restore(current);
        true
    }

    /// Move the selection to the selected row's visible parent.
    /// Returns `false` at a root.
    pub fn move_to_parent(&mut self) -> bool {
        let Some(parent) = self
            .visible
            .get(self.selected)
            .and_then(|shown| shown.vparent)
        else {
            return false;
        };
        if let Some(at) = self.visible.iter().position(|row| row.index == parent) {
            self.selected = at;
            return true;
        }
        false
    }

    /// Move the selection to the selected row's first visible child.
    /// Returns `false` on a leaf.
    pub fn move_to_first_child(&mut self) -> bool {
        let Some(shown) = self.visible.get(self.selected).map(|row| row.index) else {
            return false;
        };
        if let Some(at) = self
            .visible
            .iter()
            .position(|row| row.vparent == Some(shown))
        {
            self.selected = at;
            return true;
        }
        false
    }

    /// Clamp the selection into the view (call after external moves).
    pub fn clamp_selected(&mut self) {
        self.selected = self.selected.min(self.visible.len().saturating_sub(1));
    }

    /// The painted body rows: gutters, connectors, role markers,
    /// bookmark and live marks (gh #231).
    pub fn paint_rows(&self) -> Vec<String> {
        use crate::state::TreeRowKind;
        self.visible
            .iter()
            .map(|shown| {
                let row = &self.rows[shown.index];
                let mut line = String::new();
                for gutter in &shown.gutters {
                    line.push_str(if *gutter { "\u{2502} " } else { "  " });
                }
                match shown.last {
                    Some(true) => line.push_str("\u{2514}\u{2500} "),
                    Some(false) => line.push_str("\u{251c}\u{2500} "),
                    None => {
                        if shown.depth > 0 {
                            line.push_str("  ");
                        }
                    }
                }
                let marker = match row.kind {
                    TreeRowKind::User => "user: ",
                    TreeRowKind::Assistant => "assistant: ",
                    TreeRowKind::Tool => "tool: ",
                    TreeRowKind::Summary => "summary: ",
                    TreeRowKind::Compaction => "compaction: ",
                };
                line.push_str(marker);
                line.push_str(&row.text);
                if let Some(label) = &row.label {
                    line.push_str(&format!(" [{label}]"));
                }
                if row.live {
                    line.push_str(" \u{25c0}");
                }
                if self.editing.is_some() && self.visible.get(self.selected) == Some(shown) {
                    line.push_str(&format!(
                        " \u{2710} {}",
                        self.editing.as_deref().unwrap_or_default()
                    ));
                }
                line
            })
            .collect()
    }

    /// Rebuild the visible list from rows, filter, and folds.
    pub(crate) fn rebuild(&mut self) {
        // Full-tree parents from the depth sequence: each row's parent
        // is its nearest preceding row with a smaller depth.
        let mut parent: Vec<Option<usize>> = vec![None; self.rows.len()];
        let mut stack: Vec<usize> = Vec::new();
        for (index, row) in self.rows.iter().enumerate() {
            while stack
                .last()
                .is_some_and(|top| self.rows[*top].depth >= row.depth)
            {
                stack.pop();
            }
            parent[index] = stack.last().copied();
            stack.push(index);
        }
        // Visible rows: filter passes and no folded ancestor.
        let mut hidden = vec![false; self.rows.len()];
        let mut shown: Vec<usize> = Vec::new();
        for (index, row) in self.rows.iter().enumerate() {
            let under_fold = parent[index]
                .is_some_and(|up| self.folded.contains(&self.rows[up].id) || hidden[up]);
            hidden[index] = under_fold;
            if !under_fold && self.filter.passes(row) {
                shown.push(index);
            }
        }
        // Visible structure: nearest visible ancestor + siblings.
        let in_view: std::collections::BTreeSet<usize> = shown.iter().copied().collect();
        let mut visible_parent: Vec<Option<usize>> = vec![None; self.rows.len()];
        for index in &shown {
            let mut next = parent[*index];
            while next.is_some_and(|up| !in_view.contains(&up)) {
                next = parent[next.unwrap_or(usize::MAX)];
            }
            visible_parent[*index] = next;
        }
        let mut children: std::collections::BTreeMap<Option<usize>, Vec<usize>> =
            std::collections::BTreeMap::new();
        for index in &shown {
            children
                .entry(visible_parent[*index])
                .or_default()
                .push(*index);
        }
        self.visible = shown
            .iter()
            .map(|index| {
                let mut chain: Vec<usize> = Vec::new();
                let mut next = visible_parent[*index];
                while let Some(up) = next {
                    chain.push(up);
                    next = visible_parent[up];
                }
                chain.reverse();
                let depth = chain.len();
                let gutters = chain
                    .iter()
                    .map(|ancestor| {
                        let siblings = &children[&visible_parent[*ancestor]];
                        siblings.last() != Some(ancestor)
                    })
                    .collect();
                let siblings = &children[&visible_parent[*index]];
                let last = (siblings.len() > 1).then(|| siblings.last() == Some(index));
                VisibleRow {
                    index: *index,
                    vparent: visible_parent[*index],
                    depth,
                    last,
                    gutters,
                }
            })
            .collect();
    }

    /// Restore the selection onto a record id, else clamp.
    pub(crate) fn restore(&mut self, id: Option<String>) {
        if let Some(id) = id
            && let Some(at) = self
                .visible
                .iter()
                .position(|shown| self.rows[shown.index].id == id)
        {
            self.selected = at;
            return;
        }
        self.clamp_selected();
    }

    /// Whether a record id has visible children (foldable).
    fn has_visible_children(&self, id: &str) -> bool {
        let Some(at) = self.rows.iter().position(|row| row.id == id) else {
            return false;
        };
        self.visible.iter().any(|shown| shown.vparent == Some(at))
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
    /// Open the picker over a list of `(id, label)` rows.
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
