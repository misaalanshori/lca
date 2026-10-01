//! Mermaid diagrams as Unicode art: pi renders top-level
//! ```` ```mermaid ```` blocks through `grok-mermaid` (RE
//! `agent-components/chrome.md` §mermaid).
//!
//! `grok-mermaid` is an npm package (0.2.3), so its JavaScript cannot be
//! taken; this is the subset the review names - flowcharts and sequence
//! diagrams - behind the same fence, emitting the same span classes
//! (`border`, `text`, `edge`, `edgeLabel`, `title`) that the theme
//! colors, and the same contract: `None` for anything it cannot parse
//! (markdown falls back to the fenced source), a width guard in the
//! caller, and warnings that make markdown prefer the raw source while a
//! message is still streaming.

/// grok-mermaid's span classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// Box-drawing borders.
    Border,
    /// Node and note text.
    Text,
    /// Arrows and connectors.
    Edge,
    /// Labels sitting on or beside an edge.
    EdgeLabel,
    /// The diagram title.
    Title,
    /// Untouched text.
    None,
}

/// One styled run of a diagram row.
#[derive(Debug, Clone)]
pub struct Span {
    /// The literal text.
    pub text: String,
    /// Which class it carries.
    pub class: Class,
}

impl Span {
    fn new(text: impl Into<String>, class: Class) -> Self {
        Self {
            text: text.into(),
            class,
        }
    }
}

/// A rendered diagram: rows of spans, its width, and any warnings.
#[derive(Debug, Clone)]
pub struct Art {
    /// The rows (grok-mermaid's `art.styled`).
    pub lines: Vec<Vec<Span>>,
    /// Width in cells (pi's `art.width` guard).
    pub width: usize,
    /// Parse-time warnings (pi's `art.warnings`).
    pub warnings: Vec<String>,
}

/// Render mermaid source, or `None` when this subset cannot parse it
/// (markdown keeps the fenced source, pi's fail-soft contract).
pub fn render(source: &str) -> Option<Art> {
    let mut kind = None;
    let mut title: Option<String> = None;
    let mut warnings: Vec<String> = Vec::new();
    let mut statements: Vec<&str> = Vec::new();
    for line in source.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("%%") {
            continue;
        }
        if kind.is_none() {
            if let Some(rest) = trimmed.strip_prefix("sequenceDiagram") {
                kind = Some(Kind::Sequence);
                let rest = rest.trim();
                if !rest.is_empty() {
                    statements.push(rest);
                }
                continue;
            }
            if trimmed.starts_with("flowchart") || trimmed.starts_with("graph") {
                kind = Some(Kind::Flow {
                    direction: direction_of(trimmed),
                });
                let mut rest = trimmed.split_whitespace().nth(1).unwrap_or("");
                if trimmed.starts_with("graph") {
                    rest = trimmed.split_whitespace().nth(1).unwrap_or("");
                }
                if !rest.is_empty() && !matches!(rest, "TD" | "TB" | "LR" | "RL" | "BT") {
                    warnings.push(format!("unknown flow direction `{rest}`"));
                }
                continue;
            }
            // Anything else (class/state/pie/gantt...) is outside the subset.
            return None;
        }
        if let Some(value) = trimmed.strip_prefix("title ") {
            title = Some(value.trim().to_string());
            continue;
        }
        statements.push(trimmed);
    }
    match kind? {
        Kind::Sequence => render_sequence(&statements, title, warnings),
        Kind::Flow { direction } => render_flow(&statements, direction, title, warnings),
    }
}

enum Kind {
    Sequence,
    Flow { direction: Direction },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    TopDown,
    LeftRight,
}

fn direction_of(header: &str) -> Direction {
    let words: Vec<&str> = header.split_whitespace().collect();
    match words.get(1).copied().unwrap_or("TD") {
        "LR" | "RL" => Direction::LeftRight,
        _ => Direction::TopDown,
    }
}

/// Rows under construction: a character grid with per-cell classes, so
/// borders, edges, and labels can be painted independently.
#[derive(Default)]
struct Canvas {
    cells: Vec<Vec<char>>,
    classes: Vec<Vec<Class>>,
}

