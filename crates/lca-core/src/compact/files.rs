//! File tracking and the system-prompt checkpoint (`docs/compaction.md` phase 3):
//! the cumulative read/modified lists a compaction carries and records over, and
//! the detection of a system prompt that moved since the last checkpoint.
//!
//! Both are bounded (`MAX_TRACKED_FILES`) and both append-only: a checkpoint is
//! a record, never a rewrite.

use lca_protocol::{FORMAT_VERSION, Record};

/// Files per list a compaction record carries (gh #36 phase 3):
/// cumulative tracking stays bounded storage, sorted and truncated.
/// Pi tracks unbounded sets; the cap is the documented divergence.
pub const MAX_TRACKED_FILES: usize = 200;

/// The `custom` record type noting a system-prompt change across
/// compactions (gh #36 phase 3): the detection, not a migration.
pub const SYSTEM_PROMPT_CHANGE_TYPE: &str = "system-prompt-change";

/// What the caller chose for this compaction (gh #36 phase 3): the
/// kept anchor (`None` compacts everything and anchors the record's
/// own id, pi's retain-none shape), the accumulated file lists, and
/// the system-prompt checkpoint (`None` when the caller never knew
/// the prompt).
pub(crate) struct CompactMeta {
    pub(crate) first_kept_id: Option<String>,
    pub(crate) read_files: Vec<String>,
    pub(crate) modified_files: Vec<String>,
    pub(crate) system_prompt: Option<String>,
}

/// Cumulative file lists for a compaction (gh #36 phase 3, pi's
/// `CompactionDetails`): tool calls in the candidate plus the file
/// lists of earlier compactions in scope, read-only files sorted
/// first, each list capped at [`MAX_TRACKED_FILES`]. A file both read
/// and modified counts as modified, pi's rule.
pub(crate) fn accumulate_files(
    candidate: &[Record],
    priors: &[Record],
) -> (Vec<String>, Vec<String>) {
    use std::collections::BTreeSet;
    let mut read: BTreeSet<String> = BTreeSet::new();
    let mut modified: BTreeSet<String> = BTreeSet::new();
    let mut file_op = |name: &str, arguments: &str| {
        let path = serde_json::from_str::<serde_json::Value>(arguments)
            .ok()
            .and_then(|args| args.get("path")?.as_str().map(str::to_string));
        let Some(path) = path else { return };
        match lca_tools::canonical_tool_name(name) {
            "read" => {
                read.insert(path);
            }
            "write" | "edit" => {
                modified.insert(path);
            }
            _ => {}
        }
    };
    for record in candidate {
        match record {
            Record::ToolCall {
                name, arguments, ..
            } => file_op(name, arguments),
            Record::Assistant { content, .. } => {
                for block in content {
                    if let lca_protocol::ContentBlock::ToolCall {
                        name, arguments, ..
                    } = block
                    {
                        file_op(name, arguments);
                    }
                }
            }
            _ => {}
        }
    }
    // Earlier compactions accumulate: their lists survive into the
    // next record, so repeated compactions refine the same picture.
    for record in priors {
        if let Record::Compaction {
            read_files,
            modified_files,
            ..
        } = record
        {
            read.extend(read_files.iter().cloned());
            modified.extend(modified_files.iter().cloned());
        }
    }
    for path in &modified {
        read.remove(path);
    }
    (
        read.into_iter().take(MAX_TRACKED_FILES).collect(),
        modified.into_iter().take(MAX_TRACKED_FILES).collect(),
    )
}

/// The file sections appended to a summary when relevant (gh #36
/// phase 3, pi's `<read-files>` / `<modified-files>` shape). Empty
/// lists append nothing: a fileless range reads unchanged.
pub(crate) fn format_file_lists(read: &[String], modified: &[String]) -> String {
    let mut sections = Vec::new();
    if !read.is_empty() {
        sections.push(format!("<read-files>\n{}\n</read-files>", read.join("\n")));
    }
    if !modified.is_empty() {
        sections.push(format!(
            "<modified-files>\n{}\n</modified-files>",
            modified.join("\n")
        ));
    }
    if sections.is_empty() {
        return String::new();
    }
    format!("\n\n{}", sections.join("\n\n"))
}

/// A system-prompt change across compactions (gh #36 phase 3): the
/// latest checkpointed prompt differs from the current one, so the
/// change is recorded as its own `custom` record - the detection,
/// not a migration. `None` means no earlier checkpoint or no change.
pub(crate) fn detect_system_change(records: &[Record], current: &str) -> Option<Record> {
    let (id, previous) = records.iter().rev().find_map(|record| match record {
        Record::Compaction {
            id,
            system_prompt: Some(previous),
            ..
        } => Some((id.clone(), previous.clone())),
        _ => None,
    })?;
    if previous == current {
        return None;
    }
    Some(Record::Custom {
        v: FORMAT_VERSION,
        ts: lca_session::now_ms(),
        id: lca_session::new_record_id(),
        parent: None,
        custom_type: SYSTEM_PROMPT_CHANGE_TYPE.to_string(),
        data: serde_json::json!({
            "detail": format!(
                "system prompt changed since compaction {id} ({} to {} chars); \
                 the new compaction checkpoints the current prompt",
                previous.chars().count(),
                current.chars().count(),
            ),
        }),
    })
}
