//! The widget tree and interaction types the `ui` world crosses with
//! (ADR-0003: the extension returns a tree, the host lays it out and
//! draws it; spans carry data, never control codes - FR-UI-2).

use serde::{Deserialize, Serialize};

/// How one span paints (gh #172, `text-style` across the ABI): a
/// foreground and a background that travel independently, plus
/// decorations. Each color is a semantic role (`accent`, ...) OR a
/// `#RRGGBB` hex literal; the host parses hex into RGB bytes and
/// resolves roles through the theme. Absent means the surrounding
/// default for that channel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct TextStyle {
    /// Foreground role or hex.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fg: Option<String>,
    /// Background role or hex.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bg: Option<String>,
    /// Bold.
    #[serde(default)]
    pub bold: bool,
    /// Dim.
    #[serde(default)]
    pub dim: bool,
    /// Italic.
    #[serde(default)]
    pub italic: bool,
    /// Underline.
    #[serde(default)]
    pub underline: bool,
}

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
    /// Content with independent foreground/background channels and
    /// decorations (gh #172): the dual-channel upgrade of `Text`.
    StyledText {
        /// The text (host-sanitized like `Text`).
        content: String,
        /// How it paints.
        style: TextStyle,
    },
    /// Markdown source, rendered through the host's own markdown
    /// engine.
    Markdown {
        /// The source.
        source: String,
    },
    /// A clickable button: the `id` a `ClickWidget` input names back,
    /// and the label the host draws.
    Button {
        /// The widget id.
        id: String,
        /// The label.
        label: String,
    },
    /// Headers plus body rows; the host aligns every column.
    Table {
        /// The header row.
        headers: Vec<String>,
        /// The body rows.
        rows: Vec<Vec<String>>,
    },
    /// A virtual viewport over child indices: the host clips to
    /// `max_height` rows and paints a scrollbar thumb.
    ScrollContainer {
        /// The viewport height in rows.
        max_height: u32,
        /// Child node indices.
        children: Vec<u32>,
    },
    /// A bordered box with an optional title around one child index.
    /// The 0.6 shape adds the border's role and the background tint
    /// (both role-or-hex like `TextStyle`); absent means the theme's
    /// box default for that channel.
    Boxed {
        /// The title, when present.
        title: Option<String>,
        /// The border's role or hex.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        border: Option<String>,
        /// The background tint role or hex.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        background: Option<String>,
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
    /// The user clicked a declared `button` widget's id (gh #172).
    ClickWidget {
        /// The widget id.
        id: String,
    },
    /// The user clicked inside the region at a relative cell (gh
    /// #172): columns and rows from the region's top-left.
    Click {
        /// Relative column.
        col: u32,
        /// Relative row.
        row: u32,
    },
    /// The user turned the wheel over the region (gh #172): +1 down,
    /// -1 up.
    Scroll {
        /// The wheel delta.
        delta: i32,
    },
}

/// One host-rendered question (gh #124, the `ui-dialogs` import): the
/// kind plus what the user saw, so headless and tests can answer
/// without a terminal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum UiDialog {
    /// Yes/No buttons; `false` is no or dismissed.
    Confirm {
        /// The title.
        title: String,
        /// The question.
        message: String,
    },
    /// A picker over options; `None` is dismissed.
    Select {
        /// The title.
        title: String,
        /// The options.
        options: Vec<String>,
    },
    /// One line of text; `None` is dismissed.
    Input {
        /// The label.
        label: String,
        /// The placeholder, when any.
        placeholder: Option<String>,
    },
    /// A transient notice (no answer; headless drops it).
    Notify {
        /// The text.
        message: String,
        /// `info`, `warning`, or `error`.
        level: String,
    },
}

/// What a host dialog answered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum DialogAnswer {
    /// The confirm verdict.
    Confirm(bool),
    /// The picked option, if any.
    Select(Option<String>),
    /// The entered line, if any.
    Input(Option<String>),
    /// A notification asks for nothing.
    Notify,
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
