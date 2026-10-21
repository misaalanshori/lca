//! The `/tree` DAG navigator: pi's `tree-selector.ts` ported onto LCA's
//! session rows (FR-UI-16, gh #231), including gh #394's four layout
//! rules - flat single-child chains, active trunk first, ancestor-anchored
//! filtering, and fold glyphs with anchor-aware horizontal pan.
//!
//! Split out of `chat_pickers.rs` under the workspace's 1,200-line file
//! ceiling (gh #394's brief: "split by concern and say where").

use lca_tui::engine::text::{
    slice_by_column, strip_terminal_sequences, truncate_to_width, visible_width,
};

use crate::chat_pickers::TreeFilter;

/// One painted span of a row: literal text in one theme role.
type Segment = (crate::theme::Role, String);

/// One visible row: the source index plus the recomputed visual
/// structure (gh #231, gh #394).
///
/// Hidden chains collapse, so the structure is computed over *visible*
/// rows only. `indent` is not the ancestor count: it grows only where a
/// visible parent has more than one visible child, plus one grouping
/// level beneath a fresh fork (pi's `flattenTree` rule). A linear session
/// therefore stays at column 0 however long it is - gh #394's bug was
/// one gutter per ancestor, which walked a 40-turn session off screen.
#[derive(Debug, Clone, PartialEq, Eq)]
struct VisibleRow {
    /// Index into the picker's full row list.
    index: usize,
    /// Visible parent's row index (`None` at the visible roots).
    vparent: Option<usize>,
    /// Display indent level; the prefix is `indent * 3` columns.
    indent: usize,
    /// `Some(is_last)` paints a connector at the indent's last level.
    connector: Option<bool>,
    /// Gutters by level: `(column, paints_bar)` (pi's `GutterInfo`).
    gutters: Vec<(usize, bool)>,
}

/// One item on the structure stack: the ten fields pi's `StackItem`
/// carries, named.
struct Node {
    /// Row index.
    index: usize,
    /// Indent level before the virtual-root shift.
    indent: usize,
    /// The parent branched, so a single child still groups one level.
    just_branched: bool,
    /// A connector belongs at this node's indent.
    show_connector: bool,
    /// The node is its parent's last child (`└─` rather than `├─`).
    is_last: bool,
    /// Gutters for this node's prefix.
    gutters: Vec<(usize, bool)>,
    /// A root under a virtual branching root: no connector of its own.
    virtual_root_child: bool,
}

/// The `/tree` DAG navigator (FR-UI-16, gh #231): pi's tree-selector
/// shape on S37C's rolling-window chrome - connectors, role markers,
/// fold/unfold, filter cycling, and label editing - plus gh #394's four
/// layout rules (flat single-child chains, active trunk first,
/// ancestor-anchored filtering, fold glyphs and anchor-aware pan).
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
    /// Row index to its full-tree parent, from the depth sequence. Kept
    /// whole so the ancestor walk never depends on the filter (gh #394).
    parent: Vec<Option<usize>>,
    /// Visible parent to its visible children, in paint order.
    children: std::collections::BTreeMap<Option<usize>, Vec<usize>>,
}

impl TreePicker {
    /// Open the navigator over structured rows under a filter.
    pub fn new(rows: Vec<crate::state::TreeRow>, filter: TreeFilter) -> TreePicker {
        let parent = Self::full_parents(&rows);
        let mut picker = TreePicker {
            rows,
            selected: 0,
            filter,
            folded: std::collections::BTreeSet::new(),
            editing: None,
            visible: Vec::new(),
            parent,
            children: std::collections::BTreeMap::new(),
        };
        picker.rebuild();
        // Gh #248: the cursor opens on the live leaf (pi's
        // `realLeafId` anchoring), never row 0 - the deepest live
        // row wins; no live row keeps the top.
        if let Some(at) = picker
            .visible
            .iter()
            .rposition(|shown| picker.rows[shown.index].live)
        {
            picker.selected = at;
        }
        picker
    }

