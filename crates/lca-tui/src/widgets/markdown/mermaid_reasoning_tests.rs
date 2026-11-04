//! gh #390 gap 2: a diagram drafted in reasoning is source, not art.
//!
//! Split from `tests.rs` (gate 11's 1,200-line ceiling): one subject, one file.

use super::tests::{plain, strip};
use super::*;
// ============================================================================
// gh #390 gap 2: a diagram drafted in reasoning is source, not art.
// ============================================================================

const DRAFT_DIAGRAM: &str = "```mermaid\nflowchart TD\n  A[One] --> B[Two]\n```";

/// The same source rendered as a reasoning run instead of an answer.
fn render_as(text: &str, message_type: MarkdownMessageType) -> Vec<String> {
    strip(&render_markdown(
        text,
        60,
        &plain(),
        &MarkdownOptions {
            message_type,
            ..MarkdownOptions::default()
        },
    ))
}

// Verifies: gh #390 gap 2 - **a mermaid fence inside a reasoning run renders raw.**
// A draft the model sketches to itself is not the answer; drawing it turns the
// thinking run into a second copy of the artefact below it. pi draws the line per
// message type (`assistant-thinking` skips the transformer) rather than by
// sniffing content, and this is the row that pins the type reaching the block
// renderer.
#[test]
fn a_mermaid_fence_inside_a_reasoning_run_stays_source() {
    let out = render_as(DRAFT_DIAGRAM, MarkdownMessageType::AssistantThinking);
    assert!(
        !out.iter()
            .any(|line| line.contains('┌') || line.contains('└')),
        "no art box in a reasoning run: {out:?}"
    );
    // The draft is still *visible* - as the fence it was written as, which is the
    // point: the user sees the model sketching, not a diagram it has not committed
    // to.
    assert!(
        out.iter().any(|line| line.contains("flowchart TD")),
        "the draft shows as source: {out:?}"
    );
    assert!(
        out.iter().any(|line| line.contains("A[One] --> B[Two]")),
        "and the statements are readable: {out:?}"
    );
}

// Verifies: gh #390 gap 2 (**the control**) - **the same fence in the answer still
// renders as art.** A skip that disabled diagrams everywhere would pass the row
// above and be a far worse defect than the one being fixed.
#[test]
fn the_same_fence_in_the_answer_still_renders_as_art() {
    let out = render_as(DRAFT_DIAGRAM, MarkdownMessageType::Assistant);
    assert!(
        out.iter().any(|line| line.contains('┌')),
        "the answer's diagram is art: {out:?}"
    );
    assert!(
        !out.iter().any(|line| line.contains("flowchart TD")),
        "and the source is replaced by the drawing: {out:?}"
    );
    // The two types genuinely differ, so this pair cannot both pass by accident.
    assert_ne!(
        render_as(DRAFT_DIAGRAM, MarkdownMessageType::AssistantThinking),
        render_as(DRAFT_DIAGRAM, MarkdownMessageType::Assistant),
        "the message type is what decides, and it decides something"
    );
}

// Verifies: gh #390 gap 2 - **a non-mermaid fence inside reasoning is unchanged.**
// The skip is about the artefact transformer, not about code in general: reasoning
// that quotes a shell command or a Rust snippet must render exactly as it does in
// an answer, or the fix would have made thinking worse to read.
#[test]
fn a_non_mermaid_fence_inside_reasoning_is_unchanged() {
    let src = "```rust\nfn main() {}\n```";
    let reasoning = render_as(src, MarkdownMessageType::AssistantThinking);
    let answer = render_as(src, MarkdownMessageType::Assistant);
    assert!(
        reasoning.iter().any(|line| line.contains("fn main")),
        "the code is shown in the reasoning run: {reasoning:?}"
    );
    assert_eq!(
        reasoning, answer,
        "a non-mermaid fence reads the same in both message types"
    );
}

// Verifies: gh #390 gap 2 - **the predicate is the one place that answers the
// question.** A future reasoning-shaped type must not have to be added to each
// transformer separately; the mermaid gate and any later one ask `is_reasoning`.
#[test]
fn the_reasoning_predicate_is_what_the_pipeline_asks() {
    assert!(MarkdownMessageType::AssistantThinking.is_reasoning());
    assert!(!MarkdownMessageType::Assistant.is_reasoning());
    assert!(!MarkdownMessageType::User.is_reasoning());
    // And the mermaid gate consults it rather than matching the variant inline,
    // so a second artefact transformer has one thing to ask.
    let source = include_str!("mod.rs");
    assert!(
        source.contains("options.message_type.is_reasoning()"),
        "the block renderer asks the predicate"
    );
}
