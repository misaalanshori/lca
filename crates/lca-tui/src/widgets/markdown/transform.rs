//! Pre-parse markdown transforms (gh #12): pi's
//! `registerMarkdownTransformer` as an ordered list of
//! `fn(&str, &ctx) -> String` closures applied to the raw source before
//! parsing. Each transform sees the previous one's output; a transform
//! that panics behaves as identity (pi's try/catch), so a hostile
//! transform cannot break the render.
//!
//! Split from `mod.rs` for the workspace's 1,200-line file ceiling; the
//! pipeline (`render_markdown`) applies these through
//! [`apply_transformers`].

use std::sync::Arc;

/// Whose markdown is being rendered (gh #12, gh #390 gap 2): pi's
/// `MarkdownTransformContext["messageType"]`.
///
/// # Why thinking is a message type and not a flag
///
/// A ```` ```mermaid ```` fence inside a reasoning run is a *draft* the model is
/// sketching to itself; the same fence in the answer is the artefact. pi draws
/// that line through the message type (`assistant-thinking` skips the mermaid
/// transformer) rather than through a content sniff, and the distinction
/// generalises: any transformer that should not fire on reasoning asks the type,
/// and every one of them answers the same way. A content check ("does this look
/// like a draft?") is a heuristic that would be wrong in both directions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkdownMessageType {
    /// A user prompt's markdown.
    User,
    /// An assistant answer's markdown.
    Assistant,
    /// A reasoning run's markdown (gh #390 gap 2): pi's
    /// `assistant-thinking`. Transforms that produce artefacts - mermaid, and
    /// anything similar later - are skipped for this type, because reasoning is
    /// the model thinking aloud, not the answer it stands behind.
    AssistantThinking,
}

impl MarkdownMessageType {
    /// Whether this type is a reasoning run (pi's `assistant-thinking`).
    ///
    /// One predicate rather than `matches!(..)` scattered through the pipeline:
    /// a second thinking-shaped type would otherwise be added to some call sites
    /// and missed at others, and the artefact transforms are exactly where a miss
    /// is visible.
    #[must_use]
    pub fn is_reasoning(self) -> bool {
        matches!(self, MarkdownMessageType::AssistantThinking)
    }
}

/// What a pre-parse transform sees alongside the source (gh #12): pi's
/// `MarkdownTransformContext`, minus nothing the pipeline knows.
#[derive(Debug, Clone)]
pub struct MarkdownTransformContext {
    /// Whose markdown this is.
    pub message_type: MarkdownMessageType,
    /// The message is still streaming.
    pub is_streaming: bool,
    /// The width the render was asked for, in columns.
    pub available_width: usize,
}

/// A pre-parse markdown transform (gh #12): pi's
/// `registerMarkdownTransformer`. Transforms run in registration order
/// over the raw source before parsing; each sees the previous one's
/// output. A transform that panics behaves as identity (pi's
/// try/catch), so a hostile transform cannot break the render.
pub type MarkdownTransformer = Arc<dyn Fn(&str, &MarkdownTransformContext) -> String + Send + Sync>;

/// Apply the registered transforms in order over the raw source (gh #12).
pub fn apply_transformers(
    text: &str,
    context: &MarkdownTransformContext,
    transformers: &[MarkdownTransformer],
) -> String {
    let mut transformed = text.to_string();
    for transformer in transformers {
        let next = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            transformer(&transformed, context)
        }));
        if let Ok(next) = next {
            transformed = next;
        }
    }
    transformed
}
