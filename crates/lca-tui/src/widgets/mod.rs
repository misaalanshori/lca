//! The widget library: editor (with autocomplete), markdown, and image.
//! Built on `engine` primitives; no knowledge of agents, sessions,
//! providers, or the extension ABI.

pub mod autocomplete;
pub mod editor;
pub mod editor_rows;
pub mod image;
pub mod latex;
pub mod markdown;
pub mod mermaid;
pub mod paste;
pub mod tabs;
