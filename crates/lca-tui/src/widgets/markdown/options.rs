//! Render options for terminal markdown.
//!
//! Split from `mod.rs` for the workspace's 1,200-line file ceiling; the
//! pipeline reads these on every render.

use super::transform::{MarkdownMessageType, MarkdownTransformer};
use super::{CodeBlockBorder, LinkMode};

/// Options for rendering.
#[derive(Clone)]
pub struct MarkdownOptions {
    /// Left/right padding.
    pub padding_x: usize,
    /// Blank lines above and below.
    pub padding_y: usize,
    /// Link rendering.
    pub link_mode: LinkMode,
    /// Keep the authored ordered marker (`1.` vs `1)`) instead of
    /// renumbering the run (pi's `preserveOrderedListMarkers`).
    pub preserve_ordered_list_markers: bool,
    /// Keep backslash escapes as written instead of normalizing them to
    /// the escaped character (pi's `preserveBackslashEscapes`).
    pub preserve_backslash_escapes: bool,
    /// Render supported math to Unicode instead of always showing the
    /// source (pi's `renderLatex`, default true).
    pub render_latex: bool,
    /// The message is still streaming: pi suppresses mermaid warnings
    /// mid-stream and shows them once the message settles.
    pub streaming: bool,
    /// How fenced code blocks are framed (gh #32); `full` is the
    /// shipped look and the default.
    pub codeblock_border: CodeBlockBorder,
    /// Whose markdown this is (gh #12): the transform context's message
    /// type. Assistant answers are the common case, hence the default.
    pub message_type: MarkdownMessageType,
    /// Pre-parse transforms in registration order (gh #12): pi's
    /// `registerMarkdownTransformer`. Empty by default - no consumer,
    /// no rewriting.
    pub transformers: Vec<MarkdownTransformer>,
    /// Mermaid rendering (gh #82, pi's `markdown.mermaid`): `off`
    /// keeps fences raw, `final` renders settled messages, `streaming`
    /// renders mid-stream too.
    pub mermaid: MermaidMode,
    /// Code block content indent (gh #82, pi's
    /// `markdown.codeBlockIndent`): prefixed to every rendered block
    /// line.
    pub code_indent: String,
}

/// When mermaid fences become Unicode art (gh #82).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum MermaidMode {
    /// Never render: fences stay raw code.
    Off,
    /// Render settled messages only.
    Final,
    /// Render mid-stream too (pi's default).
    #[default]
    Streaming,
}

impl MermaidMode {
    /// Parse the config value; anything unrecognized streams (the
    /// config layer refuses those first, so this is unreachable).
    pub fn parse(text: &str) -> MermaidMode {
        match text {
            "off" => MermaidMode::Off,
            "final" => MermaidMode::Final,
            _ => MermaidMode::Streaming,
        }
    }
}

impl Default for MarkdownOptions {
    fn default() -> Self {
        Self {
            padding_x: 0,
            padding_y: 0,
            link_mode: LinkMode::Hyperlink,
            preserve_ordered_list_markers: false,
            preserve_backslash_escapes: false,
            render_latex: true,
            streaming: false,
            codeblock_border: CodeBlockBorder::Full,
            message_type: MarkdownMessageType::Assistant,
            transformers: Vec::new(),
            mermaid: MermaidMode::Streaming,
            // Empty keeps every existing caller byte-identical; the
            // transcript passes the configured indent explicitly.
            code_indent: String::new(),
        }
    }
}
