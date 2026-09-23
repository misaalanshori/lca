//! The widget tree and interaction types the `ui` world crosses with
//! (ADR-0003: the extension returns a tree, the host lays it out and
//! draws it; spans carry data, never control codes - FR-UI-2).

use serde::{Deserialize, Serialize};

/// One node of a rendered region's tree. Children on `boxed`, `row`,
/// and `column` are indices into the same node list, with node0 as the
/// root: WIT records cannot be recursive, so trees travel as an arena
/// (documented at the `render` interface).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Widget {
    /// Content plus its semantic role (`default`, `muted`, `warning`,
    /// `error`, `accent`, ...): intent, not color, so it degrades to
    /// plain text without color (FR-UI-5).
    Text {
        /// The text (host-sanitized: no control codes reach a terminal,
        /// FR-UI-2).
        content: String,
        /// Semantic color role.
        role: String,
    },
    /// An image: media type plus bytes (ADR-0003's pre-freeze
    /// addition for provider image output).
    Image {
        /// The media type (`image/png`, ...).
        media_type: String,
        /// The encoded bytes.
        bytes: Vec<u8>,
    },
    /// A bordered box with an optional title around one child index.
    Boxed {
        /// The title, when present.
        title: Option<String>,
        /// Child node index.
        child: u32,
    },
    /// Children side by side, as node indices.
    Row(Vec<u32>),
    /// Children stacked, as node indices.
    Column(Vec<u32>),
    /// A busy indicator: the extension hands over its frames and the
    /// host animates them (ADR-0003: animation is the renderer's).
    Spinner {
        /// The frames, e.g. `⠋⠙⠹`.
        frames: String,
    },
    /// A label with a0..1 fill fraction.
    Progress {
        /// The label.
        label: String,
        /// Fill fraction.
        fill: f32,
    },
    /// Label/value pairs (the key-value list).
    KeyValue(Vec<(String, String)>),
    /// Reserved so the vocabulary's next kind lands without a canonical
    /// ABI break (ADR-0003; never constructed by1.0 hosts).
    Vendor(String),
}

/// A region's rendered tree: the arena, node0 the root.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct WidgetTree {
    /// The nodes, arena-style.
    pub nodes: Vec<Widget>,
}

impl WidgetTree {
    /// An empty tree (nothing to show).
    pub fn empty() -> WidgetTree {
        WidgetTree::default()
    }

    /// Whether there is anything to draw.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
}

/// One user interaction delivered to a registered extension (the
/// `interaction` world's input variant).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum UiInput {
    /// A key the user pressed while the region was focused.
    Key {
        /// The key's logical name or text.
        key: String,
    },
    /// The user submitted what they typed in a modal.
    Submit {
        /// The submitted text.
        text: String,
    },
    /// The user dismissed the modal.
    Cancel,
}

/// What an interaction asks the host to do (the effect variant).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum UiEffect {
    /// Nothing.
    None,
    /// Close this extension's modal.
    CloseModal,
    /// Ask for the modal region (the host honors it only right after a
    /// real user event: FR-UI-6).
    OpenModal,
    /// Show text in the notice area.
    ShowNotice(String),
    /// Insert into the input editor.
    InsertText(String),
    /// Submit a prompt.
    SubmitPrompt(String),
}
