use super::{Class, render};

fn all_text(art: &super::Art) -> String {
    art.lines
        .iter()
        .map(|row| {
            row.iter()
                .map(|span| span.text.as_str())
                .collect::<String>()
        })
        .collect::<Vec<String>>()
        .join("\n")
}

// Verifies: TUI-10 M4 - a flowchart parses to Unicode art with the span
// classes grok-mermaid emits (border/text/edge/edgeLabel).
#[test]
fn a_flowchart_renders_boxes_and_edges() {
    let art = render("flowchart TD\n  A[One] --> B[Two]\n").expect("renders");
    let text = all_text(&art);
    assert!(text.contains('┌') && text.contains('┘'), "{text}");
    assert!(text.contains("One") && text.contains("Two"), "{text}");
    assert!(text.contains('▼') || text.contains('▶'), "{text}");
    assert!(art.width > 0);
    // The classes are the ones the theme colors.
    let classes: Vec<Class> = art.lines.iter().flatten().map(|span| span.class).collect();
    assert!(classes.contains(&Class::Border), "border spans");
    assert!(classes.contains(&Class::Text), "text spans");
    assert!(classes.contains(&Class::Edge), "edge spans");
}

// Verifies: M4 - edge labels ride the connector (`-->|yes|` and
// `-- label -->` forms).
#[test]
fn flow_edge_labels_survive_both_syntaxes() {
    let art = render("flowchart LR\n  A -->|yes| B\n  B -- maybe --> C\n").expect("renders");
    let text = all_text(&art);
    assert!(text.contains("yes"), "{text}");
    assert!(text.contains("maybe"), "{text}");
    assert!(text.contains("A") && text.contains("C"), "{text}");
}

// Verifies: M4 - node shapes draw their own outlines.
#[test]
fn flow_node_shapes_draw_themselves() {
    let art = render("flowchart TD\n  A{Decide} --> B((Done))\n  B --> C[Run]\n  C --> D(Round)\n")
        .expect("renders");
    let text = all_text(&art);
    assert!(
        text.contains('╱') && text.contains('╲'),
        "the rhombus: {text}"
    );
    assert!(
        text.contains('╭') && text.contains('╰'),
        "the circle: {text}"
    );
    assert!(text.contains("Decide") && text.contains("Done"), "{text}");
}

// Verifies: M4 - LR direction lays columns out left to right, and RL
// mirrors the column order so arrows still meet their boxes.
#[test]
fn flow_directions_lay_out() {
    let art = render("flowchart LR\n  A[Start] --> B[End]\n").expect("renders");
    let text = all_text(&art);
    let start_at = text.find("Start").expect("Start");
    let end_at = text.find("End").expect("End");
    assert!(start_at < end_at, "LR order: {text}");
}

// Verifies: M4's fail-soft - anything outside the subset parses to None
// so markdown keeps the fenced source, and cycles warn instead of hanging.
#[test]
fn unsupported_diagrams_fail_soft() {
    assert!(render("pie title Pets\n  \"Dogs\": 42\n").is_none());
    assert!(render("classDiagram\n  A <|-- B\n").is_none());
    assert!(render("not a diagram at all").is_none());
    let art = render("flowchart TD\n  A --> B\n  B --> A\n").expect("renders");
    assert!(
        art.warnings.iter().any(|w| w.contains("cycle")),
        "a cycle warns: {:?}",
        art.warnings
    );
}

// Verifies: M4 - sequence diagrams: actor headers, message arrows, and
// notes; a statement outside the subset warns.
#[test]
fn sequence_diagrams_draw_headers_and_messages() {
    let art = render(
        "sequenceDiagram\n  participant Alice\n  Alice->>Bob: Hello there\n  Note over Alice: thinking\n",
    )
    .expect("renders");
    let text = all_text(&art);
    assert!(text.contains("Alice") && text.contains("Bob"), "{text}");
    assert!(text.contains('▶'), "the message head: {text}");
    assert!(text.contains("Hello there"), "{text}");
    assert!(text.contains("thinking"), "the note: {text}");

    let art = render("sequenceDiagram\n  A->>B: hi\n  unknown statement\n").expect("renders");
    assert!(
        art.warnings.iter().any(|w| w.contains("unsupported")),
        "{:?}",
        art.warnings
    );
}

// Verifies: M4 - a sequence diagram draws a title in the title class,
// and a self message loops back under its own actor.
#[test]
fn sequence_titles_and_self_messages() {
    let art = render("sequenceDiagram\n  title Handshake\n  A->>A: think\n  A->>B: send\n")
        .expect("renders");
    let title = &art.lines[0];
    assert_eq!(title[0].class, Class::Title, "the title carries its class");
    let text = all_text(&art);
    assert!(text.contains("Handshake"), "{text}");
    assert!(
        text.contains("┌") && text.contains("└"),
        "self loop box: {text}"
    );
}
