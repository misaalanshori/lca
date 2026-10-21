//! In-file branch machinery (gh #37): entry-tree rows, tip-anchored
//! appends, branch navigation with summaries, and bookmarks. Split
//! from `store.rs` under the workspace's 1,200-line file ceiling.

use super::store::{Session, SessionStore};
use super::view::ViewMode;
use super::{Result, ids};

/// The record kind behind an entry row (gh #231): the DAG
/// navigator filters and marks rows by this, never by sniffing text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    /// A user prompt.
    User,
    /// An assistant reply (textless ones are pure tool calls).
    Assistant,
    /// A tool call or result.
    Tool,
    /// A branch summary landmark.
    Summary,
    /// A compaction landmark.
    Compaction,
    /// Extension state (gh #137): navigable, never model content.
    Custom,
    /// An extension context injection (gh #137).
    CustomMessage,
}

/// One navigable row of a session's entry tree (gh #37, FR-UI-16).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryRow {
    /// The record navigating here branches at.
    pub id: String,
    /// Nesting depth (the picker indents two spaces per level).
    pub depth: usize,
    /// The record kind (gh #231).
    pub kind: EntryKind,
    /// One-line row text, bare of kind markers (the navigator paints
    /// those; gh #231).
    pub text: String,
    /// The argument summary of the tool call a `Tool` row answers
    /// (gh #394 fix 3): `[read: src/main.rs:1-50]` instead of the opaque
    /// result head. `None` on every other kind, and on a result whose call
    /// is not in this log.
    pub call: Option<String>,
    /// The live bookmark on this record, when one names it.
    pub label: Option<String>,
    /// Whether the record is on the live chain.
    pub live: bool,
    /// Whether the record is the live chain's tip - the leaf the
    /// navigator's cursor opens on (gh #394 fix 3, pi's `currentLeafId`).
    pub live_leaf: bool,
}

/// One tool call rendered the way pi's `formatToolCall` renders it
/// (gh #394 fix 3): the argument that identifies the call, shortened to a
/// home-relative path where one applies, capped so a row stays one line.
fn tool_call_label(name: &str, arguments: &str) -> String {
    /// pi caps a shell command's rendering at this many cells.
    const COMMAND_CAP: usize = 50;
    /// The catch-all rendering's JSON cap (pi's `slice(0, 40)`).
    const ARGS_CAP: usize = 40;
    fn shorten(path: &str) -> String {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .ok();
        match home {
            Some(home) if !home.is_empty() && path.starts_with(&home) => {
                format!("~{}", &path[home.len()..])
            }
            _ => path.to_string(),
        }
    }
    fn one_line(text: &str) -> String {
        text.replace(['\n', '\t'], " ").trim().to_string()
    }
    let args: serde_json::Value = serde_json::from_str(arguments).unwrap_or_default();
    let string = |key: &str| -> Option<String> {
        args.get(key)
            .and_then(|value| match value {
                serde_json::Value::String(text) => Some(text.clone()),
                serde_json::Value::Number(number) => Some(number.to_string()),
                _ => None,
            })
            .map(|text| one_line(&text))
    };
    let path = || {
        string("path")
            .map(|path| shorten(&path))
            .unwrap_or_default()
    };
    match name {
        "read" => match (string("offset"), string("limit")) {
            (None, None) => format!("[read: {}]", path()),
            (offset, limit) => {
                let start = offset.unwrap_or_else(|| "1".to_string());
                match limit {
                    Some(limit) => {
                        let start_number: u64 = start.parse().unwrap_or(1);
                        let limit_number: u64 = limit.parse().unwrap_or(0);
                        let end = start_number.saturating_add(limit_number.saturating_sub(1));
                        format!("[read: {}:{start}-{end}]", path())
                    }
                    None => format!("[read: {}:{start}-]", path()),
                }
            }
        },
        // `list` is also callable as `ls`; `glob` as `find` (specs.rs).
        "list" | "ls" => format!("[list: {}]", path()),
        "write" => format!("[write: {}]", path()),
        "edit" => format!("[edit: {}]", path()),
        "glob" | "find" => format!(
            "[glob: {} in {}]",
            string("pattern").unwrap_or_default(),
            path()
        ),
        "grep" => format!(
            "[grep: /{}/ in {}]",
            string("pattern").unwrap_or_default(),
            path()
        ),
        "shell" => {
            let command = string("command").unwrap_or_default();
            let short: String = command.chars().take(COMMAND_CAP).collect();
            let ellipsis = if command.chars().count() > COMMAND_CAP {
                "..."
            } else {
                ""
            };
            format!("[shell: {short}{ellipsis}]")
        }
        other => {
            let rendered = one_line(arguments);
            let short: String = rendered.chars().take(ARGS_CAP).collect();
            if short.is_empty() {
                format!("[{other}]")
            } else {
                format!("[{other}: {short}]")
            }
        }
    }
}

