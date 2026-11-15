//! Gh #602: a resumed session draws the tool images it lost.
//!
//! # The defect
//!
//! Three record shapes carry attachment hashes, and the replay path resolved two
//! of them: `Record::User`'s `attachments` and `Record::Assistant`'s inline
//! `ContentBlock::Image`. The third - `Record::ToolResult`'s `images` - was
//! destructured as `nested, ..` and never loaded, so a session that showed a
//! picture a tool returned came back with the tool card and **no image**, while
//! the bytes were still on disk (`gc`'s `referenced_attachments` walks them) and
//! `/export` named them. Pre-existing: before gh #599 the same frame showed the
//! placeholder, which is why nobody noticed until the live path started drawing.
//!
//! # What makes the rows honest
//!
//! Every row asserts an **observable** in the painted frame: a header's `f=`, the
//! payload's own magic bytes, the entry count, the document row order, or the
//! derivative's encode census. None of them assert "the closure ran".
//!
//! The rows live here rather than in `transcript/replay_records.rs`'s own test
//! module because they need the same fixture the gh #599 rows built - a **real**
//! `read` under a vision model's resize profile, so the persisted wire bytes are
//! a JPEG and the display derivative is what makes it drawable at all.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::sync::Arc;

use lca_protocol::{FORMAT_VERSION, Record, ToolResultStatus};
use lca_tui::widgets::image::{ImageInfo, ImageProtocol};
use lca_ui::state::LoadAttachment;
use lca_ui::transcript::Transcript;

/// A photo-like PNG: continuous tones, so the resize path's JPEG is a real JPEG.
fn photo_png() -> Vec<u8> {
    let rgb = image::RgbImage::from_fn(2000, 1333, |x, y| {
        image::Rgb([
            (x * 200 / 2000) as u8,
            (y * 200 / 1333) as u8,
            ((x + y) * 200 / 3333) as u8,
        ])
    });
    let mut png = Vec::new();
    rgb.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .expect("encode the fixture");
    png
}

/// The **real** `read` result for `path` under a vision model's resize profile -
/// the wire bytes a turn actually persists.
fn resized_read(path: &std::path::Path) -> lca_protocol::ToolResult {
    use std::sync::Arc as StdArc;
    let workspace = path.parent().expect("a parent directory");
    let mut exec = lca_tools::ToolExecutor::new(
        StdArc::new(lca_tools::NativeOps::default()),
        workspace.to_path_buf(),
        workspace.to_path_buf(),
        65536,
        Some(std::time::Duration::from_secs(30)),
    );
    exec.set_image_policy(lca_tools::ImagePolicy {
        vision: lca_tools::ImageVision::Vision,
        resize: Some(lca_protocol::ImageResize {
            max_width: 1280,
            max_height: 1280,
            max_bytes: 4_500_000,
        }),
    });
    let call = lca_protocol::ToolCall {
        call_id: "call-read".to_string(),
        name: "read".to_string(),
        arguments: serde_json::json!({ "path": path.file_name().unwrap().to_string_lossy() })
            .to_string(),
        parent_call_id: None,
    };
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
        .block_on(async {
            let mut sink = |_: &[u8]| {};
            exec.execute(&call, &mut sink, &lca_tools::CancelFlag::new())
                .await
        })
}

/// The hash the record carries for the one image, and the bytes a loader
/// answers for it.
fn persisted_image(name: &str) -> (String, String, Vec<u8>) {
    let dir = lca_testkit::scratch_path(&format!("gh602-{name}"));
    std::fs::create_dir_all(dir.join("project")).expect("mkdir");
    let path = dir.join("project").join("photo.png");
    std::fs::write(&path, photo_png()).expect("write the fixture");
    let result = resized_read(&path);
    assert_eq!(result.status, ToolResultStatus::Ok);
    let image = result.images.first().expect("one image").clone();
    assert_eq!(image.media_type, "image/jpeg", "the wire bytes are a JPEG");
    // The hash is opaque to the replay - it only has to round-trip through the
    // record and back through the loader, exactly as the store's does.
    let hash = "gh602imagehash".to_string();
    (hash, image.media_type, image.bytes)
}

/// The persisted pair: the `tool-call` and the `tool-result` naming `hash`.
fn records(hash: &str, content: &str) -> Vec<Record> {
    vec![
        Record::ToolCall {
            v: FORMAT_VERSION,
            ts: 1,
            id: "call-1".into(),
            parent: None,
            call_id: "call-read".into(),
            name: "read".into(),
            arguments: "{\"path\":\"photo.png\"}".into(),
            source: lca_protocol::ToolSource::Builtin,
        },
        Record::ToolResult {
            v: FORMAT_VERSION,
            ts: 2,
            id: "result-1".into(),
            parent: Some("call-1".into()),
            call_id: "call-read".into(),
            status: ToolResultStatus::Ok,
            content: Some(content.to_string()),
            attachment: None,
            truncated: false,
            duration_ms: Some(300),
            exit_code: None,
            full_output_path: None,
            nested: Vec::new(),
            images: vec![hash.to_string()],
        },
    ]
}

