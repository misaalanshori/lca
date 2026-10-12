//! Tool-result images end to end (gh #113, pi's `ImageContent` in tool
//! results): `read` returns image bytes, the record persists hashes,
//! assembly emits image blocks the provider receives, and a resume
//! re-sends them from the store.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
mod common;

use common::*;
use lca_protocol::{ContentBlock, MessageRole, Record};
use lca_session::ViewMode;
use lca_testkit::{FakeProvider, fake_usage};

const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 1, 2, 3, 4];

fn tool_images(record: &Record) -> Vec<String> {
    match record {
        Record::ToolResult { images, .. } => images.clone(),
        other => panic!("a tool result, not {}", other.type_tag()),
    }
}

// Verifies: gh #113 - "look at shot.png" sends the image bytes to the
// model; the record keeps hashes (never bytes), the file lands in the
// attachment store, and a resumed turn re-sends the image from the
// store without re-reading the tool output.
#[tokio::test]
async fn a_read_image_reaches_the_model_and_survives_resume() {
    let provider = FakeProvider::builder()
        .turn(|t| {
            t.tool_call("read", r#"{"path":"shot.png"}"#)
                .usage(fake_usage(100, 20, 0, 100))
        })
        .turn(|t| {
            t.text("a red square")
                .usage(fake_usage(200, 30, 100, 100))
        })
        .turn(|t| {
            t.text("still a red square")
                .usage(fake_usage(200, 30, 100, 100))
        })
        .build();
    let mut h = harness("tool-image", provider, default_config());
    std::fs::write(h.project.join("shot.png"), PNG).expect("write fixture");
    let mut sink = CollectingSink::default();
    let mut prompt = Prompt {
        answers: vec![],
        asked: vec![],
    };
    let outcome = turn(&mut h, "look at shot.png", &mut sink, &mut prompt).await;
    assert_eq!(outcome.status, lca_core::TurnStatus::Ok);

    // The second request carries the tool result with image blocks.
    let requests = h.provider.requests();
    assert_eq!(requests.len(), 2);
    let result_message = requests[1]
        .messages
        .iter()
        .find(|message| message.role == MessageRole::Tool)
        .expect("tool result message");
    let seen: Vec<&[u8]> = result_message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Image { bytes, .. } => Some(bytes.as_slice()),
            _ => None,
        })
        .collect();
    assert_eq!(seen, vec![PNG], "the model sees the image bytes");

    // The log keeps hashes; the bytes live in the attachment store.
    let log = h
        .store
        .read_with(&h.session, ViewMode::Audit)
        .expect("read");
    let hashes: Vec<String> = log
        .records
        .iter()
        .filter(|record| matches!(record, Record::ToolResult { .. }))
        .flat_map(tool_images)
        .collect();
    assert_eq!(hashes.len(), 1, "one persisted image hash");
    let file = h.session.dir().join("attachments").join(&hashes[0]);
    assert_eq!(std::fs::read(&file).expect("stored"), PNG);

    // Resume: a fresh turn re-sends the same image from the store.
    let outcome = turn(&mut h, "again", &mut sink, &mut prompt).await;
    assert_eq!(outcome.status, lca_core::TurnStatus::Ok);
    let requests = h.provider.requests();
    assert_eq!(requests.len(), 3);
    let resent: Vec<&[u8]> = requests[2]
        .messages
        .iter()
        .filter(|message| message.role == MessageRole::Tool)
        .flat_map(|message| message.content.iter())
        .filter_map(|block| match block {
            ContentBlock::Image { bytes, .. } => Some(bytes.as_slice()),
            _ => None,
        })
        .collect();
    assert_eq!(resent, vec![PNG], "resume re-sends the stored image");
}
