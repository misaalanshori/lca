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

// Verifies: gh #325 defect 2 - an asymmetric node (`id>label]`) shows its label.
//
// The defect was that `Shape::Asymmetric` hand-built its middle row as spaces, so
// the node rendered as an empty box. The row asserts the label text *and* the box
// character that proves it is inside the border rather than dumped beside it.
#[test]
fn an_asymmetric_node_renders_its_label() {
    let art = render("flowchart TD\n  A>Asym label] --> B[Plain]\n").expect("renders");
    let text = all_text(&art);
    assert!(
        text.contains("Asym label"),
        "the asymmetric node's label is drawn: {text}"
    );
    let label_row = art
        .lines
        .iter()
        .map(|row| {
            row.iter()
                .map(|span| span.text.as_str())
                .collect::<String>()
        })
        .find(|line| line.contains("Asym label"))
        .expect("a row carries the label");
    assert!(
        label_row.starts_with('│'),
        "and it sits inside the left border, not outside it: {label_row:?}"
    );
    assert!(
        label_row.ends_with('╲'),
        "with the slanted right border intact: {label_row:?}"
    );
}

// Verifies: gh #325 - an asymmetric node on the **left** of an edge splits too.
//
// `>` both opens an asymmetric node and ends an arrow, so the edge scanner read
// `A>label] --> B` as one unterminated node and never found the arrow - the label
// swallowed the rest of the statement. A node on the *right* of an edge worked,
// which is what made the bug look like a shape problem rather than a scanner one.
#[test]
fn an_asymmetric_node_on_either_side_of_an_edge_renders() {
    for source in [
        "flowchart TD\n  A>Left asym] --> B[Right rect]\n",
        "flowchart TD\n  A[Left rect] --> B>Right asym]\n",
    ] {
        let art = render(source).expect("renders");
        let text = all_text(&art);
        assert!(
            text.contains("Left asym") || text.contains("Right asym"),
            "the asymmetric label survives on its side of the edge ({source:?}): {text}"
        );
        // The pre-fix signature: the `>` was not counted as an opener, so `]`
        // drove the depth negative and the arrow was scanned as if it were inside
        // the node - the label then swallowed the rest of the statement and the
        // edge drew nothing. Asserting on the arrow text *and* the box count is
        // what distinguishes "one node drawn" from "two, connected".
        assert!(
            !text.contains("-->"),
            "and the edge syntax itself is consumed, not swallowed into a label \
             ({source:?}): {text}"
        );
        assert_eq!(
            text.matches('┌').count(),
            2,
            "both nodes are drawn ({source:?}): {text}"
        );
    }
}

// Verifies: gh #325 brief point 7 - **every** shape renders its label.
//
// A table over the variants, because the defect was one shape being written
// differently from the rest: a row per shape would have been written by copying
// the working shapes and would have missed `Asymmetric` the same way the code did.
#[test]
fn every_shape_renders_its_label() {
    // (mermaid source for the shape, the label it must show)
    let cases = [
        ("flowchart TD\n  N[Rect label]\n", "Rect label"),
        ("flowchart TD\n  N(Round label)\n", "Round label"),
        ("flowchart TD\n  N{Decide label}\n", "Decide label"),
        ("flowchart TD\n  N((Double label))\n", "Double label"),
        // Gh #325: on both sides of an edge, not only alone - the `>` opener also
        // ends arrows, so a lone node never reaches the scanner path that got it
        // wrong (the mutation that removed the `is_asymmetric_open` rule stayed
        // green against a lone-node-only table).
        ("flowchart TD\n  N>Asym label] --> M[End]\n", "Asym label"),
        ("flowchart TD\n  M[Start] --> N>Asym label]\n", "Asym label"),
        // `[[...]]` maps to Rect in this subset but is its own input form.
        ("flowchart TD\n  N[[Subroutine]]\n", "Subroutine"),
    ];
    for (source, label) in cases {
        let art = render(source).unwrap_or_else(|| panic!("parses: {source:?}"));
        let text = all_text(&art);
        assert!(
            text.contains(label),
            "shape from {source:?} must render {label:?}: {text}"
        );
    }
}

// Verifies: gh #325 brief point 6 - a label wider than an asymmetric box's
// interior **widens the box**; it is never truncated.
//
// The policy is stated rather than incidental: an asymmetric box derives its width
// from the label (`label + 3` for the slanted border), so there is no
// interior-overflow case to handle. A diagram that then exceeds the pane is the
// caller's existing rule - `markdown` falls back to the fenced source when
// `art.width > inner_width` - which is where a width decision belongs, because a
// box cannot know the window. This row pins the widening so a future
// hand-built interior cannot silently start cutting.
#[test]
fn an_asymmetric_label_wider_than_the_box_widens_it_rather_than_truncating() {
    let long = "A very long asymmetric label indeed";
    let art = render(&format!("flowchart TD\n  N>{long}]\n")).expect("renders");
    let text = all_text(&art);
    assert!(
        text.contains(long),
        "the whole label is drawn, not truncated: {text}"
    );
    // The top border spans the label plus the two border columns and the one cell
    // of padding the slant costs.
    let top = art
        .lines
        .iter()
        .map(|row| row.iter().map(|s| s.text.as_str()).collect::<String>())
        .find(|line| line.starts_with('┌'))
        .expect("the top border");
    assert!(
        top.chars().count() >= long.chars().count() + 2,
        "the box grew to hold it: border {:?} vs label {}",
        top,
        long.chars().count()
    );
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

// Verifies: M4 - LR direction lays its columns out left to right (RL
// renders the same left-to-right layout; the subset does not mirror).
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