/// The kitty transmission headers in a frame, as `(f=, c=, r=)`.
fn kitty_headers(frame: &str) -> Vec<(Option<u32>, u32, u32, bool)> {
    let mut out = Vec::new();
    let mut rest = frame;
    while let Some(at) = rest.find("\x1b_G") {
        let tail = &rest[at + 3..];
        let Some(end) = tail.find(';') else {
            break;
        };
        let params = &tail[..end];
        let (mut format, mut columns, mut rows) = (None, 0, 0);
        for param in params.split(',') {
            let Some((key, value)) = param.split_once('=') else {
                continue;
            };
            if let Ok(number) = value.parse::<u32>() {
                match key {
                    "f" => format = Some(number),
                    "c" => columns = number,
                    "r" => rows = number,
                    _ => {}
                }
            }
        }
        if params.contains("a=T") || params.contains("a=p") {
            out.push((format, columns, rows, params.contains("a=T")));
        }
        rest = &tail[end..];
    }
    out
}

/// The base64 payload of the first kitty transmission, decoded (every chunk -
/// the protocol splits a large upload into `m=1` continuations).
fn kitty_payload(frame: &str) -> Option<Vec<u8>> {
    if !frame.contains("\x1b_Ga=T") {
        return None;
    }
    let mut encoded = String::new();
    let mut rest = frame;
    while let Some(at) = rest.find("\x1b_G") {
        let tail = &rest[at + 3..];
        let end = tail.find("\x1b\\")?;
        if let Some(semi) = tail[..end].find(';') {
            encoded.push_str(&tail[semi + 1..end]);
        }
        rest = &tail[end..];
    }
    lca_mcp::wire::base64_decode(&encoded).ok()
}

/// Replay the pair with a loader that answers `(media, bytes)` for `hash`, and
/// return the painted frame plus the encode census.
fn replay_with_loader(hash: &str, media: &str, bytes: &[u8]) -> (String, usize, usize) {
    let wanted = hash.to_string();
    let media = media.to_string();
    let bytes = bytes.to_vec();
    let loader: LoadAttachment =
        Arc::new(move |asked: &str| (asked == wanted).then(|| (media.clone(), bytes.clone())));
    let mut transcript = Transcript::new();
    transcript.replay_records(
        &records(hash, "[image image/jpeg, 97531 bytes]"),
        Some(&loader),
    );
    let rows =
        transcript.render_for_protocol(100, &lca_ui::theme::Theme::plain(), ImageProtocol::Kitty);
    (
        rows.join("\n"),
        transcript.image_encodes_for_test(),
        transcript.entries().len(),
    )
}

// Verifies: gh #602 row 1 - **a replayed tool-result image is drawn.** The frame
// carries a kitty transmission whose **payload's own first eight bytes** are
// `\x89PNG\r\n\x1a\n` (the display derivative's magic, not just the `f=` code -
// that is how #598/#599 shipped), and the image **entry** exists, so a
// "some row contains a sequence" assertion cannot pass on a frame that also lost
// the card. Red on `3f25c83e`: transmissions 0, encodes 0, image entries 0.
#[test]
fn a_replayed_tool_result_image_is_drawn() {
    let (hash, media, bytes) = persisted_image("drawn");
    let (frame, encodes, entries) = replay_with_loader(&hash, &media, &bytes);
    let headers = kitty_headers(&frame);
    assert_eq!(headers.len(), 1, "one transmission for one image:\n{frame}");
    assert_eq!(headers[0].0, Some(100), "kitty's PNG code");
    assert!(
        headers[0].3,
        "and it is a transmission (`a=T`), not a placement"
    );
    let payload = kitty_payload(&frame).expect("the transmission carries a payload");
    assert_eq!(
        payload.get(..8),
        Some(&b"\x89PNG\r\n\x1a\n"[..]),
        "the payload's own magic bytes say PNG"
    );
    assert_eq!(
        entries, 2,
        "one card entry and one image entry - the count is what proves the image \
         is its own entry, not text folded into the card"
    );
    assert_eq!(encodes, 1, "the derivative encoded once for this width");
}

