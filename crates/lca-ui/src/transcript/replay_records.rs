//! Replaying persisted records into the transcript, and the entry appenders
//! that a replay uses.
//!
//! Split from `transcript/mod.rs` at gate 11's 1,200-line ceiling (gh #574).
//! The seam is **how rows arrive**: the live path pushes as it streams, a
//! resumed session replays a record slice through the very same appenders, so
//! `replay_records` and the `push_*` methods it calls belong together. What
//! stays in `mod.rs` is the entry list itself, its render, and the state the
//! two paths share.

use lca_tui::widgets::image::ImageInfo;

use super::nested::ChildCall;
use super::{Card, Entry, ToolStatus, Transcript};

impl Transcript {
    /// Append a replay of persisted records with the rendering the live
    /// path produced (FR-UI-7): the user band, markdown assistant text,
    /// tool cards, and image labels. The plain `user:`/`assistant:` dump
    /// this replaced could not show a table after a resume - the table had
    /// been flattened to one line per record (manual side-by-side against
    /// pi, 2026-10-01).
    ///
    /// `attachment` resolves a user-attachment hash to media type and bytes
    /// (FR-UI-13); a hash with no file replays as text alone.
    pub fn replay_records(
        &mut self,
        records: &[lca_protocol::Record],
        attachment: Option<&crate::state::LoadAttachment>,
    ) {
        use lca_protocol::{ContentBlock, Record};
        for record in records {
            match record {
                Record::SessionStart { working_dir, .. } => {
                    self.push_raw(format!("[session in {}]", crate::display_path(working_dir)))
                }
                Record::User {
                    content,
                    attachments,
                    ..
                } => {
                    // Render each resolvable attachment as the image card the
                    // live transcript showed; a hash with no file keeps the
                    // record's own stub line, which then names the missing
                    // image (FR-UI-13's named placeholder).
                    let mut resolved: Vec<&String> = Vec::new();
                    for hash in attachments {
                        if let Some((media, bytes)) = attachment.and_then(|load| load(hash)) {
                            let info = ImageInfo::new(media, &bytes);
                            self.push_image(info, bytes);
                            resolved.push(hash);
                        }
                    }
                    // The record's text carries a stub naming each
                    // attachment (assemble.rs: "the message text gains a stub
                    // naming it"); once the card is on screen that stub is a
                    // second copy of the same image. The live transcript
                    // pushed the typed text and never showed it.
                    let text = if resolved.is_empty() {
                        content.clone()
                    } else {
                        content
                            .lines()
                            .filter(|line| {
                                let stub = line.starts_with("[image attachment ");
                                !(stub
                                    && resolved
                                        .iter()
                                        .any(|hash| line.contains(&hash[..8.min(hash.len())])))
                            })
                            .collect::<Vec<_>>()
                            .join("\n")
                    };
                    if !text.trim().is_empty() {
                        self.push_user(text);
                    }
                }
                Record::Assistant {
                    content, reasoning, ..
                } => {
                    let reasoning = reasoning.as_deref().filter(|text| !text.is_empty());
                    if content.is_empty() && reasoning.is_none() {
                        continue;
                    }
                    let mut index = 0;
                    let mut reasoning_done = false;
                    while index < content.len() {
                        // An image is its own entry, as it renders live.
                        if let ContentBlock::Image { media_type, bytes } = &content[index] {
                            let info = ImageInfo::new(media_type, bytes);
                            self.push_image(info, bytes.clone());
                            index += 1;
                            continue;
                        }
                        self.begin_assistant();
                        if !reasoning_done {
                            if let Some(reasoning) = reasoning {
                                self.append_reasoning(reasoning);
                            }
                            reasoning_done = true;
                        }
                        while index < content.len()
                            && !matches!(&content[index], ContentBlock::Image { .. })
                        {
                            match &content[index] {
                                // The same call is persisted as the ToolCall
                                // and ToolResult records that follow, and
                                // the card comes from those - rendering the
                                // block too would show every call twice.
                                ContentBlock::ToolCall { .. } => {}
                                // The record's own reasoning field carries it.
                                ContentBlock::Reasoning { .. } => {}
                                ContentBlock::Text { text } => self.append_text(text),
                                ContentBlock::Image { .. } => {}
                            }
                            index += 1;
                        }
                        self.finish_assistant();
                    }
                    // A reasoning-only answer still earns its entry.
                    if !reasoning_done && let Some(reasoning) = reasoning {
                        self.begin_assistant();
                        self.append_reasoning(reasoning);
                        self.finish_assistant();
                    }
                }
                // A nested call is never its own record (gh #77): the turn
                // files it under the parent's result, so a replay only ever
                // sees top-level calls here. No parent link to consult, and
                // none needed - the checklist arrives on `ToolResult.nested`
                // below.
                Record::ToolCall {
                    name, arguments, ..
                } => self.start_tool(name.clone(), arguments.clone()),
                Record::ToolResult {
                    status,
                    content,
                    duration_ms,
                    nested,
                    ..
                } => {
                    let status = match status {
                        lca_protocol::ToolResultStatus::Ok => ToolStatus::Ok,
                        lca_protocol::ToolResultStatus::Error => ToolStatus::Error,
                        lca_protocol::ToolResultStatus::Denied => ToolStatus::Denied,
                        lca_protocol::ToolResultStatus::Timeout => ToolStatus::Timeout,
                    };
                    // Gh #214: replays show the recorded duration; old lines
                    // without one show no timing line.
                    //
                    // Gh #404 gap 1: the bounded nested record rides along, so a
                    // resumed session shows the same checklist the live one did.
                    // It is the *persisted* shape (a head, no id), which is all a
                    // checklist needs.
                    let children: Vec<ChildCall> = nested
                        .iter()
                        .map(|entry| ChildCall {
                            name: entry.name.clone(),
                            status: entry.status.into(),
                            head: entry.content_head.clone(),
                        })
                        .collect();
                    self.finish_tool_with_children(
                        status,
                        content.clone(),
                        None,
                        *duration_ms,
                        children,
                    );
                }
                // Gh #393: a card, not a wall of prose. The record still holds
                // the full summary - this is the view.
                //
                // No count: the display view has already **suppressed** the
                // records this compaction replaced (`lca_session::view::
                // suppress_compacted`), so the range's ends are not in this slice
                // and the number is not derivable here. Pi shows "Compacted from N
                // tokens" because its message carries `tokensBefore`; LCA's record
                // carries no pre-compaction token count (its `usage` is the
                // summarization call's own cost), so the card names the record
                // range instead of inventing a figure.
                Record::Compaction {
                    summary,
                    strategy,
                    replaced_from,
                    replaced_to,
                    ..
                } => {
                    self.entries.push(Entry::Compaction {
                        card: Card::replay(strategy, summary, replaced_from, replaced_to, None),
                        expanded: None,
                    });
                }
                // Gh #393 gap 2: a branch summary is the same wall-of-text
                // problem in a different record. Before this it rendered
                // **nowhere** - `BranchSummary` fell into the `_ => {}` arm below,
                // so a resumed fork silently lost the text that explained it.
                Record::BranchSummary { summary, .. } => {
                    self.entries.push(Entry::Compaction {
                        card: Card::branch(summary),
                        expanded: None,
                    });
                }
                // Permission decisions, extension events, fork points, and
                // the end marker are transient or structural: the live path
                // shows them as the bottom notice, not as transcript lines.
                _ => {}
            }
        }
    }