/// Build entry rows from resolved audit records (gh #37): candidates
/// attach to their nearest candidate ancestor through jump markers
/// and non-row records; orphans root; children sort oldest-first;
/// emission is pre-order. The live set and the label map overlay.
fn entry_rows(
    records: &[lca_protocol::Record],
    live: &std::collections::BTreeSet<String>,
    live_leaf: Option<&str>,
    marks: &std::collections::BTreeMap<String, String>,
) -> Vec<EntryRow> {
    use std::collections::BTreeMap;
    fn row_kind_text(record: &lca_protocol::Record) -> Option<(EntryKind, String)> {
        let head = |text: &str| text.lines().next().unwrap_or_default().to_string();
        match record {
            lca_protocol::Record::User { content, .. } => Some((EntryKind::User, head(content))),
            lca_protocol::Record::Assistant { content, .. } => {
                let text = content.iter().find_map(|block| match block {
                    lca_protocol::ContentBlock::Text { text } => Some(text.clone()),
                    _ => None,
                });
                Some((
                    EntryKind::Assistant,
                    text.map(|t| head(&t))
                        .unwrap_or_else(|| "(tool call)".to_string()),
                ))
            }
            lca_protocol::Record::ToolCall { name, .. } => Some((EntryKind::Tool, name.clone())),
            lca_protocol::Record::ToolResult { content, .. } => Some((
                EntryKind::Tool,
                content
                    .as_deref()
                    .map(head)
                    .unwrap_or_else(|| "(no output)".to_string()),
            )),
            lca_protocol::Record::BranchSummary { summary, .. } => {
                Some((EntryKind::Summary, head(summary)))
            }
            lca_protocol::Record::Compaction { summary, .. } => {
                Some((EntryKind::Compaction, head(summary)))
            }
            lca_protocol::Record::Custom { custom_type, .. } => {
                Some((EntryKind::Custom, custom_type.clone()))
            }
            lca_protocol::Record::CustomMessage { content, .. } => {
                Some((EntryKind::CustomMessage, head(content)))
            }
            _ => None,
        }
    }
    fn row_text(record: &lca_protocol::Record) -> Option<String> {
        row_kind_text(record).map(|(_, text)| text)
    }
    // Nearest candidate ancestor through anything the picker skips.
    let by_id: BTreeMap<&str, &lca_protocol::Record> = records
        .iter()
        .filter_map(|r| r.id().map(|id| (id, r)))
        .collect();
    fn tree_parent<'a>(
        record: &'a lca_protocol::Record,
        by_id: &BTreeMap<&str, &'a lca_protocol::Record>,
    ) -> Option<&'a str> {
        let mut next = record.parent();
        let mut hops = 0;
        while let Some(id) = next {
            hops += 1;
            if hops > by_id.len() + 1 {
                return None; // link cycle: root it rather than spin
            }
            let parent = by_id.get(id)?;
            match parent {
                lca_protocol::Record::BranchPoint { target_id, .. } => {
                    next = Some(target_id);
                }
                _ if row_text(parent).is_some() => return Some(id),
                _ => next = parent.parent(),
            }
        }
        None
    }
    // Gh #394 fix 3: a `toolResult` names only `call_id` and no arguments,
    // so the arguments live in the assistant's `ToolCall` block. Index them
    // by call id and render a pi-shaped summary, so a row reads
    // `[read: src/main.rs:1-50]` rather than an opaque id.
    let calls: BTreeMap<&str, (&str, &str)> = records
        .iter()
        .filter_map(|record| match record {
            lca_protocol::Record::ToolCall {
                call_id,
                name,
                arguments,
                ..
            } => Some((call_id.as_str(), (name.as_str(), arguments.as_str()))),
            _ => None,
        })
        .collect();
    let mut children: BTreeMap<Option<&str>, Vec<&lca_protocol::Record>> = BTreeMap::new();
    let mut order: BTreeMap<&str, usize> = BTreeMap::new();
    for (index, record) in records.iter().enumerate() {
        if row_text(record).is_none() {
            continue;
        }
        let id = record.id().unwrap_or_default();
        order.insert(id, index);
        children
            .entry(tree_parent(record, &by_id))
            .or_default()
            .push(record);
    }
    for siblings in children.values_mut() {
        siblings.sort_by_key(|r| order.get(r.id().unwrap_or_default()));
    }
    let mut rows = Vec::new();
    #[allow(clippy::too_many_arguments)] // the walk's carried state, one arg each
    fn emit(
        parent: Option<&str>,
        depth: usize,
        children: &BTreeMap<Option<&str>, Vec<&lca_protocol::Record>>,
        live: &std::collections::BTreeSet<String>,
        live_leaf: Option<&str>,
        marks: &std::collections::BTreeMap<String, String>,
        calls: &BTreeMap<&str, (&str, &str)>,
        rows: &mut Vec<EntryRow>,
    ) {
        let Some(siblings) = children.get(&parent) else {
            return;
        };
        for record in siblings {
            let id = record.id().unwrap_or_default().to_string();
            let (kind, text) =
                row_kind_text(record).unwrap_or((EntryKind::Assistant, String::new()));
            let call = match record {
                lca_protocol::Record::ToolResult { call_id, .. } => calls
                    .get(call_id.as_str())
                    .map(|(name, arguments)| tool_call_label(name, arguments)),
                _ => None,
            };
            rows.push(EntryRow {
                id: id.clone(),
                depth,
                kind,
                text,
                call,
                label: marks.get(&id).cloned(),
                live: live.contains(&id),
                live_leaf: live_leaf == Some(id.as_str()),
            });
            emit(
                Some(record.id().unwrap_or_default()),
                depth + 1,
                children,
                live,
                live_leaf,
                marks,
                calls,
                rows,
            );
        }
    }
    emit(
        None, 0, &children, live, live_leaf, marks, &calls, &mut rows,
    );
    rows
}