// Verifies: gh #602 row 2 - **a hash with no file keeps the record's text.** The
// absent image is a fact, not an error to paint: no image entry, no transmission,
// and the record's own line (which already names the image) still on screen.
// This is the arm that must not swallow a broken reference.
#[test]
fn a_replayed_hash_with_no_file_keeps_the_record_text() {
    let (hash, _media, _bytes) = persisted_image("absent");
    let mut transcript = Transcript::new();
    transcript.replay_records(&records(&hash, "[image image/jpeg, 97531 bytes]"), None);
    let rows =
        transcript.render_for_protocol(100, &lca_ui::theme::Theme::plain(), ImageProtocol::Kitty);
    let frame = rows.join("\n");
    assert!(
        kitty_headers(&frame).is_empty(),
        "nothing is transmitted for a hash that does not resolve:\n{frame}"
    );
    assert_eq!(
        transcript.image_encodes_for_test(),
        0,
        "and nothing is encoded"
    );
    assert_eq!(
        transcript.entries().len(),
        1,
        "the tool card is the only entry"
    );
    // The card is collapsed by default (`Ctrl+O` expands it), so "the record's
    // text stands" is the card's own line - which is exactly what the pre-cycle
    // frame showed. The image line lives inside the card, one keystroke away, and
    // the point of the row is that *nothing* was invented in its place.
    assert!(
        frame.contains("read photo.png ok"),
        "the card's own line is what the resumed frame shows:\n{frame}"
    );
    assert!(
        !frame.contains("\x1b]1337"),
        "and no image lane was opened for a hash with no file"
    );
}

// Verifies: gh #602 row 3 - **a resumed image transmits rather than places.** The
// replayed entry's bytes are a different `Arc` than a live entry's, so the
// derivative's `Arc::ptr_eq` validation misses and re-encodes; that is the
// intended behaviour, and the row pins both halves: the census is **1** and the
// frame carries `a=T`. A placement for a texture this terminal never received
// would draw nothing on a real kitty terminal - which is the failure mode a
// shared `Arc` (or a stale placement cache) would produce.
#[test]
fn a_resumed_image_transmits_rather_than_places() {
    let (hash, media, bytes) = persisted_image("transmit");
    let (frame, encodes, _entries) = replay_with_loader(&hash, &media, &bytes);
    assert_eq!(encodes, 1, "one encode for the resumed bytes");
    assert!(
        frame.contains("\x1b_Ga=T"),
        "the frame transmits the texture:\n{frame}"
    );
    assert!(
        !frame.contains("\x1b_Ga=p"),
        "and does not merely place one the terminal never received"
    );
    // A second frame at the same width is the placement (the texture is now in
    // the terminal) - the same rule the live path follows.
    let mut transcript = Transcript::new();
    let wanted = hash.clone();
    let media = media.clone();
    let bytes = bytes.clone();
    let loader: LoadAttachment =
        Arc::new(move |asked: &str| (asked == wanted).then(|| (media.clone(), bytes.clone())));
    transcript.replay_records(&records(&hash, "[image image/jpeg]"), Some(&loader));
    let _first =
        transcript.render_for_protocol(100, &lca_ui::theme::Theme::plain(), ImageProtocol::Kitty);
    let again =
        transcript.render_for_protocol(100, &lca_ui::theme::Theme::plain(), ImageProtocol::Kitty);
    let again = again.join("\n");
    assert_eq!(
        transcript.image_encodes_for_test(),
        1,
        "the second frame reuses the derivative"
    );
    assert!(
        again.contains("\x1b_Ga=p"),
        "and places rather than re-transmits:\n{again}"
    );
}

// Verifies: gh #602 row 4 - **the image sits after its tool card.** The live loop
// pushes the image after `finish_tool_with_id`, and a resumed transcript must read
// like the one that produced it (FR-UI-7). The assertion is the **document order**
// (card row index < image block row index), so the arm cannot drift above the card.
#[test]
fn the_replayed_image_sits_after_its_tool_card() {
    let (hash, media, bytes) = persisted_image("order");
    let wanted = hash.clone();
    let media = media.clone();
    let bytes = bytes.clone();
    let loader: LoadAttachment =
        Arc::new(move |asked: &str| (asked == wanted).then(|| (media.clone(), bytes.clone())));
    let mut transcript = Transcript::new();
    transcript.replay_records(&records(&hash, "[image image/jpeg]"), Some(&loader));
    let rows =
        transcript.render_for_protocol(100, &lca_ui::theme::Theme::plain(), ImageProtocol::Kitty);
    let card = rows
        .iter()
        .position(|row| row.contains("read"))
        .expect("the tool card is painted");
    let image = rows
        .iter()
        .position(|row| row.contains("\x1b_G"))
        .expect("the image is painted");
    assert!(
        image > card,
        "the image block follows the card: card row {card}, image row {image}"
    );
}

