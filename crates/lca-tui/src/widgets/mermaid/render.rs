//! How it is drawn: a sequence diagram's lanes and messages, and a flow graph's
//! layered boxes and edges.
//!
//! Split from the barrel for gate 11; the code, its comments and its rows are
//! unchanged. Both paint into `syntax.rs`'s canvas from what that module
//! decided, and both answer in spans - the caller decides whether the width
//! fits.

use super::syntax::{ARROWS, Canvas, Direction, NodeBox, Shape, parse_node, split_edge};
use super::{Art, Class};

pub(super) fn render_sequence(
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

/// Build a node's outline for `shape`, with `label` placed inside it.
///
/// **Every shape places its label the same way** (gh #325). A shape differs in
/// its **borders** - corners, slants, doubled edges - and never in how the label
/// goes in, so [`label_row`] is the one place a label is positioned. The previous
/// version hand-built each interior instead, which is why `Asymmetric` shipped
/// with a middle row of spaces and no label at all: one shape was written
/// differently from the rest, and nothing said it had to agree.
///
/// **There is no inside-the-box overflow case, by construction.** The width is
/// *derived* from the label (`label width + border overhead`), so the label always
/// fits and the box widens to hold it - the same policy every other shape already
/// had. A diagram that then exceeds the pane is the caller's existing rule
/// (`markdown` falls back to the fenced source when `art.width > inner_width`),
/// which is where a width decision belongs: a box cannot know the window.
pub(super) fn make_box(label: &str, shape: Shape) -> NodeBox {
    let label = if label.is_empty() { " " } else { label };
    let len = label.chars().count();
    match shape {
        // A capsule is one row: the borders are `((` and `))` with air inside.
        Shape::DoubleRound => {
            let width = len + 6;
            NodeBox {
                rows: vec![label_row(
                    label,
                    &[
                        ('(', Class::Border),
                        ('(', Class::Border),
                        (' ', Class::Text),
                    ],
                    &[
                        (' ', Class::Text),
                        (')', Class::Border),
                        (')', Class::Border),
                    ],
                    width,
                )],
                width,
            }
        }
        // Asymmetric (`id>label]`): a vertical left edge and a slanted right one.
        // The slant costs a column on the right, which is why the label sits
        // against the left border with one cell of air before the slant.
        Shape::Asymmetric => {
            let width = len + 3;
            let mut top = vec![('┌', Class::Border)];
            top.extend((1..width - 1).map(|_| ('─', Class::Border)));
            top.push(('╲', Class::Border));
            let mut bottom = vec![('└', Class::Border)];
            bottom.extend((1..width).map(|_| ('─', Class::Border)));
            NodeBox {
                rows: vec![
                    top,
                    label_row(
                        label,
                        &[('│', Class::Border)],
                        &[('╲', Class::Border)],
                        width,
                    ),
                    bottom,
                ],
                width,
            }
        }
        // A rhombus: slanted top and bottom, straight sides.
        Shape::Rhombus => {
            let width = len + 2;
            let mut top = vec![('╱', Class::Border)];
            top.extend((0..len).map(|_| ('─', Class::Border)));
            top.push(('╲', Class::Border));
            let mut bottom = vec![('╲', Class::Border)];
            bottom.extend((0..len).map(|_| ('─', Class::Border)));
            bottom.push(('╱', Class::Border));
            NodeBox {
                rows: vec![
                    top,
                    label_row(
                        label,
                        &[('│', Class::Border)],
                        &[('│', Class::Border)],
                        width,
                    ),
                    bottom,
                ],
                width,
            }
        }
        Shape::Rect | Shape::Round => {
            let (tl, tr, bl, br) = if shape == Shape::Round {
                ('╭', '╮', '╰', '╯')
            } else {
                ('┌', '┐', '└', '┘')
            };
            let width = len + 2;
            let mut top = vec![(tl, Class::Border)];
            top.extend((1..width - 1).map(|_| ('─', Class::Border)));
            top.push((tr, Class::Border));
            let mut bottom = vec![(bl, Class::Border)];
            bottom.extend((1..width - 1).map(|_| ('─', Class::Border)));
            bottom.push((br, Class::Border));
            NodeBox {
                rows: vec![
                    top,
                    label_row(
                        label,
                        &[('│', Class::Border)],
                        &[('│', Class::Border)],
                        width,
                    ),
                    bottom,
                ],
                width,
            }
        }
    }
}

/// The one row a label is placed on: `left`, the label, padding, then `right`.
///
/// Padding carries [`Class::None`] because it is background, not text - the
/// caller's `into_rows` trims trailing empty spans, so an unpadded right edge
/// would still be correct, but an *interior* gap that claims `Text` would be
/// styled as a word.
pub(super) fn label_row(
    label: &str,
    left: &[(char, Class)],
    right: &[(char, Class)],
    width: usize,
) -> Vec<(char, Class)> {
    let mut row = Vec::with_capacity(width);
    row.extend(left.iter().copied());
    row.extend(label.chars().map(|ch| (ch, Class::Text)));
    while row.len() + right.len() < width {
        row.push((' ', Class::None));
    }
    row.extend(right.iter().copied());
    row
}

pub(super) fn paint_box(canvas: &mut Canvas, kase: &NodeBox, row: usize, col: usize) {
    for (dr, line) in kase.rows.iter().enumerate() {
        for (dc, (ch, class)) in line.iter().enumerate() {
            canvas.put(row + dr, col + dc, *ch, *class);
        }
    }
}

pub(super) fn render_flow(
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
            let from = parse_node(&left, true)?;
            upsert(
                &from.0,
                Some(&from.1),
                Some(from.2),
                &mut order,
                &mut labels,
                &mut shapes,
            );
            let to = parse_node(&right, true)?;
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