    /// Append a compaction card (gh #393). A *new* card always starts collapsed:
    /// the reader's expansion belongs to the card they opened, not to the next
    /// summary that arrives.
    pub fn push_compaction_card(&mut self, card: Card) {
        self.entries.push(Entry::Compaction {
            card,
            expanded: None,
        });
    }

    /// Append an image (FR-UI-13, R5): rendered through the graphics ladder.
    pub fn push_image(&mut self, info: ImageInfo, bytes: Vec<u8>) {
        self.entries.push(Entry::Image {
            info,
            bytes: std::sync::Arc::new(bytes),
        });
    }

    /// Append streamed tool output to the most recent running tool card.
    pub fn append_tool_output(&mut self, chunk: &str) {
        self.invalidate_last();
        for entry in self.entries.iter_mut().rev() {
            if let Entry::Tool { result, .. } = entry {
                result.get_or_insert_with(String::new).push_str(chunk);
                return;
            }
        }
        self.entries.push(Entry::Tool {
            name: "tool".into(),
            args: String::new(),
            status: ToolStatus::Running,
            result: Some(chunk.to_string()),
            diff: None,
            manual: false,
            expanded: None,
            duration_ms: None,
            started_at: Some(std::time::Instant::now()),
            excluded: false,
            children: Vec::new(),
            parent_link: None,
            call_id: None,
        });
    }
}