// Verifies: gh #602 row 5 - **the two arms that already worked still do.** A user
// attachment and an assistant inline image replay in the same call as a tool
// result, so the new arm cannot disturb its neighbours (the "no silent regression
// next door" row). Each of the three drawing shapes produces exactly one
// transmission.
#[test]
fn a_user_and_an_assistant_image_still_replay_as_before() {
    let (hash, media, bytes) = persisted_image("neighbours");
    let wanted = hash.clone();
    let loader_media = media.clone();
    let loader_bytes = bytes.clone();
    let loader: LoadAttachment = Arc::new(move |asked: &str| {
        (asked == wanted).then(|| (loader_media.clone(), loader_bytes.clone()))
    });
    let png = {
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend_from_slice(&[0, 0, 0, 13]);
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&64u32.to_be_bytes());
        png.extend_from_slice(&64u32.to_be_bytes());
        png.extend_from_slice(&[0; 12]);
        png
    };
    let records = vec![
        Record::User {
            v: FORMAT_VERSION,
            ts: 1,
            id: "user-1".into(),
            parent: None,
            content: "look at this".into(),
            attachments: vec![hash.clone()],
            queue: None,
        },
        Record::Assistant {
            v: FORMAT_VERSION,
            ts: 2,
            id: "assistant-1".into(),
            parent: Some("user-1".into()),
            content: vec![
                lca_protocol::ContentBlock::Text {
                    text: "and this".into(),
                },
                lca_protocol::ContentBlock::Image {
                    media_type: "image/png".into(),
                    bytes: png.clone(),
                },
            ],
            reasoning: None,
            reasoning_signature: None,
            provider_thinking_level: None,
            model: None,
            provider: None,
            usage: None,
        },
        Record::ToolCall {
            v: FORMAT_VERSION,
            ts: 3,
            id: "call-1".into(),
            parent: Some("assistant-1".into()),
            call_id: "call-read".into(),
            name: "read".into(),
            arguments: "{}".into(),
            source: lca_protocol::ToolSource::Builtin,
        },
        Record::ToolResult {
            v: FORMAT_VERSION,
            ts: 4,
            id: "result-1".into(),
            parent: Some("call-1".into()),
            call_id: "call-read".into(),
            status: ToolResultStatus::Ok,
            content: Some("[image image/jpeg]".into()),
            attachment: None,
            truncated: false,
            duration_ms: Some(300),
            exit_code: None,
            full_output_path: None,
            nested: Vec::new(),
            images: vec![hash.clone()],
        },
    ];
    let mut transcript = Transcript::new();
    transcript.replay_records(&records, Some(&loader));
    let rows =
        transcript.render_for_protocol(100, &lca_ui::theme::Theme::plain(), ImageProtocol::Kitty);
    let frame = rows.join("\n");
    assert!(
        frame.contains("look at this") && frame.contains("and this"),
        "both text rows are there:\n{frame}"
    );
    // Three drawing shapes reach the frame. The user's attachment and the tool's
    // image carry **byte-identical payloads** here, so their derived kitty ids
    // match and the renderer's own id cache (the placement cache, gh #324 gap 1)
    // turns the second sighting into an `a=p`: two transmissions (the big tool
    // image and the assistant's small PNG) plus one placement. That is precisely
    // why this cycle added no entry-level dedupe (§2.4) - the entries stay two and
    // the upload happens once, without the transcript knowing about it.
    let headers = kitty_headers(&frame);
    assert_eq!(
        transcript
            .entries()
            .iter()
            .filter(|entry| matches!(entry, lca_ui::Entry::Image { .. }))
            .count(),
        3,
        "three image entries: the user's attachment, the assistant's inline image, \
         and the tool's result"
    );
    assert_eq!(
        headers.iter().filter(|header| header.3).count(),
        2,
        "two transmissions, and an assistant PNG among them:\n{frame}"
    );
    // And the ordering fact the live path has too: the three entries land in
    // record order (user image, assistant image, tool image), never interleaved.
    let image_rows: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter(|(_, row)| row.contains("\x1b_G"))
        .map(|(index, _)| index)
        .collect();
    assert_eq!(image_rows.len(), 3, "three image blocks: {image_rows:?}");
    assert!(
        image_rows[0] < image_rows[1] && image_rows[1] < image_rows[2],
        "in document order: {image_rows:?}"
    );
    assert!(frame.contains("read"), "and the tool card still renders");
    // The live path's own image info is unchanged by the replay: the entry holds
    // what the record said, not something re-derived.
    let info = ImageInfo::new(&media, &bytes);
    assert_eq!(info.media_type, "image/jpeg");
    assert_eq!((info.width, info.height), (Some(1280), Some(853)));
}
