//! Gh #599: the kitty lane draws a **resized** image, without a second stored copy.
//!
//! # The defect this closes
//!
//! gh #598 made `kitty_format_for` honest: `image/png` → `f=100`, everything else
//! `None`, and a `None` takes the placeholder. That is right about the protocol and
//! leaves a hole: a vision model's resize is re-encoded **JPEG q80**
//! (`lca_tools::image::downscale_image`), so the common case - "a photo I pasted, this
//! model downscaled it" - painted `[image image/jpeg · 1280×853 · …]` on kitty: a
//! label where a picture belongs. iTerm2 has always worked, because its protocol
//! carries no format field and the terminal sniffs.
//!
//! # The shape (manager's measurement, restated so the rows are not mysterious)
//!
//! The fix is a **render-time PNG derivative** of the stored wire bytes, at the
//! terminal's cell width, cached per entry. Not a second persisted attachment: a
//! display copy's correct pixel size is the pane's (2.06× the wire JPEG at 60 cells,
//! 0.85× at 40), which the session cannot know when it writes the record. The wire
//! bytes, their hash, the record shape and `assemble` are all untouched - which is
//! what row 2 pins, because "just normalise it to PNG" is the tempting wrong answer
//! (15× the store and the model then reads PNG, the exact thing #598 refused).
//!
//! # Why these rows drive `on_turn_event` and a real JPEG
//!
//! A minimal `\xff\xd8\xff\xe0` fixture does not decode, so a row built on one would
//! only ever exercise the fallback. These bytes come from the real `read` tool with a
//! real vision policy, so the payload the transcript receives is the payload a user
//! gets.
//!
//! Verifies: gh #599, and the protocol rule gh #598 established (kitty's `f=` names
//! PNG, never JPEG).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lca_protocol::{ToolCall, ToolResult, TurnEvent};
use lca_tui::engine::keybindings::KeybindingsManager;
use lca_tui::widgets::image::ImageProtocol;
use lca_ui::{Chat, UiOptions};

/// A chat in scrollback-free fullscreen mode, with the default display tuning
/// (images on, a 60-cell cap).
fn chat() -> Chat {
    Chat::new(
        UiOptions {
            prompt_slot: Default::default(),
            dialog_slot: Default::default(),
            pending_models: None,
            model_label: Arc::new(Mutex::new("fake/faux-1".to_string())),
            context_window: Arc::new(std::sync::Mutex::new(0)),
            thinking: Arc::new(Mutex::new(None)),
            theme: "auto".to_string(),
            theme_extra_dirs: Vec::new(),
            themes: lca_ui::theme::THEMES
                .iter()
                .map(|s| s.to_string())
                .collect(),
            initial_lines: Vec::new(),
            initial_header: (Vec::new(), Vec::new()),
            initial_records: Vec::new(),
            initial_tail_lines: Vec::new(),
            initial_messages: Vec::new(),
            open_resume_picker: false,
            yolo: false,
            early_input: None,
            thinking_visibility: Default::default(),
            codeblock_border: Default::default(),
            plain: true,
            color_depth: lca_tui::engine::colors::ColorDepth::Truecolor,
            invoke_command: Arc::new(|_, _| lca_protocol::CommandEffect::None),
            slash_commands: Vec::new(),
            models: Vec::new(),
            workspace: PathBuf::new(),
            keybinding_overrides: Default::default(),
            keybinding_error: None,
            render_regions: None,
            ui_events: None,
            update_notice: None,
            login: None,
            complete_login: None,
            pick_login: None,
            confirm_login_grant: None,
            confirm_switch: None,
            hooks: lca_ui::UiHooks::default(),
            fullscreen: true,
            follow_up_mode: lca_protocol::QueueMode::OneAtATime,
        },
        Arc::new(KeybindingsManager::with_user_bindings_for(
            Default::default(),
            lca_tui::engine::keybindings::HostFamily::Unix,
        )),
    )
}

