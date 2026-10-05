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

/// Whose markdown is being rendered (gh #12): pi's
/// `MarkdownTransformContext["messageType"]`. Reasoning runs render as
/// plain wrapped lines rather than parsed markdown, so there is no
/// thinking variant - transforms see user and assistant sources only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkdownMessageType {
    /// A user prompt's markdown.
    User,
    /// An assistant answer's markdown.
    Assistant,
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
