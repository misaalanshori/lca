//! What the source says: the diagram model, the statement scan's vocabulary
//! (kinds, directions, node shapes, arrow tables, edge splitting), and the
//! character grid the renderers paint into.
//!
//! Split from the barrel for gate 11 (the 1,200-line ceiling); the code, its
//! comments and its rows are unchanged. `render.rs` draws; this layer decides
//! what there is to draw, and owns the canvas type they both write to.

use super::{Class, Span};

pub(super) enum Kind {
    Sequence,
    Flow { direction: Direction },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Direction {
    TopDown,
    LeftRight,
}

pub(super) fn direction_of(header: &str) -> Direction {
    let words: Vec<&str> = header.split_whitespace().collect();
    match words.get(1).copied().unwrap_or("TD") {
        "LR" | "RL" => Direction::LeftRight,
        _ => Direction::TopDown,
    }
}

/// Rows under construction: a character grid with per-cell classes, so
/// borders, edges, and labels can be painted independently.
#[derive(Default)]
pub(super) struct Canvas {
    pub(super) cells: Vec<Vec<char>>,
    classes: Vec<Vec<Class>>,
}

impl Canvas {
    pub(super) fn new() -> Self {
        Self::default()
    }

    pub(super) fn ensure(&mut self, row: usize, col: usize) {
        while self.cells.len() <= row {
            self.cells.push(Vec::new());
            self.classes.push(Vec::new());
        }
        while self.cells[row].len() <= col {
            self.cells[row].push(' ');
            self.classes[row].push(Class::None);
        }
    }

    pub(super) fn put(&mut self, row: usize, col: usize, ch: char, class: Class) {
        self.ensure(row, col);
        self.cells[row][col] = ch;
        self.classes[row][col] = class;
    }

    pub(super) fn put_str(&mut self, row: usize, col: usize, text: &str, class: Class) {
        for (offset, ch) in text.chars().enumerate() {
            self.put(row, col + offset, ch, class);
        }
    }

    /// Paint a horizontal run of `ch`, skipping cells something else owns.
    pub(super) fn hline(&mut self, row: usize, from: usize, to: usize, ch: char, class: Class) {
        for col in from..=to {
            self.ensure(row, col);
            if self.cells[row][col] == ' ' {
                self.put(row, col, ch, class);
            }
        }
    }