    /// Open the navigator from a filter's config spelling (gh #394): the
    /// seam the regression suite and any host that holds only rows uses.
    pub fn from_filter_name(rows: Vec<crate::state::TreeRow>, filter: &str) -> TreePicker {
        Self::new(rows, TreeFilter::parse(filter))
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

    /// Switch filters (pi clears folds here). The selection rides its
    /// record id and, when the filter hides that record, lands on its
    /// nearest visible ancestor (gh #394 fix 3) - never on whatever row
    /// happens to sit at the old numeric index.
    pub fn set_filter(&mut self, filter: TreeFilter) {
        let id = self.selected_id();
        self.filter = filter;
        self.folded.clear();
        self.rebuild();
        self.restore(id);
    }

    /// Fold the selected row's subtree, or unfold it when already folded.
    /// Returns `false` when the row is not foldable and nothing changed.
    pub fn toggle_fold_at(&mut self, at: usize) -> bool {
        let Some(id) = self
            .visible
            .get(at)
            .map(|shown| self.rows[shown.index].id.clone())
        else {
            return false;
        };
        if !self.folded.remove(&id) {
            if !self.is_foldable_at_index(at) {
                return false;
            }
            self.folded.insert(id);
        }
        let current = self.selected_id();
        self.rebuild();
        self.restore(current);
        true
    }

    /// pi's `foldOrUp` (gh #394 fix 4): fold an expanded foldable row;
    /// otherwise jump to the start of this branch segment. Returns
    /// `false` when there is nowhere to go.
    pub fn fold_or_up(&mut self) -> bool {
        let Some(shown) = self.visible.get(self.selected) else {
            return false;
        };
        let foldable = self.is_foldable_at_index(self.selected);
        let folded = self.folded.contains(&self.rows[shown.index].id);
        if foldable && !folded {
            return self.toggle_fold_at(self.selected);
        }
        match self.segment_start_up() {
            Some(target) => {
                self.selected = target;
                true
            }
            None => false,
        }
    }

    /// pi's `unfoldOrDown` (gh #394 fix 4): unfold a folded row; otherwise
    /// jump to the next fork at or below the selection. Returns `false`
    /// when there is nowhere to go.
    pub fn unfold_or_down(&mut self) -> bool {
        let Some(shown) = self.visible.get(self.selected) else {
            return false;
        };
        if self.folded.contains(&self.rows[shown.index].id) {
            return self.toggle_fold_at(self.selected);
        }
        match self.segment_start_down() {
            Some(target) => {
                self.selected = target;
                true
            }
            None => false,
        }
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
        match self.visible.iter().position(|row| row.index == parent) {
            Some(at) => {
                self.selected = at;
                true
            }
            None => false,
        }
    }

    /// Move the selection to the selected row's first visible child.
    /// Returns `false` on a leaf.
    pub fn move_to_first_child(&mut self) -> bool {
        let Some(shown) = self.visible.get(self.selected).map(|row| row.index) else {
            return false;
        };
        match self
            .visible
            .iter()
            .position(|row| row.vparent == Some(shown))
        {
            Some(at) => {
                self.selected = at;
                true
            }
            None => false,
        }
    }

    /// Clamp the selection into the view (call after external moves).
    pub fn clamp_selected(&mut self) {
        self.selected = self.selected.min(self.visible.len().saturating_sub(1));
    }

    /// The painted body rows: prefix, connectors, fold glyphs, role
    /// markers, bookmark and live marks (gh #231, gh #248, gh #394).
    ///
    /// `width` is the columns available to a painted row (`usize::MAX`
    /// for unclipped, which the unit rows read). The caller's cursor
    /// gutter stays pinned and the prefix plus body pan left together,
    /// only when the selected row's anchor would leave under a third of
    /// the viewport for its text (pi's `renderHorizontalViewport`), then
    /// truncate - so no row is ever wider than the box it is drawn in and
    /// some connector context stays visible left of the text.
    /// `Theme::plain()` renders byte-identical to the old unstyled rows,
    /// so unit asserts read through `strip`.
    pub fn paint_rows(&self, theme: &crate::theme::Theme, width: usize) -> Vec<String> {
        let pan = self.pan_offset(width);
        self.visible
            .iter()
            .enumerate()
            .map(|(at, shown)| {
                let (prefix, body) = self.row_segments(theme, shown);
                let mut line = String::new();
                for (role, text) in prefix.iter().chain(body.iter()) {
                    line.push_str(&theme.role(*role)(text));
                }
                if width == usize::MAX {
                    // Unclipped: the editing buffer still rides the row.
                    if self.editing.is_some() && at == self.selected {
                        line.push_str(&format!(
                            " \u{2710} {}",
                            self.editing.as_deref().unwrap_or_default()
                        ));
                    }
                    return line;
                }
                let line = if pan > 0 {
                    slice_by_column(&line, pan, usize::MAX, true)
                } else {
                    line
                };
                let line = truncate_to_width(&line, width, "\u{2026}", false);
                let painted = visible_width(&strip_terminal_sequences(&line));
                let mut line = format!("{line}{}", " ".repeat(width.saturating_sub(painted)));
                if self.editing.is_some() && at == self.selected {
                    line.push_str(&format!(
                        " \u{2710} {}",
                        self.editing.as_deref().unwrap_or_default()
                    ));
                }
                line
            })
            .collect()
    }

    /// One row's prefix segments (the gutter, connector and markers) and
    /// its body segments (the entry text and its decorations).
    fn row_segments(
        &self,
        _theme: &crate::theme::Theme,
        shown: &VisibleRow,
    ) -> (Vec<Segment>, Vec<Segment>) {
        use crate::state::TreeRowKind;
        use crate::theme::Role;
        let row = &self.rows[shown.index];
        let folded = self.folded.contains(&row.id);
        let levels = shown.indent;
        let connector_level = shown.connector.map(|_| levels.saturating_sub(1));
        let mut cells: Vec<char> = vec![' '; levels * 3];
        for level in 0..levels {
            match shown.gutters.iter().find(|(at, _)| *at == level) {
                Some((_, open)) => {
                    cells[level * 3] = if *open { '\u{2502}' } else { ' ' };
                }
                None => {
                    if Some(level) == connector_level {
                        let last = shown.connector == Some(true);
                        cells[level * 3] = if last { '\u{2514}' } else { '\u{251c}' };
                        // Gh #394 fix 4: the fold glyph replaces the dash,
                        // so a fold costs no extra column.
                        cells[level * 3 + 1] = if folded {
                            '\u{229e}'
                        } else if self.is_foldable_at(shown.index) {
                            '\u{229f}'
                        } else {
                            '\u{2500}'
                        };
                    }
                }
            }
        }
        let mut prefix: Vec<Segment> = vec![(Role::Dim, cells.into_iter().collect())];
        // A folded row with no connector of its own carries `⊞` here
        // (pi's `foldMarker`), so the state is never invisible.
        if folded && shown.connector.is_none() {
            prefix.push((Role::Accent, "\u{229e} ".to_string()));
        }
        // pi's active-path marker sits between the connectors and the text.
        if row.live {
            prefix.push((Role::Accent, "\u{2022} ".to_string()));
        }
        let mut body: Vec<Segment> = Vec::new();
        let text = |segment: &str| (Role::Text, segment.to_string());
        match row.kind {
            TreeRowKind::User => {
                body.push((Role::Accent, "user: ".to_string()));
                body.push(text(&row.text));
            }
            TreeRowKind::Assistant => {
                body.push((Role::Success, "assistant: ".to_string()));
                body.push(text(&row.text));
            }
            TreeRowKind::Tool => {
                // Gh #394 fix 3: a result whose call is in the log reads as
                // the call (`[read: src/main.rs:1-50]`), which is what the
                // user is looking for; the result head backs it up on the
                // next line of the same row's collapsed card.
                body.push((Role::Muted, "tool: ".to_string()));
                match &row.call {
                    Some(call) => body.push((Role::Muted, call.clone())),
                    None => body.push((Role::Muted, row.text.clone())),
                }
            }
            TreeRowKind::Summary => {
                body.push((Role::Warning, "summary: ".to_string()));
                body.push(text(&row.text));
            }
            TreeRowKind::Compaction => {
                body.push((Role::BorderAccent, format!("compaction: {}", row.text)))
            }
            TreeRowKind::Custom => body.push((Role::Dim, format!("custom: {}", row.text))),
            TreeRowKind::CustomMessage => {
                body.push((Role::CustomMessageLabel, "message: ".to_string()));
                body.push(text(&row.text));
            }
        }
        if let Some(label) = &row.label {
            body.push((Role::Warning, format!(" [{label}]")));
        }
        if row.live {
            body.push(text(" \u{25c0}"));
        }
        (prefix, body)
    }

    /// The horizontal pan for a frame of `width` columns (gh #394 fix 4,
    /// pi's `renderHorizontalViewport`): zero unless the selected row's
    /// anchor would leave under a third of the viewport for its text, and
    /// otherwise enough to leave a quarter of the viewport of connector
    /// context to its left. `width == usize::MAX` never pans.
    fn pan_offset(&self, width: usize) -> usize {
        /// pi's `TREE_GUTTER_WIDTH`: the cursor column the box paints.
        const GUTTER_WIDTH: usize = 2;
        if width == usize::MAX {
            return 0;
        }
        let viewport = width.saturating_sub(GUTTER_WIDTH);
        if viewport == 0 {
            return 0;
        }
        let Some(shown) = self.visible.get(self.selected) else {
            return 0;
        };
        let anchor = self.anchor_col(shown);
        // pi clamps the guaranteed text to 4..20 columns, a third of the
        // viewport by preference.
        let min_visible = (viewport / 3).clamp(4, 20);
        if anchor <= viewport - min_visible {
            return 0;
        }
        let widest = self
            .visible
            .iter()
            .map(|shown| self.anchor_col(shown) + self.body_width(shown))
            .max()
            .unwrap_or(0);
        let max_scroll = widest.saturating_sub(viewport);
        let context = (viewport / 4).clamp(2, 12);
        anchor.saturating_sub(context).min(max_scroll)
    }

    /// The column where a row's entry text begins (pi's `anchorCol`): its
    /// prefix, including the fold and path markers.
    fn anchor_col(&self, shown: &VisibleRow) -> usize {
        let row = &self.rows[shown.index];
        let mut width = shown.indent * 3;
        if self.folded.contains(&row.id) && shown.connector.is_none() {
            width += 2;
        }
        if row.live {
            width += 2;
        }
        width
    }

    /// A row's body width, for the pan's `maxHorizontalScroll`.
    fn body_width(&self, shown: &VisibleRow) -> usize {
        let theme = crate::theme::Theme::plain();
        self.row_segments(&theme, shown)
            .1
            .iter()
            .map(|(_, text)| visible_width(text))
            .sum()
    }

    /// Rebuild the visible list from rows, filter, and folds.
    pub(crate) fn rebuild(&mut self) {
        let filter = self.filter;
        let folded = self.folded.clone();
        let rows = &self.rows;
        // Visible rows: the filter passes and no folded ancestor hides it.
        let mut hidden = vec![false; rows.len()];
        let mut shown: Vec<usize> = Vec::new();
        for (index, row) in rows.iter().enumerate() {
            let under_fold =
                self.parent[index].is_some_and(|up| folded.contains(&rows[up].id) || hidden[up]);
            hidden[index] = under_fold;
            if !under_fold && filter.passes(row) {
                shown.push(index);
            }
        }
        let in_view: std::collections::BTreeSet<usize> = shown.iter().copied().collect();
        // Nearest visible ancestor per row, then the visible tree.
        let mut visible_parent: Vec<Option<usize>> = vec![None; rows.len()];
        for index in &shown {
            let mut next = self.parent[*index];
            while next.is_some_and(|up| !in_view.contains(&up)) {
                next = self.parent[next.unwrap_or(usize::MAX)];
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
        // Gh #394 fix 2: the live trunk leads at every level. `sort_by_key`
        // is stable, so equal-priority siblings keep their stored order and
        // two frames of one session are identical.
        if shown.iter().any(|index| rows[*index].live) {
            let live = Self::subtree_live(rows, &children);
            for siblings in children.values_mut() {
                siblings.sort_by_key(|index| !live.get(index).copied().unwrap_or(false));
            }
        }
        let multiple_roots = children.get(&None).is_some_and(|roots| roots.len() > 1);
        // Depth-first in paint order. Children are pushed reversed so the
        // stack pops them left to right.
        let mut visible: Vec<VisibleRow> = Vec::with_capacity(shown.len());
        let mut stack: Vec<Node> = Vec::new();
        for index in children.get(&None).into_iter().flatten().rev() {
            stack.push(Node {
                index: *index,
                indent: usize::from(multiple_roots),
                just_branched: multiple_roots,
                show_connector: multiple_roots,
                is_last: true,
                gutters: Vec::new(),
                virtual_root_child: multiple_roots,
            });
        }
        if let Some(roots) = children.get(&None) {
            for (position, node) in stack.iter_mut().enumerate() {
                let forward = roots.len().saturating_sub(1).saturating_sub(position);
                node.is_last = forward + 1 == roots.len();
            }
        }
        while let Some(node) = stack.pop() {
            let kids = children.get(&Some(node.index));
            let multiple_children = kids.is_some_and(|kids| kids.len() > 1);
            let display_indent = if multiple_roots {
                node.indent.saturating_sub(1)
            } else {
                node.indent
            };
            let connector_shown = node.show_connector && !node.virtual_root_child;
            visible.push(VisibleRow {
                index: node.index,
                vparent: visible_parent[node.index],
                indent: display_indent,
                connector: connector_shown.then_some(node.is_last),
                gutters: node.gutters.clone(),
            });
            // pi's rule: branching children (and the first generation
            // beneath a fresh fork) shift one level; a single-child chain
            // stays flat.
            let grows = multiple_children || (node.just_branched && node.indent > 0);
            let child_indent = node.indent + usize::from(grows);
            let child_gutters = if connector_shown {
                let position = display_indent.saturating_sub(1);
                let mut gutters = node.gutters.clone();
                gutters.push((position, !node.is_last));
                gutters
            } else {
                node.gutters.clone()
            };
            let Some(kids) = kids else {
                continue;
            };
            for (position, kid) in kids.iter().enumerate().rev() {
                stack.push(Node {
                    index: *kid,
                    indent: child_indent,
                    just_branched: multiple_children,
                    show_connector: multiple_children,
                    is_last: position + 1 == kids.len(),
                    gutters: child_gutters.clone(),
                    virtual_root_child: false,
                });
            }
        }
        self.children = children;
        self.visible = visible;
    }

    /// Restore the selection onto a record id. When the filter hides it,
    /// walk to its nearest visible ancestor (gh #394 fix 3); with none,
    /// take the last visible row, as pi does.
    pub(crate) fn restore(&mut self, id: Option<String>) {
        if let Some(id) = id {
            if let Some(at) = self
                .visible
                .iter()
                .position(|shown| self.rows[shown.index].id == id)
            {
                self.selected = at;
                return;
            }
            let mut next = self
                .rows
                .iter()
                .position(|row| row.id == id)
                .and_then(|index| self.parent[index]);
            let mut hops = 0;
            while let Some(ancestor) = next {
                hops += 1;
                if hops > self.rows.len() + 1 {
                    break; // a link cycle: fall through rather than spin
                }
                if let Some(at) = self
                    .visible
                    .iter()
                    .position(|shown| shown.index == ancestor)
                {
                    self.selected = at;
                    return;
                }
                next = self.parent[ancestor];
            }
            self.selected = self.visible.len().saturating_sub(1);
            return;
        }
        self.clamp_selected();
    }

    /// Full-tree parents from the depth sequence: each row's parent is its
    /// nearest preceding row with a smaller depth.
    fn full_parents(rows: &[crate::state::TreeRow]) -> Vec<Option<usize>> {
        let mut parent: Vec<Option<usize>> = vec![None; rows.len()];
        let mut stack: Vec<usize> = Vec::new();
        for (index, row) in rows.iter().enumerate() {
            while stack
                .last()
                .is_some_and(|top| rows[*top].depth >= row.depth)
            {
                stack.pop();
            }
            parent[index] = stack.last().copied();
            stack.push(index);
        }
        parent
    }

    /// Mark every visible subtree containing the live leaf (pi's
    /// `containsActive`, over the visible tree).
    fn subtree_live(
        rows: &[crate::state::TreeRow],
        children: &std::collections::BTreeMap<Option<usize>, Vec<usize>>,
    ) -> std::collections::BTreeMap<usize, bool> {
        let mut live: std::collections::BTreeMap<usize, bool> = std::collections::BTreeMap::new();
        let mut stack: Vec<(usize, bool)> = children
            .get(&None)
            .into_iter()
            .flatten()
            .map(|index| (*index, false))
            .collect();
        while let Some((index, expanded)) = stack.pop() {
            if !expanded {
                stack.push((index, true));
                for kid in children.get(&Some(index)).into_iter().flatten() {
                    stack.push((*kid, false));
                }
                continue;
            }
            let below = children
                .get(&Some(index))
                .into_iter()
                .flatten()
                .any(|kid| live.get(kid).copied().unwrap_or(false));
            live.insert(index, rows[index].live || below);
        }
        live
    }

    /// Whether the visible row at `at` can be folded (pi's `isFoldable`):
    /// it has visible children and is either a root or a branch segment
    /// start, so folding never collapses a chain that has no fork in it.
    fn is_foldable_at_index(&self, at: usize) -> bool {
        self.visible
            .get(at)
            .is_some_and(|shown| self.is_foldable_at(shown.index))
    }

    /// [`TreePicker::is_foldable_at_index`] by row index.
    fn is_foldable_at(&self, index: usize) -> bool {
        let Some(kids) = self.children.get(&Some(index)) else {
            return false;
        };
        if kids.is_empty() {
            return false;
        }
        match self
            .visible
            .iter()
            .find(|shown| shown.index == index)
            .and_then(|shown| shown.vparent)
        {
            None => true,
            Some(parent) => self
                .children
                .get(&Some(parent))
                .is_some_and(|siblings| siblings.len() > 1),
        }
    }

    /// pi's `findBranchSegmentStart("up")`: walk up while each parent has a
    /// single visible child; the first branch start found *strictly above*
    /// the selection wins, and the walk continues to the root otherwise.
    fn segment_start_up(&self) -> Option<usize> {
        let mut cursor = self.selected;
        loop {
            let shown = self.visible.get(cursor)?;
            let Some(parent) = shown.vparent else {
                return Some(cursor);
            };
            if self
                .children
                .get(&Some(parent))
                .is_some_and(|siblings| siblings.len() > 1)
                && cursor < self.selected
            {
                return Some(cursor);
            }
            cursor = self
                .visible
                .iter()
                .position(|candidate| candidate.index == parent)?;
        }
    }

    /// pi's `findBranchSegmentStart("down")`: the next fork at or below the
    /// selection, following single children down.
    fn segment_start_down(&self) -> Option<usize> {
        let mut cursor = self.selected;
        loop {
            let shown = self.visible.get(cursor)?;
            let kids = self.children.get(&Some(shown.index))?;
            match kids.len() {
                0 => return None,
                1 => {
                    cursor = self
                        .visible
                        .iter()
                        .position(|candidate| candidate.index == kids[0])?;
                }
                _ => {
                    return self
                        .visible
                        .iter()
                        .position(|candidate| candidate.index == kids[0]);
                }
            }
        }
    }
}
