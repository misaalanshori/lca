//! NFR-24 guard for gh #598: the kitty encoder must not claim a format it was not
//! handed, and the **frame** is what proves it.
//!
//! The defect: `lca_tools::image::downscale_image` re-encodes a vision model's resize
//! **as JPEG** and pushes it onto `ToolResult::images`; the bytes are stored
//! content-addressed with no conversion; `lca_ui` builds `ImageInfo` from the carried
//! media type; and `lca_tui`'s kitty encoder then emitted `f=100` — the protocol's
//! **PNG** code — for a JPEG payload. The terminal followed the claim, tried to read
//! JPEG bytes as PNG, and drew nothing. So the bug was not `lca-ui`'s and not
//! `lca-tools`'s: it was the encoder being asked a question it had no media type for
//! and answering anyway.
//!
//! # Why these rows are driven through the transcript
//!
//! A unit call to `encode_kitty` cannot see the bug's actual path, and the brief is
//! explicit that the row it wants is "a scripted tool result carrying `image/jpeg`
//! bytes, one turn driven through the loop, and the **frame's `f=` asserted**".
//! `ToolFinished` with JPEG bytes is exactly that event: the loop hands the result to
//! `Chat::on_turn_event`, which stores it and renders the transcript. The assertion is
//! on `render(width)`'s output, so a regression anywhere between the event and the
//! paint is caught.
//!
//! Verifies: gh #598, and the protocol rule it rests on (kitty's spec: a terminal
//! "must understand pixel data in three formats, 24-bit RGB, 32-bit RGBA and PNG" -
//! there is no JPEG code).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use lca_protocol::{ToolResult, TurnEvent};
use lca_tui::engine::keybindings::KeybindingsManager;
use lca_ui::{Chat, UiOptions};

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

/// Paint a chat's sections for an explicit protocol.
///
/// `Chat::sections` detects the protocol from the terminal, and CI's macOS leg is
/// iTerm2-capable, so a row about kitty's `f=` must **name** kitty - the same seam
/// gh #324 gap 1 added for exactly this reason (`sections_for_protocol`).
fn paint(chat: &Chat, protocol: lca_tui::widgets::image::ImageProtocol) -> String {
    // **No `strip_terminal_sequences` here**: the kitty `ESC_G` sequence is the
    // subject, and the stripper removes exactly it. The assertions therefore read the
    // raw frame, which is what the terminal gets.
    let rows = chat
        .transcript
        .render_for_protocol(100, &chat.theme, protocol);
    rows.join("\n")
}

/// A one-turn tool result carrying `bytes` labelled `media_type`, painted.
fn frame_with(media_type: &str, bytes: Vec<u8>) -> String {
    let mut chat = chat();
    chat.on_turn_event(TurnEvent::ToolStarted(lca_protocol::ToolCall {
        call_id: "call-jpeg".into(),
        name: "read".into(),
        arguments: "{\"path\":\"shot.jpg\"}".into(),
        parent_call_id: None,
    }));
    let mut result = ToolResult::ok("call-jpeg", "[image image/jpeg, 44 bytes]");
    result.images.push(lca_protocol::ImageContent {
        media_type: media_type.to_string(),
        bytes,
    });
    chat.on_turn_event(TurnEvent::ToolFinished(result));
    paint(&chat, lca_tui::widgets::image::ImageProtocol::Kitty)
}

/// A minimal JPEG: SOI + APP0 marker and a short body. `sniff_image_media_type` reads
/// the magic, so the host accepts it exactly as it accepts a real screenshot.
fn jpeg_bytes() -> Vec<u8> {
    let mut jpeg = b"\xff\xd8\xff\xe0".to_vec();
    jpeg.extend_from_slice(&[0u8; 40]);
    jpeg
}

/// A minimal PNG header with real dimensions (the sniffer reads 16..24).
fn png_bytes(width: u32, height: u32) -> Vec<u8> {
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.extend_from_slice(&[0, 0, 0, 13]);
    png.extend_from_slice(b"IHDR");
    png.extend_from_slice(&width.to_be_bytes());
    png.extend_from_slice(&height.to_be_bytes());
    png.extend_from_slice(&[0; 12]);
    png
}

/// Every kitty `f=` value the frame carries.
fn kitty_formats(frame: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = frame;
    while let Some(at) = rest.find("\x1b_G") {
        let tail = &rest[at + 3..];
        let end = tail.find(';').unwrap_or(tail.len());
        for param in tail[..end].split(',') {
            if let Some(value) = param.strip_prefix("f=") {
                out.push(value.to_string());
            }
        }
        rest = &tail[end.min(tail.len())..];
    }
    out
}