impl SessionStore {
    /// One navigable row of the live session's entry tree (gh #37,
    /// FR-UI-16): the `/tree` picker shows these oldest-first,
    /// depth-indented, with labels and live-chain marks.
    ///
    /// Rows cover the whole file (every branch, like pi's tree), not
    /// the live chain: user and assistant messages, tool traffic, plus
    /// summary and compaction landmarks. Jump markers, label records,
    /// and session framing stay out (labels surface as row marks).
    pub fn entry_tree(&self, session: &Session) -> Result<Vec<EntryRow>> {
        let audit = self.resolved(session, ViewMode::Audit)?;
        let shown = self.resolved(session, ViewMode::Display).ok();
        let live: std::collections::BTreeSet<String> = shown
            .as_ref()
            .map(|shown| {
                shown
                    .records
                    .iter()
                    .filter_map(|r| r.id().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        // Gh #394 fix 3: the live *leaf* is the display view's last
        // record, the one the navigator's cursor opens on and the only
        // row no filter may hide (pi's `currentLeafId`).
        let live_leaf = shown
            .as_ref()
            .and_then(|shown| shown.records.iter().rev().find_map(|r| r.id()));
        let marks = self.labels(session).unwrap_or_default();
        Ok(entry_rows(&audit.records, &live, live_leaf, &marks))
    }

    /// Bookmark `record_id` as `label` (gh #37, ADR-0046): appends a
    /// `label` record to the live session's own log. `None` clears the
    /// bookmark. The target is resolved through the fork chain, so a
    /// fork can bookmark (and see) its ancestors' records (FR-SESS-10).
    pub fn set_label(&self, session: &Session, record_id: &str, label: Option<&str>) -> Result<()> {
        let resolved = self.resolved(session, ViewMode::Audit)?;
        if !resolved.records.iter().any(|r| r.id() == Some(record_id)) {
            return Err(crate::Error::LabelTargetMissing {
                session: session.id().to_string(),
                record: record_id.to_string(),
            });
        }
        let now = ids::now_ms();
        self.append(
            session,
            lca_protocol::Record::Label {
                v: lca_protocol::FORMAT_VERSION,
                ts: now,
                id: ids::record_id(now),
                parent: None,
                target_id: record_id.to_string(),
                label: label.map(str::to_string),
            },
        )
    }

    /// Every live bookmark as `record id -> label` (gh #37): walks the
    /// resolved history latest-wins, so a clear (`None`) erases an
    /// earlier name and a fork sees its ancestors' marks (FR-SESS-10).
    pub fn labels(&self, session: &Session) -> Result<std::collections::BTreeMap<String, String>> {
        let resolved = self.resolved(session, ViewMode::Audit)?;
        let mut marks = std::collections::BTreeMap::new();
        for record in &resolved.records {
            if let lca_protocol::Record::Label {
                target_id, label, ..
            } = record
            {
                match label {
                    Some(name) => {
                        marks.insert(target_id.clone(), name.clone());
                    }
                    None => {
                        marks.remove(target_id);
                    }
                }
            }
        }
        Ok(marks)
    }

    /// The record a bookmark name points at (gh #37): latest set wins
    /// across the resolved history; `None` when no live bookmark
    /// carries the name (FR-SESS-10).
    pub fn resolve_label(&self, session: &Session, name: &str) -> Result<Option<String>> {
        let resolved = self.resolved(session, ViewMode::Audit)?;
        // Latest set wins per name; a clear erases only names pointing
        // at its own target, so a sibling keeping the name resolves.
        let mut by_name = std::collections::BTreeMap::new();
        for record in &resolved.records {
            if let lca_protocol::Record::Label {
                target_id, label, ..
            } = record
            {
                match label {
                    Some(mark) => {
                        by_name.insert(mark.clone(), target_id.clone());
                    }
                    None => {
                        by_name.retain(|_, held| held != target_id);
                    }
                }
            }
        }
        Ok(by_name.remove(name))
    }
}