impl Canvas {
    fn new() -> Self {
        Self::default()
    }

    fn ensure(&mut self, row: usize, col: usize) {
        while self.cells.len() <= row {
            self.cells.push(Vec::new());
            self.classes.push(Vec::new());
        }
        while self.cells[row].len() <= col {
            self.cells[row].push(' ');
            self.classes[row].push(Class::None);
        }
    }

    fn put(&mut self, row: usize, col: usize, ch: char, class: Class) {
        self.ensure(row, col);
        self.cells[row][col] = ch;
        self.classes[row][col] = class;
    }

    fn put_str(&mut self, row: usize, col: usize, text: &str, class: Class) {
        for (offset, ch) in text.chars().enumerate() {
            self.put(row, col + offset, ch, class);
        }
    }

    /// Paint a horizontal run of `ch`, skipping cells something else owns.
    fn hline(&mut self, row: usize, from: usize, to: usize, ch: char, class: Class) {
        for col in from..=to {
            self.ensure(row, col);
            if self.cells[row][col] == ' ' {
                self.put(row, col, ch, class);
            }
        }
    }

    fn into_rows(self) -> Vec<Vec<Span>> {
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
const ARROWS: &[&str] = &["-->>", "->>", "-->", "->", "--x", "--o", "-)", "-x", "-o"];

fn render_sequence(
    statements: &[&str],
    title: Option<String>,
    mut warnings: Vec<String>,
) -> Option<Art> {
    #[derive(Debug)]
    enum Event {
        Message {
            from: usize,
            to: usize,
            label: String,
            head: char,
        },
        Note {
            from: usize,
            to: usize,
            text: String,
        },
    }

    let mut actors: Vec<String> = Vec::new();
    let mut labels: Vec<String> = Vec::new();
    let mut events: Vec<Event> = Vec::new();

    let actor_index = |actors: &mut Vec<String>, labels: &mut Vec<String>, name: &str| -> usize {
        if let Some(at) = actors.iter().position(|a| a == name) {
            return at;
        }
        actors.push(name.to_string());
        labels.push(name.to_string());
        actors.len() - 1
    };

    for statement in statements {
        let statement = *statement;
        if let Some(rest) = statement
            .strip_prefix("participant ")
            .or_else(|| statement.strip_prefix("actor "))
        {
            let (name, label) = match rest.split_once(" as ") {
                Some((name, label)) => (name.trim(), label.trim()),
                None => (rest.trim(), rest.trim()),
            };
            if name.is_empty() {
                warnings.push("empty participant name".to_string());
                continue;
            }
            let index = actor_index(&mut actors, &mut labels, name);
            if !label.is_empty() {
                labels[index] = label.to_string();
            }
            continue;
        }
        if let Some(rest) = statement
            .strip_prefix("Note over ")
            .or_else(|| statement.strip_prefix("note over "))
        {
            let (who, text) = rest.split_once(':').unwrap_or((rest, ""));
            let names: Vec<&str> = who.split(',').map(str::trim).collect();
            if names.is_empty() {
                warnings.push("note without a target".to_string());
                continue;
            }
            let first = actor_index(&mut actors, &mut labels, names[0]);
            let last = names
                .iter()
                .map(|name| actor_index(&mut actors, &mut labels, name))
                .max()
                .unwrap_or(first);
            events.push(Event::Note {
                from: first.min(last),
                to: first.max(last),
                text: text.trim().to_string(),
            });
            continue;
        }
        let Some(arrow_at) = ARROWS
            .iter()
            .find_map(|arrow| statement.find(arrow).map(|at| (at, *arrow)))
        else {
            warnings.push(format!("unsupported statement `{statement}`"));
            continue;
        };
        let (at, arrow) = arrow_at;
        let from = statement[..at].trim().to_string();
        let after = &statement[at + arrow.len()..];
        let (to, label) = match after.split_once(':') {
            Some((to, label)) => (to.trim().to_string(), label.trim().to_string()),
            None => (after.trim().to_string(), String::new()),
        };
        if from.is_empty() || to.is_empty() {
            warnings.push(format!("message without both ends `{statement}`"));
            continue;
        }
        let head = if arrow.ends_with('x') {
            '✕'
        } else if arrow.ends_with('o') {
            '○'
        } else {
            '▶'
        };
        let from_index = actor_index(&mut actors, &mut labels, &from);
        let to_index = actor_index(&mut actors, &mut labels, &to);
        events.push(Event::Message {
            from: from_index,
            to: to_index,
            label,
            head,
        });
    }

    if actors.is_empty() || events.is_empty() {
        return None;
    }

    // Column geometry: each actor gets a column of its label width, four
    // cells of air between columns.
    let widths: Vec<usize> = labels
        .iter()
        .map(|label| label.chars().count().max(2) + 2)
        .collect();
    let mut xs: Vec<usize> = Vec::with_capacity(labels.len());
    let mut cursor = 0usize;
    for width in &widths {
        xs.push(cursor);
        cursor += width + 4;
    }
    let centers: Vec<usize> = xs
        .iter()
        .zip(&widths)
        .map(|(x, width)| x + width / 2)
        .collect();

    let mut canvas = Canvas::new();
    let mut row = 0usize;
    if let Some(title) = &title {
        canvas.put_str(0, 0, title, Class::Title);
        row = 2;
    }
    // Header: top border, names, bottom border.
    for (index, label) in labels.iter().enumerate() {
        let x = xs[index];
        let width = widths[index];
        for col in x..x + width {
            canvas.put(row, col, '─', Class::Border);
        }
        canvas.put(row, x, '┌', Class::Border);
        canvas.put(row, x + width - 1, '┐', Class::Border);
        canvas.put_str(row + 1, x + 1, label, Class::Text);
        canvas.put(row + 1, x, '│', Class::Border);
        canvas.put(row + 1, x + width - 1, '│', Class::Border);
        for col in x..x + width {
            canvas.put(row + 2, col, '─', Class::Border);
        }
        canvas.put(row + 2, x, '└', Class::Border);
        canvas.put(row + 2, x + width - 1, '┘', Class::Border);
    }
    row += 3;

    for event in events {
        match event {
            Event::Note { from, to, text } => {
                let start = xs[from] + 1;
                canvas.put_str(row, start, &format!("· {text} ·"), Class::Text);
                let _ = to;
                row += 1;
            }
            Event::Message {
                from,
                to,
                label,
                head,
            } => {
                if from == to {
                    let center = centers[from];
                    let text = if label.is_empty() {
                        String::new()
                    } else {
                        format!(" {label}")
                    };
                    canvas.put(row, center.saturating_sub(1), '┌', Class::Border);
                    canvas.put(row, center, '─', Class::Edge);
                    canvas.put(row, center + 1, head, Class::Edge);
                    canvas.put_str(row, center + 2, &text, Class::EdgeLabel);
                    canvas.put(row + 1, center.saturating_sub(1), '│', Class::Border);
                    canvas.put(row + 2, center.saturating_sub(1), '└', Class::Border);
                    canvas.put(row + 2, center, '─', Class::Edge);
                    canvas.put(row + 2, center + 1, '┘', Class::Border);
                    row += 3;
                    continue;
                }
                let (left, right) = (
                    centers[from].min(centers[to]),
                    centers[from].max(centers[to]),
                );
                for col in left + 1..right {
                    canvas.put(row, col, '─', Class::Edge);
                }
                if centers[to] > centers[from] {
                    canvas.put(row, right, head, Class::Edge);
                } else {
                    canvas.put(row, left, head, Class::Edge);
                }
                let gap = right - left;
                if !label.is_empty() {
                    let label_width = label.chars().count();
                    if label_width + 2 < gap {
                        let start = left + 1 + (gap - label_width) / 2;
                        canvas.put_str(row, start, &label, Class::EdgeLabel);
                    } else {
                        canvas.put_str(row + 1, left + 1, &label, Class::EdgeLabel);
                        row += 1;
                    }
                }
                row += 1;
            }
        }
    }

    let rows = canvas.into_rows();
    let width = rows
        .iter()
        .map(|row| {
            row.iter()
                .map(|span| span.text.chars().count())
                .sum::<usize>()
        })
        .max()
        .unwrap_or(0);
    if width == 0 {
        return None;
    }
    Some(Art {
        lines: rows,
        width,
        warnings,
    })
}

/// A node's outline: rows of (char, class) already drawn to shape.
struct NodeBox {
    rows: Vec<Vec<(char, Class)>>,
    width: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    Rect,
    Round,
    Rhombus,
    DoubleRound,
    Asymmetric,
}

fn make_box(label: &str, shape: Shape) -> NodeBox {
    let label = if label.is_empty() { " " } else { label };
    let len = label.chars().count();
    match shape {
        Shape::DoubleRound => NodeBox {
            rows: vec![
                vec![('(', Class::Border); 2]
                    .into_iter()
                    .chain(
                        [(' ', Class::Text)]
                            .into_iter()
                            .chain(label.chars().map(|c| (c, Class::Text))),
                    )
                    .chain(vec![
                        (' ', Class::Text),
                        (')', Class::Border),
                        (')', Class::Border),
                    ])
                    .collect(),
            ],
            width: len + 6,
        },
        Shape::Asymmetric => {
            let width = len + 3;
            NodeBox {
                rows: vec![
                    (0..width)
                        .map(|col| {
                            if col == 0 {
                                ('┌', Class::Border)
                            } else if col + 1 == width {
                                ('╲', Class::Border)
                            } else {
                                ('─', Class::Border)
                            }
                        })
                        .collect(),
                    (0..width)
                        .map(|col| {
                            if col == 0 {
                                ('│', Class::Border)
                            } else if col + 1 == width {
                                ('╲', Class::Border)
                            } else {
                                (' ', Class::None)
                            }
                        })
                        .collect(),
                    (0..width)
                        .map(|col| {
                            if col == 0 {
                                ('└', Class::Border)
                            } else {
                                ('─', Class::Border)
                            }
                        })
                        .collect(),
                ],
                width,
            }
        }
        Shape::Rhombus => {
            let inner = len.max(1);
            let width = inner + 2;
            let mut top = vec![('╱', Class::Border)];
            top.extend((0..inner).map(|_| ('─', Class::Border)));
            top.push(('╲', Class::Border));
            let mut middle = vec![('│', Class::Border)];
            middle.extend(label.chars().map(|c| (c, Class::Text)));
            while middle.len() + 1 < width {
                middle.push((' ', Class::None));
            }
            middle.push(('│', Class::Border));
            let mut bottom = vec![('╲', Class::Border)];
            bottom.extend((0..inner).map(|_| ('─', Class::Border)));
            bottom.push(('╱', Class::Border));
            NodeBox {
                rows: vec![top, middle, bottom],
                width,
            }
        }
        _ => {
            let (tl, tr, bl, br) = if shape == Shape::Round {
                ('╭', '╮', '╰', '╯')
            } else {
                ('┌', '┐', '└', '┘')
            };
            let width = len + 2;
            let mut top = Vec::with_capacity(width);
            top.push((tl, Class::Border));
            top.extend((1..width - 1).map(|_| ('─', Class::Border)));
            top.push((tr, Class::Border));
            let mut middle = Vec::with_capacity(width);
            middle.push(('│', Class::Border));
            middle.extend(label.chars().map(|c| (c, Class::Text)));
            middle.push(('│', Class::Border));
            let mut bottom = Vec::with_capacity(width);
            bottom.push((bl, Class::Border));
            bottom.extend((1..width - 1).map(|_| ('─', Class::Border)));
            bottom.push((br, Class::Border));
            NodeBox {
                rows: vec![top, middle, bottom],
                width,
            }
        }
    }
}

fn paint_box(canvas: &mut Canvas, kase: &NodeBox, row: usize, col: usize) {
    for (dr, line) in kase.rows.iter().enumerate() {
        for (dc, (ch, class)) in line.iter().enumerate() {
            canvas.put(row + dr, col + dc, *ch, *class);
        }
    }
}

/// Parse `id[label]`-shaped node syntax; `bare` also accepts a plain id.
fn parse_node(text: &str, bare: bool) -> Option<(String, String, Shape)> {
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
const FLOW_ARROWS: &[&str] = &["-.->", "==>", "-->", "---", "--x", "--o"];

/// Split `A -->|label| B` / `A -- label --> B` style statements.
fn split_edge(statement: &str) -> Option<(&str, &str, String, &str)> {
    let bytes: Vec<char> = statement.chars().collect();
    let mut depth = 0i32;
    let mut at = None;
    let mut i = 0;
    'scan: while i < bytes.len() {
        match bytes[i] {
            '[' | '(' | '{' => depth += 1,
            ']' | ')' | '}' => depth -= 1,
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
    Some((leak(left.trim()), arrow, label, leak(right)))
}

/// Own a &str for the edge parser's returns (statements are short and the
/// diagram is small; the allocation keeps the split logic honest).
fn leak(text: &str) -> &'static str {
    text.to_string().leak()
}

fn render_flow(
    statements: &[&str],
    direction: Direction,
    title: Option<String>,
    mut warnings: Vec<String>,
) -> Option<Art> {
    let mut order: Vec<String> = Vec::new();
    let mut labels: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut shapes: std::collections::HashMap<String, Shape> = std::collections::HashMap::new();
    let mut edges: Vec<(String, String, String, char)> = Vec::new();
    let mut in_subgraph = false;

    let upsert = |id: &str,
                  label: Option<&str>,
                  shape: Option<Shape>,
                  order: &mut Vec<String>,
                  labels: &mut std::collections::HashMap<String, String>,
                  shapes: &mut std::collections::HashMap<String, Shape>| {
        if !order.iter().any(|existing| existing == id) {
            order.push(id.to_string());
            labels.insert(id.to_string(), label.unwrap_or(id).to_string());
            shapes.insert(id.to_string(), shape.unwrap_or(Shape::Rect));
        } else if let Some(label) = label {
            // A bare `A --> B` reference names the node; it does not
            // overwrite the label an earlier definition gave it.
            if label != id {
                labels.insert(id.to_string(), label.to_string());
            }
        }
    };

    for statement in statements {
        let statement = statement.trim();
        if in_subgraph {
            if statement == "end" {
                in_subgraph = false;
            }
            continue;
        }
        if statement.starts_with("subgraph") {
            in_subgraph = true;
            warnings.push("subgraph is flattened".to_string());
            continue;
        }
        if statement.starts_with("classDef")
            || statement.starts_with("class ")
            || statement.starts_with("style ")
            || statement.starts_with("linkStyle")
            || statement.starts_with("click ")
        {
            continue;
        }
        if let Some((left, arrow, label, right)) = split_edge(statement) {
            let head = if arrow == "---" {
                ' '
            } else if arrow.ends_with('x') {
                '✕'
            } else if arrow.ends_with('o') {
                '○'
            } else {
                '▶'
            };
            let from = parse_node(left, true)?;
            upsert(
                &from.0,
                Some(&from.1),
                Some(from.2),
                &mut order,
                &mut labels,
                &mut shapes,
            );
            let to = parse_node(right, true)?;
            upsert(
                &to.0,
                Some(&to.1),
                Some(to.2),
                &mut order,
                &mut labels,
                &mut shapes,
            );
            if from.0 == to.0 {
                warnings.push(format!("self loop on `{}`", from.0));
                continue;
            }
            edges.push((from.0, to.0, label, head));
            continue;
        }
        if let Some((id, label, shape)) = parse_node(statement, false) {
            upsert(
                &id,
                Some(&label),
                Some(shape),
                &mut order,
                &mut labels,
                &mut shapes,
            );
            continue;
        }
        warnings.push(format!("unsupported statement `{statement}`"));
    }

    if order.is_empty() {
        return None;
    }
    let index_of = |id: &str| order.iter().position(|existing| existing == id);
    let n = order.len();

    // Longest-path layering (relaxation), cycle-aware.
    let mut layer = vec![0usize; n];
    for _ in 0..n {
        let mut changed = false;
        for (from, to, _, _) in &edges {
            if let (Some(u), Some(v)) = (index_of(from), index_of(to))
                && layer[v] < layer[u] + 1
            {
                layer[v] = layer[u] + 1;
                changed = true;
            }
        }
        if !changed {
            break;
        }
        if n > 0 && last_round_might_cycle(&edges, &layer, &index_of) {
            warnings.push("cycle detected".to_string());
            break;
        }
    }

    let mut canvas = Canvas::new();
    let mut row = 0usize;
    if let Some(title) = &title {
        canvas.put_str(0, 0, title, Class::Title);
        row = 2;
    }

    let max_layer = layer.iter().copied().max().unwrap_or(0);
    let mut columns: Vec<Vec<usize>> = vec![Vec::new(); max_layer + 1];
    for (node, at) in order.iter().enumerate() {
        let _ = at;
        columns[layer[node]].push(node);
    }
    let boxes: Vec<NodeBox> = order
        .iter()
        .map(|id| make_box(&labels[id], shapes[id]))
        .collect();

    match direction {
        Direction::TopDown => {
            let gap = columns
                .iter()
                .map(|nodes| {
                    nodes.iter().map(|n| boxes[*n].width).sum::<usize>()
                        + 6 * nodes.len().saturating_sub(1)
                })
                .max()
                .unwrap_or(0)
                .max(10);
            // Per-layer x placement: each layer centered in the run.
            let mut layer_x: Vec<Vec<usize>> = Vec::with_capacity(columns.len());
            let mut layer_rows: Vec<usize> = Vec::with_capacity(columns.len());
            for nodes in &columns {
                let used: usize = nodes.iter().map(|n| boxes[*n].width + 6).sum::<usize>();
                let mut x = gap.saturating_sub(used) / 2;
                let mut placements = Vec::with_capacity(nodes.len());
                for node in nodes {
                    placements.push(x);
                    x += boxes[*node].width + 6;
                }
                layer_x.push(placements);
                layer_rows.push(
                    nodes
                        .iter()
                        .map(|n| boxes[*n].rows.len())
                        .max()
                        .unwrap_or(1),
                );
            }
            let mut y = row;
            let mut node_center = vec![0usize; n];
            for (depth, nodes) in columns.iter().enumerate() {
                for (slot, node) in nodes.iter().enumerate() {
                    paint_box(&mut canvas, &boxes[*node], y, layer_x[depth][slot]);
                    node_center[*node] = layer_x[depth][slot] + boxes[*node].width / 2;
                }
                y += layer_rows[depth];
                if depth + 1 < columns.len() {
                    let connectors = edges
                        .iter()
                        .filter(|(from, to, _, _)| {
                            index_of(from).is_some_and(|u| layer[u] == depth)
                                && index_of(to).is_some_and(|v| layer[v] == depth + 1)
                        })
                        .count();
                    let mut seen: std::collections::HashMap<usize, usize> =
                        std::collections::HashMap::new();
                    let gap_top = y;
                    y += 2;
                    for (from, to, label, head) in &edges {
                        let (Some(u), Some(v)) = (index_of(from), index_of(to)) else {
                            continue;
                        };
                        if layer[u] != depth || layer[v] != depth + 1 {
                            continue;
                        }
                        let slot = *seen.entry(depth).or_insert(0);
                        seen.insert(depth, slot + 1);
                        let spread = (slot as isize - (connectors as isize - 1) / 2) * 3;
                        let uc = node_center[u];
                        let vc =
                            (node_center[v] as isize + spread).clamp(1, gap as isize - 2) as usize;
                        if uc == vc {
                            canvas.put(gap_top + 1, vc, *head, Class::Edge);
                        } else {
                            canvas.put(gap_top, uc, '│', Class::Edge);
                            canvas.hline(gap_top + 1, uc.min(vc), uc.max(vc), '─', Class::Edge);
                            canvas.put(gap_top, uc, '│', Class::Edge);
                            canvas.put(gap_top + 1, vc, *head, Class::Edge);
                        }
                        if !label.is_empty()
                            && label.chars().count() + 2 < uc.abs_diff(vc).max(4) + 3
                        {
                            let at = uc.max(vc) + 1;
                            canvas.put_str(gap_top, at, label, Class::EdgeLabel);
                        }
                    }
                }
            }
        }
        Direction::LeftRight => {
            // One column per layer; boxes share a middle row.
            let column_gap = 6;
            let mut xs: Vec<usize> = Vec::with_capacity(columns.len());
            let mut x = row;
            for nodes in &columns {
                xs.push(x);
                let widest = nodes.iter().map(|n| boxes[*n].width).max().unwrap_or(1);
                x += widest + column_gap;
            }
            let mid = columns
                .iter()
                .flatten()
                .map(|n| boxes[*n].rows.len())
                .max()
                .unwrap_or(1)
                / 2;
            let mut slot_y: Vec<Vec<usize>> = Vec::with_capacity(columns.len());
            for nodes in &columns {
                let mut y = row;
                let mut placements = Vec::with_capacity(nodes.len());
                for node in nodes {
                    placements.push(y);
                    y += boxes[*node].rows.len() + 2;
                }
                slot_y.push(placements);
            }
            let mut node_y = vec![0usize; n];
            for (depth, nodes) in columns.iter().enumerate() {
                for (slot, node) in nodes.iter().enumerate() {
                    let widest = columns[depth]
                        .iter()
                        .map(|n| boxes[*n].width)
                        .max()
                        .unwrap_or(1);
                    let offset = (widest - boxes[*node].width) / 2;
                    let box_x = xs[depth] + offset;
                    paint_box(&mut canvas, &boxes[*node], slot_y[depth][slot], box_x);
                    node_y[*node] = slot_y[depth][slot];
                }
            }
            for (from, to, label, head) in &edges {
                let (Some(u), Some(v)) = (index_of(from), index_of(to)) else {
                    continue;
                };
                if layer[u] == layer[v] {
                    continue; // same-column edges fall out of the subset
                }
                let (left_depth, right_depth) = (layer[u].min(layer[v]), layer[u].max(layer[v]));
                let (left, right) = if layer[u] < layer[v] { (u, v) } else { (v, u) };
                let head_row = node_y[left] + mid.min(boxes[left].rows.len() - 1);
                let left_edge = xs[left_depth] + boxes[left].width;
                let right_edge = xs[right_depth];
                for col in left_edge..right_edge {
                    canvas.ensure(head_row, col);
                    if canvas.cells[head_row][col] == ' ' {
                        canvas.put(head_row, col, '─', Class::Edge);
                    }
                }
                if layer[u] < layer[v] {
                    canvas.put(head_row, right_edge.saturating_sub(1), *head, Class::Edge);
                } else {
                    canvas.put(head_row, left_edge, *head, Class::Edge);
                }
                let _ = (left, right);
                if !label.is_empty() {
                    let gap_width = right_edge.saturating_sub(left_edge);
                    let at = if 1 + label.chars().count() + 1 < gap_width {
                        left_edge + 1
                    } else {
                        right_edge + 1
                    };
                    canvas.put_str(head_row + 1, at, label, Class::EdgeLabel);
                }
            }
            let _ = mid;
        }
    }

    let rows = canvas.into_rows();
    let width = rows
        .iter()
        .map(|row| {
            row.iter()
                .map(|span| span.text.chars().count())
                .sum::<usize>()
        })
        .max()
        .unwrap_or(0);
    if width == 0 {
        return None;
    }
    Some(Art {
        lines: rows,
        width,
        warnings,
    })
}

/// One more relaxation pass would still move a layer: the graph has a
/// cycle (pi's grok-mermaid warns rather than loops).
fn last_round_might_cycle(
    edges: &[(String, String, String, char)],
    layer: &[usize],
    index_of: &dyn Fn(&str) -> Option<usize>,
) -> bool {
    edges
        .iter()
        .any(|(from, to, _, _)| match (index_of(from), index_of(to)) {
            (Some(u), Some(v)) => layer[v] <= layer[u],
            _ => false,
        })
        && edges
            .iter()
            .any(|(from, to, _, _)| match (index_of(from), index_of(to)) {
                (Some(u), Some(v)) => layer[v] < layer[u] + 1,
                _ => false,
            })
}

#[cfg(test)]
mod tests;
