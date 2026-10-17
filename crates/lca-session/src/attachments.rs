//! Attachment reachability: ONE walker that says which
//! `attachments/<sha256>` files a record set still needs.
//!
//! This module exists because of gh #311. Two walkers used to answer this
//! question independently — the GC's mark set and the export's attachment
//! map — and they drifted: the export learned about tool-result `images`
//! (gh #113) while the GC never did, so `lca session gc` deleted live
//! tool-result images. A shared walker makes "gc keeps what export
//! resolves" true by construction rather than by two edits agreeing.
//!
//! The checklist below is deliberately explicit: when [`Record`] grows a
//! field that carries an attachment hash, this is the one place to add it.

use std::collections::BTreeSet;

use lca_protocol::Record;

/// Every attachment hash `records` references, in ascending order.
///
/// Enumerates each hash-bearing field of [`Record`]. The list is the whole
/// contract:
///
/// * [`Record::User::attachments`] — images/files the user attached.
/// * [`Record::ToolResult::attachment`] — the oversized-text spill hash
///   (FR-TOOL-7); its bytes live in the same `attachments/` directory.
/// * [`Record::ToolResult::images`] — image payloads a tool returned
///   (gh #113). The field the GC used to miss (gh #311).
///
/// Deliberately excluded, with reasons, so a future reader does not add
/// them by pattern-matching:
///
/// * [`Record::ToolResult::full_output_path`] — the spill's *path*, derived
///   from `attachment` (`spill_dir.join(hash)`), never an independent hash.
/// * [`Record::Compaction::read_files`] / `modified_files` — workspace
///   paths, not content-addressed attachments.
/// * [`Record::Custom::data`] — an extension's own opaque JSON, which the
///   attachment contract does not reach (ADR-0030).
/// * [`Record::BranchSummary`] / [`Record::ContextEdit`] text — plain text,
///   never a hash.
pub(crate) fn referenced_attachments(records: &[Record], out: &mut BTreeSet<String>) {
    for record in records {
        match record {
            Record::User { attachments, .. } => {
                out.extend(attachments.iter().cloned());
            }
            Record::ToolResult {
                attachment, images, ..
            } => {
                if let Some(hash) = attachment {
                    out.insert(hash.clone());
                }
                out.extend(images.iter().cloned());
            }
            _ => {}
        }
    }
}

/// [`referenced_attachments`] as a fresh ordered set.
pub(crate) fn reachable_set(records: &[Record]) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    referenced_attachments(records, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use lca_protocol::{Record, ToolResultStatus};

    const V: u32 = lca_protocol::FORMAT_VERSION;

    fn tool_result(id: &str, attachment: Option<&str>, images: &[&str]) -> Record {
        Record::ToolResult {
            v: V,
            ts: 1,
            id: id.to_string(),
            parent: None,
            call_id: format!("{id}-call"),
            status: ToolResultStatus::Ok,
            content: None,
            attachment: attachment.map(str::to_string),
            truncated: false,
            duration_ms: None,
            exit_code: None,
            nested: Vec::new(),
            full_output_path: None,
            images: images.iter().map(|h| h.to_string()).collect(),
        }
    }

    fn user(id: &str, attachments: &[&str]) -> Record {
        Record::User {
            v: V,
            ts: 1,
            id: id.to_string(),
            parent: None,
            content: "hi".into(),
            attachments: attachments.iter().map(|h| h.to_string()).collect(),
            queue: None,
        }
    }

    // Verifies: gh #311 - every hash-bearing record shape is collected:
    // a user attachment, a tool-result text spill, and tool-result images
    // (the shape the GC used to miss).
    #[test]
    fn every_hash_bearing_shape_is_collected() {
        let records = vec![
            user("u", &["aaa", "bbb"]),
            tool_result("t1", Some("ccc"), &[]),
            tool_result("t2", None, &["ddd", "eee"]),
            tool_result("t3", Some("fff"), &["ggg"]),
        ];
        let set = reachable_set(&records);
        assert_eq!(
            set.into_iter().collect::<Vec<_>>(),
            ["aaa", "bbb", "ccc", "ddd", "eee", "fff", "ggg"]
        );
    }

    // Verifies: gh #311 - the fields this walker deliberately ignores never
    // leak a hash into the mark set (they are not attachment hashes).
    #[test]
    fn non_attachment_fields_contribute_nothing() {
        let records = vec![Record::Compaction {
            v: V,
            ts: 1,
            id: "c".into(),
            parent: None,
            replaced_from: "a".into(),
            replaced_to: "b".into(),
            first_kept_id: String::new(),
            read_files: vec!["src/main.rs".into()],
            modified_files: vec!["README.md".into()],
            system_prompt: None,
            summary: "s".into(),
            strategy: "s".into(),
            usage: None,
        }];
        assert!(reachable_set(&records).is_empty());
    }

    // Verifies: gh #311 - a tool result with neither a spill nor images
    // contributes nothing (so an unrelated tool call cannot pin a file).
    #[test]
    fn a_bare_tool_result_contributes_nothing() {
        assert!(reachable_set(&[tool_result("t", None, &[])]).is_empty());
    }
}