/// A photo-like 2000×1333 PNG: continuous tones, so the resize path's JPEG is
/// genuinely a JPEG (a flat rectangle would compress to a few hundred bytes and make
/// the size assertions uninteresting).
fn photo_png() -> Vec<u8> {
    let rgb = image::RgbImage::from_fn(2000, 1333, |x, y| {
        image::Rgb([
            ((x * 200 / 2000) + 40) as u8,
            ((y * 200 / 1333) + 20) as u8,
            ((x + y) * 200 / 3333 + (x * 7 + y * 13) % 40) as u8,
        ])
    });
    let mut png = Vec::new();
    rgb.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .expect("encode the fixture");
    png
}

/// The `ToolResult` the **real** read tool returns for `path` under a vision model's
/// resize profile - the wire bytes a turn actually carries.
fn resized_read(bytes_path: &std::path::Path) -> ToolResult {
    let workspace = bytes_path.parent().expect("a parent directory");
    let mut exec = lca_tools::ToolExecutor::new(
        Arc::new(lca_tools::NativeOps::default()),
        workspace.to_path_buf(),
        workspace.to_path_buf(),
        65536,
        Some(Duration::from_secs(30)),
    );
    exec.set_image_policy(lca_tools::ImagePolicy {
        vision: lca_tools::ImageVision::Vision,
        resize: Some(lca_protocol::ImageResize {
            max_width: 1280,
            max_height: 1280,
            max_bytes: 4_500_000,
        }),
    });
    let call = ToolCall {
        call_id: "call-read".to_string(),
        name: "read".to_string(),
        arguments: serde_json::json!({ "path": bytes_path.file_name().unwrap().to_string_lossy() })
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

/// A chat holding one image tool result, plus the wire result itself.
fn chat_with_resized_photo(name: &str) -> (Chat, ToolResult) {
    let dir = lca_testkit::scratch_path(&format!("gh599-{name}"));
    std::fs::create_dir_all(dir.join("project")).expect("mkdir");
    let path = dir.join("project").join("photo.png");
    std::fs::write(&path, photo_png()).expect("write the fixture");
    let result = resized_read(&path);
    assert_eq!(result.status, lca_protocol::ToolResultStatus::Ok);
    let mut chat = chat();
    chat.on_turn_event(TurnEvent::ToolStarted(ToolCall {
        call_id: "call-read".to_string(),
        name: "read".to_string(),
        arguments: "{\"path\":\"photo.png\"}".to_string(),
        parent_call_id: None,
    }));
    chat.on_turn_event(TurnEvent::ToolFinished(result.clone()));
    (chat, result)
}

/// Paint the transcript for an explicit protocol, raw (no sequence stripping): the
/// kitty `ESC_G` block and the placeholder text are both the subject.
fn paint(chat: &Chat, width: u16, protocol: ImageProtocol) -> String {
    chat.transcript
        .render_for_protocol(width, &chat.theme, protocol)
        .join("\n")
}

/// Every kitty transmission header in a frame, as `(f=, c=, r=)`.
fn kitty_headers(frame: &str) -> Vec<(Option<u32>, u32, u32)> {
    let mut out = Vec::new();
    let mut rest = frame;
    while let Some(at) = rest.find("\x1b_G") {
        let tail = &rest[at + 3..];
        let Some(end) = tail.find(';') else {
            break;
        };
        let (mut format, mut columns, mut rows) = (None, 0, 0);
        for param in tail[..end].split(',') {
            let Some((key, value)) = param.split_once('=') else {
                continue;
            };
            let Ok(number) = value.parse::<u32>() else {
                continue;
            };
            match key {
                "f" => format = Some(number),
                "c" => columns = number,
                "r" => rows = number,
                _ => {}
            }
        }
        if tail[..end].contains("a=T") {
            out.push((format, columns, rows));
        }
        rest = &tail[end..];
    }
    out
}

/// The base64 payload a kitty transmission carries, decoded: **every chunk**, since
/// the protocol breaks a large upload into 4096-character continuations
/// (`\x1b_Gm=1;…`) and re-emitting each one with a body is exactly how #324 gap 1
/// shipped. Reading only the first chunk would compare a truncated payload to the
/// stored bytes and pass for the wrong reason.
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

/// The wire bytes of the single image in a result.
fn wire(result: &ToolResult) -> (String, &[u8]) {
    let image = result.images.first().expect("one image");
    (image.media_type.clone(), image.bytes.as_slice())
}

// Verifies: gh #599 (row 1) - **the kitty lane draws a resized image.** The frame
// carries an `f=100` transmission whose payload **sniffs as PNG** (the magic bytes,
// not just the code - that is how #598 shipped), and the placeholder text is absent
// from that frame. Red on the pre-cycle tree, which painted the placeholder because
// `kitty_format_for("image/jpeg")` is `None`.
#[test]
fn a_resized_image_is_drawn_on_kitty_as_a_png_and_not_labelled() {
    let (chat, _result) = chat_with_resized_photo("kitty");
    let frame = paint(&chat, 100, ImageProtocol::Kitty);
    let headers = kitty_headers(&frame);
    assert_eq!(headers.len(), 1, "one transmission for one image:\n{frame}");
    assert_eq!(headers[0].0, Some(100), "kitty's PNG code, not a lie");
    let payload = kitty_payload(&frame).expect("the transmission carries a payload");
    assert_eq!(
        payload.get(..8),
        Some(&b"\x89PNG\r\n\x1a\n"[..]),
        "the payload's own magic bytes say PNG, not the `f=` alone"
    );
    assert!(
        !frame.contains("[image image/jpeg"),
        "no placeholder label for the picture that is now drawn:\n{frame}"
    );
}

// Verifies: gh #599 (row 2) - **the wire bytes are unchanged.** The same read still
// hands the model `image/jpeg`, with the JPEG's own magic bytes and the resize
// profile's dimensions, and the tool's own content line still says `resized to`.
// This is the row that stops a future "just normalise it to PNG" from slipping in:
// the derivative is a *display* fact, and the provider must keep reading a JPEG.
#[test]
fn the_wire_bytes_stay_a_resized_jpeg_for_the_model() {
    let (_chat, result) = chat_with_resized_photo("wire");
    let (media_type, bytes) = wire(&result);
    assert_eq!(media_type, "image/jpeg", "the model's bytes stay JPEG");
    assert_eq!(
        bytes.get(..2),
        Some(&b"\xff\xd8"[..]),
        "and the payload really is a JPEG"
    );
    let decoded = image::load_from_memory(bytes).expect("the wire bytes decode");
    assert_eq!(
        (decoded.width(), decoded.height()),
        (1280, 853),
        "at the profile's dimensions, as the tool reported"
    );
    assert!(
        result.content.contains("resized to 1280x853"),
        "the tool's own line still names the resize: {}",
        result.content
    );
}

// Verifies: gh #599 (row 3) - **iTerm2 is unchanged.** The same image still renders
// through the iTerm2 lane, which sniffs the payload and needs no format claim. The
// point of the cycle is that only kitty was broken; a derivative that also took over
// the working lane would be a regression dressed as a fix.
#[test]
fn the_iterm2_lane_still_draws_the_same_image() {
    let (chat, _result) = chat_with_resized_photo("iterm2");
    let frame = paint(&chat, 100, ImageProtocol::Iterm2);
    assert!(
        frame.contains("\x1b]1337;File=inline=1"),
        "the iTerm2 inline-image command is there:\n{frame}"
    );
    assert!(
        kitty_headers(&frame).is_empty(),
        "and nothing kitty-shaped was invented for it"
    );
    assert!(
        !frame.contains("[image image/jpeg"),
        "iTerm2 draws the picture, so no label either:\n{frame}"
    );
}

// Verifies: gh #599 (row 5) - **the derivative is encoded once, not once per frame.**
// A redraw at the same width drops the transcript's per-entry line cache (so the
// entry really is re-rendered) and the encoder must still have run exactly once: a
// renderer that encodes per frame is a renderer that stutters, and the painted bytes
// cannot tell the two apart - only the census can. This is gh #324's "derived rather
// than allocated" lesson, one layer over.
#[test]
fn a_redraw_at_the_same_width_does_not_encode_again() {
    let (mut chat, _result) = chat_with_resized_photo("cache");
    let first = paint(&chat, 100, ImageProtocol::Kitty);
    assert_eq!(
        chat.transcript.image_encodes_for_test(),
        1,
        "the first frame encodes"
    );
    assert!(!first.is_empty());
    // Drop every cached line, the way a theme change or a streaming append that
    // invalidates the last entry does: the next frame re-renders the image entry.
    chat.transcript.invalidate();
    let again = paint(&chat, 100, ImageProtocol::Kitty);
    assert_eq!(
        chat.transcript.image_encodes_for_test(),
        1,
        "the redraw re-rendered the entry and still did not re-encode"
    );
    // The second frame is a **placement** for the texture the terminal already holds
    // (gh #324 gap 1), which is the proof the bytes were not re-encoded: the id is
    // derived from the payload and the box, so a re-encode would be a new id and a
    // fresh transmission.
    let first_id = kitty_headers(&first);
    assert_eq!(first_id.len(), 1, "the first frame transmits:\n{first}");
    assert!(
        again.contains("\x1b_Ga=p"),
        "the redraw places the same texture instead of transmitting again:\n{again}"
    );
    assert!(
        !again.contains("\x1b_Ga=T"),
        "and it carries no payload, so nothing was re-encoded"
    );
}

// Verifies: gh #599 (row 6) - **a width change re-derives once**, and the drawn box
// follows the new width. The box is the renderer's arithmetic (`image_cell_size`), so
// a wider pane really does mean more cells; a derivative encoded at the wire's pixel
// size would keep the old box and fail here.
#[test]
fn a_width_change_re_derives_the_box_once_more() {
    let (chat, result) = chat_with_resized_photo("width");
    // Both panes are narrower than the 60-cell cap (`terminal.image_width_cells`), so
    // the pane itself is the box - which is the fact the row is about: a cap wider
    // than the pane would hold both frames at 60 and the box would not follow.
    let narrow = paint(&chat, 30, ImageProtocol::Kitty);
    let wider = paint(&chat, 60, ImageProtocol::Kitty);
    assert_eq!(
        chat.transcript.image_encodes_for_test(),
        2,
        "one encode per width, and only the new width encodes again"
    );
    let (_, narrow_cols, narrow_rows) = kitty_headers(&narrow)[0];
    let (_, wide_cols, wide_rows) = kitty_headers(&wider)[0];
    assert_eq!(narrow_cols, 30, "the narrow frame uses its whole pane");
    assert!(
        wide_cols > narrow_cols,
        "a wider pane is a wider box: {narrow_cols} -> {wide_cols}"
    );
    assert!(
        wide_rows >= narrow_rows,
        "and the aspect holds the rows in step: {narrow_rows} -> {wide_rows}"
    );
    // The wire bytes are still the JPEG at both widths: the box is display state.
    let (media_type, _bytes) = wire(&result);
    assert_eq!(media_type, "image/jpeg");
}

// Verifies: gh #599 (row 7) - **a payload that does not decode still takes the
// placeholder.** The renderer never fails a frame because a picture would not decode:
// `display_png` answers `None`, the ladder is asked the ordinary question, and #598's
// refusal is what the frame shows. This is the arm a decode-failure stub would
// silently turn into a blank box.
#[test]
fn an_undecodable_jpeg_still_paints_the_placeholder() {
    let mut chat = chat();
    chat.on_turn_event(TurnEvent::ToolStarted(ToolCall {
        call_id: "call-bad".to_string(),
        name: "read".to_string(),
        arguments: "{}".to_string(),
        parent_call_id: None,
    }));
    let mut result = ToolResult::ok("call-bad", "[image image/jpeg, 44 bytes]");
    // The #598 fixture: JPEG magic, no decodable body.
    let mut broken = b"\xff\xd8\xff\xe0".to_vec();
    broken.extend_from_slice(&[0u8; 40]);
    result.images.push(lca_protocol::ImageContent {
        media_type: "image/jpeg".to_string(),
        bytes: broken,
    });
    chat.on_turn_event(TurnEvent::ToolFinished(result));
    let frame = paint(&chat, 100, ImageProtocol::Kitty);
    assert!(
        kitty_headers(&frame).is_empty(),
        "nothing is transmitted for bytes that do not decode"
    );
    assert!(
        frame.contains("[image image/jpeg"),
        "and the placeholder names what could not be sent:\n{frame}"
    );
    assert_eq!(
        chat.transcript.image_encodes_for_test(),
        0,
        "nothing was encoded either"
    );
}

// Verifies: gh #599 - **a PNG is not re-encoded at all.** It already has a kitty code
// (`f=100`), so the derivative would be pure cost: the census stays zero and the
// transmission is the original bytes.
#[test]
fn a_png_passes_through_without_a_derivative() {
    let mut chat = chat();
    chat.on_turn_event(TurnEvent::ToolStarted(ToolCall {
        call_id: "call-png".to_string(),
        name: "read".to_string(),
        arguments: "{}".to_string(),
        parent_call_id: None,
    }));
    let mut result = ToolResult::ok("call-png", "[image image/png]");
    let png = photo_png();
    result.images.push(lca_protocol::ImageContent {
        media_type: "image/png".to_string(),
        bytes: png.clone(),
    });
    chat.on_turn_event(TurnEvent::ToolFinished(result));
    let frame = paint(&chat, 100, ImageProtocol::Kitty);
    let payload = kitty_payload(&frame).expect("the PNG transmits");
    assert_eq!(
        payload.len(),
        png.len(),
        "the payload is the stored bytes, not a re-encode"
    );
    assert_eq!(
        chat.transcript.image_encodes_for_test(),
        0,
        "no derivative was made for a format kitty already sends"
    );
}

// Verifies: gh #599 - **the window crop's band arithmetic uses the payload's height,
// not the wire bytes'.** A kitty placement's `y=`/`h=` are pixels of the texture the
// terminal holds, which is now the derived PNG (a different size from the wire JPEG),
// so a scrolled band sliced against the wire height would be misaligned by up to a
// cell row. The rows read the number the crop itself reads, and it is not the wire's.
#[test]
fn the_crop_reads_the_derived_payload_height() {
    let (chat, result) = chat_with_resized_photo("crop");
    let (_, wire_bytes) = wire(&result);
    let wire_height = image::load_from_memory(wire_bytes)
        .map(|image| image.height())
        .expect("the wire JPEG decodes");
    assert_eq!(wire_height, 853, "the wire bytes are 1280x853");
    let width = 100;
    // Paint the document first, exactly as a frame does, so the memo is warm.
    let rows = chat
        .transcript
        .render_for_protocol(width, &chat.theme, ImageProtocol::Kitty);
    assert!(rows.iter().any(|row| row.contains("\x1b_Ga=T")));
    let heights = chat.transcript.image_blocks(width);
    let derived = heights
        .values()
        .copied()
        .next()
        .expect("the image block is in the document");
    assert_ne!(
        derived, wire_height,
        "the crop is sliced in the derived payload's pixels, not the wire JPEG's"
    );
    // The payload is the 60-cell box at the 9x18 estimate: 540x360.
    assert_eq!(derived, 360, "the derived payload's own height");
    // A cold read at a *different* width has no memo for it and falls back to the
    // wire height, which is the same picture's aspect - the honest answer rather than
    // an encode during a measurement.
    let cold = chat.transcript.image_blocks(40);
    assert_eq!(
        cold.values().copied().next(),
        Some(wire_height),
        "a width nobody painted keeps the wire height"
    );
}