// Verifies: gh #598 - **a JPEG reaching the transcript takes the placeholder, and the
// frame carries no `f=100`.** That is the defect's exact shape: before the fix the
// frame said `f=100` (PNG) for these bytes, and a kitty terminal dropped the decode.
//
// The assertion is on the **rendered frame**, not on `encode_kitty`, because the
// question the bug asks is "what did the terminal get told", and only the frame knows.
#[test]
fn a_jpeg_tool_result_paints_a_placeholder_and_never_claims_png() {
    let frame = frame_with("image/jpeg", jpeg_bytes());
    assert!(
        kitty_formats(&frame).is_empty(),
        "no kitty transmission at all for a JPEG: {:?}",
        kitty_formats(&frame)
    );
    assert!(
        frame.contains("[image image/jpeg"),
        "the placeholder names what could not be sent:\n{frame}"
    );
}

// Verifies: gh #598 - **PNG keeps painting through kitty, with the legacy PNG code.**
// The fix must not cost the working path: the frame carries `f=100` and a real
// transmission, and the payload is present.
#[test]
fn a_png_tool_result_still_paints_through_kitty() {
    let frame = frame_with("image/png", png_bytes(400, 300));
    assert_eq!(
        kitty_formats(&frame),
        vec!["100".to_string()],
        "PNG transmits as PNG, once:\n{frame}"
    );
    assert!(
        frame.contains(",i="),
        "with the derived id the placement cache keys on:\n{frame}"
    );
}

// Verifies: gh #598 - **the refusal is a decision about the media type, not about
// whether the bytes happen to look decodable.** A WebP and a GIF are also un-sendable
// over kitty, so both take the placeholder; and the placeholder is specific about
// their dimensions, because the sniffer read the header.
#[test]
fn a_webp_or_gif_tool_result_is_refused_for_the_same_reason() {
    let mut webp = vec![0u8; 30];
    webp[..4].copy_from_slice(b"RIFF");
    webp[8..12].copy_from_slice(b"WEBP");
    webp[12..16].copy_from_slice(b"VP8X");
    webp[24..27].copy_from_slice(&[0x2f, 0x01, 0x00]); // 560-1
    webp[27..30].copy_from_slice(&[0x8f, 0x01, 0x00]); // 400-1
    let mut gif = b"GIF89a".to_vec();
    gif.extend_from_slice(&320u16.to_le_bytes());
    gif.extend_from_slice(&200u16.to_le_bytes());

    for (media_type, bytes) in [("image/webp", webp), ("image/gif", gif)] {
        let frame = frame_with(media_type, bytes);
        assert!(
            kitty_formats(&frame).is_empty(),
            "{media_type} must not be transmitted: {:?}",
            kitty_formats(&frame)
        );
        assert!(
            frame.contains(media_type),
            "the placeholder names it: {media_type}\n{frame}"
        );
    }
}

// Verifies: gh #598 - **iTerm2 needs no format code, which is why JPEG already
// displayed there.** The asymmetry is the finding: kitty must be told a format and
// has none for JPEG, so claiming one was a lie; iTerm2's protocol carries the box and
// the payload and lets the terminal sniff. A frame painted through the iTerm2 arm
// therefore has a JPEG transmission and **no `f=` at all**.
#[test]
fn an_iterm2_frame_carries_a_jpeg_with_no_format_claim() {
    let mut chat = chat();
    chat.on_turn_event(TurnEvent::ToolStarted(lca_protocol::ToolCall {
        call_id: "call-jpeg".into(),
        name: "read".into(),
        arguments: "{}".into(),
        parent_call_id: None,
    }));
    let mut result = ToolResult::ok("call-jpeg", "[image image/jpeg]");
    result.images.push(lca_protocol::ImageContent {
        media_type: "image/jpeg".to_string(),
        bytes: jpeg_bytes(),
    });
    chat.on_turn_event(TurnEvent::ToolFinished(result));
    let frame = paint(&chat, lca_tui::widgets::image::ImageProtocol::Iterm2);
    assert!(
        frame.contains("\x1b]1337;File=inline=1"),
        "the iTerm2 inline-image command is there:\n{frame}"
    );
    assert!(
        kitty_formats(&frame).is_empty(),
        "and no kitty `f=` was invented for it: {:?}",
        kitty_formats(&frame)
    );
}