    pub(super) fn into_rows(self) -> Vec<Vec<Span>> {
        self.cells
            .into_iter()
            .zip(self.classes)
            .map(|(row, classes)| {
                let mut spans: Vec<Span> = Vec::new();
                for (ch, class) in row.into_iter().zip(classes) {
                    match spans.last_mut() {
                        Some(last) if last.class == class => last.text.push(ch),
                        _ => spans.push(Span::new(ch.to_string(), class)),
                    }
                }
                // Trailing pure-space runs carry no meaning.
                while let Some(last) = spans.last()
                    && last.class == Class::None
                    && last.text.chars().all(|c| c == ' ')
                {
                    spans.pop();
                }
                spans
            })
            .collect()
    }
}

/// Longest-first arrow tokens (mermaid's message syntax).
pub(super) const ARROWS: &[&str] = &["-->>", "->>", "-->", "->", "--x", "--o", "-)", "-x", "-o"];

/// A node's outline: rows of (char, class) already drawn to shape.
pub(super) struct NodeBox {
    pub(super) rows: Vec<Vec<(char, Class)>>,
    pub(super) width: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Shape {
    Rect,
    Round,
    Rhombus,
    DoubleRound,
    Asymmetric,
}

/// Parse `id[label]`-shaped node syntax; `bare` also accepts a plain id.
pub(super) fn parse_node(text: &str, bare: bool) -> Option<(String, String, Shape)> {
    let text = text.trim();
    let id: String = text
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    if id.is_empty() {
        return None;
    }
    let rest = text[id.len()..].trim();
    if rest.is_empty() {
        return if bare {
            Some((id.clone(), id, Shape::Rect))
        } else {
            None
        };
    }
    let shape_and_label = |open: char, close: char, shape: Shape| {
        rest.strip_prefix(open)
            .and_then(|inner| inner.strip_suffix(close))
            .map(|label| (id.clone(), label.to_string(), shape))
    };
    rest.strip_prefix("[[")
        .and_then(|inner| inner.strip_suffix("]]"))
        .map(|label| (id.clone(), label.to_string(), Shape::Rect))
        .or_else(|| {
            rest.strip_prefix("[(")
                .and_then(|inner| inner.strip_suffix(")]"))
                .map(|label| (id.clone(), label.to_string(), Shape::Round))
        })
        .or_else(|| {
            rest.strip_prefix("((")
                .and_then(|inner| inner.strip_suffix("))"))
                .map(|label| (id.clone(), label.to_string(), Shape::DoubleRound))
        })
        .or_else(|| shape_and_label('[', ']', Shape::Rect))
        .or_else(|| shape_and_label('(', ')', Shape::Round))
        .or_else(|| shape_and_label('{', '}', Shape::Rhombus))
        .or_else(|| {
            rest.strip_prefix('>')
                .and_then(|inner| inner.strip_suffix(']'))
                .map(|label| (id.clone(), label.to_string(), Shape::Asymmetric))
        })
}

/// Flowchart edge tokens, longest first.
pub(super) const FLOW_ARROWS: &[&str] = &["-.->", "==>", "-->", "---", "--x", "--o"];

/// Whether the `>` at `at` opens an asymmetric node rather than ending an arrow.
///
/// `id>label]` and an arrow head both use `>`, so the opening one is the `>` that
/// has an identifier immediately before it and a closing `]` somewhere after.
/// Without this distinction `A --> B` would be read as an asymmetric opener at
/// the arrow's own head.
pub(super) fn is_asymmetric_open(bytes: &[char], at: usize) -> bool {
    let ident_before = at > 0 && (bytes[at - 1].is_ascii_alphanumeric() || bytes[at - 1] == '_');
    // An arrow's `>` is preceded by `-`, `=` or `.`; an asymmetric opener is
    // preceded by the node id's last character.
    ident_before && bytes[at + 1..].contains(&']')
}

/// Split `A -->|label| B` / `A -- label --> B` style statements.
///
/// Returns owned `String`s for the two sides (gh #325). It used to return
/// `&'static str` via `String::leak`, which bought **nothing**: the only caller
/// immediately hands each side to `parse_node`, which copies the identifier out
/// into its own `String` anyway. So the leak was a pure cost - two allocations
/// per edge that were never freed, on every paint, because a diagram re-renders
/// whenever its entry is re-rendered (a resize, a repaint, a `/theme` change).
pub(super) fn split_edge(statement: &str) -> Option<(String, &str, String, String)> {
    let bytes: Vec<char> = statement.chars().collect();
    let mut depth = 0i32;
    let mut at = None;
    let mut i = 0;
    'scan: while i < bytes.len() {
        match bytes[i] {
            '[' | '(' | '{' => depth += 1,
            // Gh #325: `>` opens an asymmetric node (`id>label]`), so it must be
            // counted or the `]` that closes it drives `depth` negative and the
            // rest of the statement - arrow included - is scanned as if it were
            // inside a node. That is why an asymmetric node on the *left* of an
            // edge never split, while one on the right did: the scanner only
            // reached the arrow before the first `]`.
            //
            // Only at depth 0, so a `>` inside brackets stays literal text.
            '>' if depth == 0 && is_asymmetric_open(&bytes, i) => depth += 1,
            ']' | ')' | '}' => depth = (depth - 1).max(0),
            '-' | '=' | '.' | '>' | 'x' | 'o' if depth == 0 => {
                for token in FLOW_ARROWS {
                    let token_chars: Vec<char> = token.chars().collect();
                    if i + token_chars.len() <= bytes.len()
                        && bytes[i..i + token_chars.len()] == token_chars[..]
                    {
                        at = Some((i, *token));
                        break 'scan;
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
    let (at, arrow) = at?;
    let left_raw: String = bytes[..at].iter().collect();
    let after: String = bytes[at + arrow.len()..].iter().collect();

    // `A -- label --> B`: the label trails the left side after ` -- `.
    let (left, inline_label) = match left_raw.rfind(" -- ") {
        Some(split) => (
            left_raw[..split].to_string(),
            left_raw[split + 4..].trim().to_string(),
        ),
        None => (left_raw.trim().to_string(), String::new()),
    };

    // `A -->|label| B`: the label rides on the right side.
    let after = after.trim_start();
    let (label, right) = if let Some(rest) = after.strip_prefix('|') {
        match rest.find('|') {
            Some(end) => (rest[..end].to_string(), rest[end + 1..].trim_start()),
            None => (String::new(), after),
        }
    } else {
        (inline_label, after)
    };
    Some((
        left.trim().to_string(),
        arrow,
        label,
        right.trim().to_string(),
    ))
}
