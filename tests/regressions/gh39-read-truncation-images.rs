//! GitHub #39: read-tool truncation and image semantics (pi's contract
//! shapes on LCA's machinery).
//!
//! Pi ground truth: `src/core/tools/read.ts` (line truncation + offset
//! guidance + first-line advice), `src/core/tools/truncate.ts`
//! (`DEFAULT_MAX_LINES = 2000`), `processImage` with
//! `inputLimits.images.resize`, `docs/models.md` ("Image Input Limits").

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use std::sync::Arc;
use std::time::Duration;

use lca_protocol::ToolCall;

fn executor(workspace: &std::path::Path) -> lca_tools::ToolExecutor {
    lca_tools::ToolExecutor::new(
        Arc::new(lca_tools::NativeOps::default()),
        workspace.to_path_buf(),
        workspace.to_path_buf(),
        65536,
        Duration::from_secs(30),
    )
}

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = lca_testkit::scratch_path(&format!("gh39-read-{name}"));
    std::fs::create_dir_all(dir.join("project")).expect("mkdir");
    dir.join("project")
}

fn call(name: &str, arguments: serde_json::Value) -> ToolCall {
    ToolCall {
        call_id: "call-1".to_string(),
        name: name.to_string(),
        arguments: arguments.to_string(),
    }
}

fn run(exec: &mut lca_tools::ToolExecutor, call: &ToolCall) -> lca_protocol::ToolResult {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
        .block_on(async {
            let mut sink = |_: &[u8]| {};
            exec.execute(call, &mut sink, &lca_tools::CancelFlag::new())
                .await
        })
}

// Verifies: #39 (pi's line budget rides on top of the byte budget: long
// files truncate by lines with the offset sentence the model relies on).
#[test]
fn gh39_long_file_truncates_by_lines_with_offset_guidance() {
    let workspace = scratch("lines");
    let body = (1..=2500)
        .map(|n| format!("line {n}"))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    std::fs::write(workspace.join("long.txt"), &body).expect("write");
    let mut exec = executor(&workspace);
    let result = run(
        &mut exec,
        &call("read", serde_json::json!({"path": "long.txt"})),
    );
    assert_eq!(result.status, lca_protocol::ToolResultStatus::Ok);
    assert!(
        result.truncated,
        "2500 short lines exceed the 2000-line budget"
    );
    assert!(
        result.content.contains("of 2500")
            && result.content.contains("Use offset=2001 to continue"),
        "the continuation sentence names the resume point: {}",
        result.content.lines().last().unwrap_or("")
    );
}

// Verifies: #39 (pi's first-line advice: a single line over the byte
// budget points at the shell instead of returning nothing useful).
#[test]
fn gh39_first_line_overflow_points_at_the_shell() {
    let workspace = scratch("firstline");
    std::fs::write(workspace.join("blob.txt"), "y".repeat(70000)).expect("write");
    let mut exec = executor(&workspace);
    let result = run(
        &mut exec,
        &call("read", serde_json::json!({"path": "blob.txt"})),
    );
    assert_eq!(result.status, lca_protocol::ToolResultStatus::Ok);
    assert!(
        result.content.contains("exceeds") && result.content.contains("Use shell:"),
        "the overflow names the shell fallback: {}",
        result.content.lines().next().unwrap_or("")
    );
}

fn big_png(width: u32, height: u32) -> Vec<u8> {
    let image = image::RgbImage::from_fn(width, height, |x, y| {
        image::Rgb([(x % 256) as u8, (y % 256) as u8, 128])
    });
    let mut bytes = Vec::new();
    image
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .expect("encode");
    bytes
}

// Verifies: #39 (a known-limits model gets a downscaled JPEG at or under
// its profile).
#[test]
fn gh39_image_resizes_to_the_model_profile() {
    let workspace = scratch("resize");
    std::fs::write(workspace.join("big.png"), big_png(3000, 2000)).expect("write");
    let mut exec = executor(&workspace);
    exec.set_image_policy(lca_tools::ImagePolicy {
        vision: lca_tools::ImageVision::Vision,
        resize: Some(lca_protocol::ImageResize {
            max_width: 1568,
            max_height: 1568,
            max_bytes: 524288,
        }),
    });
    let result = run(
        &mut exec,
        &call("read", serde_json::json!({"path": "big.png"})),
    );
    assert_eq!(result.status, lca_protocol::ToolResultStatus::Ok);
    assert_eq!(result.images.len(), 1, "one image block, resized");
    let image = &result.images[0];
    assert_eq!(
        image.media_type, "image/jpeg",
        "resized images travel as JPEG"
    );
    let decoded = image::load_from_memory(&image.bytes)
        .expect("decode")
        .to_rgb8();
    assert!(
        decoded.width() <= 1568 && decoded.height() <= 1568,
        "within the profile: {}x{}",
        decoded.width(),
        decoded.height()
    );
}

// Verifies: #39 (pi's explicit note: a model without vision gets words,
// never bytes it cannot use).
#[test]
fn gh39_no_vision_model_gets_the_note_not_bytes() {
    let workspace = scratch("novision");
    std::fs::write(workspace.join("pic.png"), big_png(64, 64)).expect("write");
    let mut exec = executor(&workspace);
    exec.set_image_policy(lca_tools::ImagePolicy {
        vision: lca_tools::ImageVision::NoVision,
        resize: None,
    });
    let result = run(
        &mut exec,
        &call("read", serde_json::json!({"path": "pic.png"})),
    );
    assert_eq!(result.status, lca_protocol::ToolResultStatus::Ok);
    assert!(
        result.images.is_empty(),
        "no image block for a model without vision"
    );
    assert!(
        result.content.contains("does not support images"),
        "the explicit note instead: {}",
        result.content
    );
}

// Verifies: #39 (unknown model = current behavior: bytes pass through at
// original size).
#[test]
fn gh39_unknown_model_sends_bytes_unchanged() {
    let workspace = scratch("unknown");
    let bytes = big_png(3000, 2000);
    std::fs::write(workspace.join("big.png"), &bytes).expect("write");
    let mut exec = executor(&workspace);
    let result = run(
        &mut exec,
        &call("read", serde_json::json!({"path": "big.png"})),
    );
    assert_eq!(result.status, lca_protocol::ToolResultStatus::Ok);
    assert_eq!(result.images.len(), 1);
    assert_eq!(result.images[0].media_type, "image/png");
    assert_eq!(result.images[0].bytes, bytes, "original bytes, untouched");
}
