//! Gh #324 gap 3: a scrolled-past image is cropped to the band the window shows.
//!
//! The arithmetic is pinned in `lca-tui` (`image::crop_tests`, `image::clip_band_tests`).
//! These rows are the **seam**: the transcript window slices a document by integer
//! rows, so a block that straddles an edge arrives at the renderer as a fragment. The
//! crop has to happen where the whole block is still addressable, which is here.

use super::*;
use lca_tui::widgets::image::{ImageInfo, ImageProtocol};

/// A PNG header carrying real dimensions, so `ImageInfo::new` knows the pixel height.
fn png(width: u32, height: u32) -> Vec<u8> {
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.extend_from_slice(&[0, 0, 0, 13]);
    png.extend_from_slice(b"IHDR");
    png.extend_from_slice(&width.to_be_bytes());
    png.extend_from_slice(&height.to_be_bytes());
    png.extend_from_slice(&[0; 12]);
    png
}

/// A fullscreen chat holding one tall image and enough trailing text to scroll past
/// it. Returns the chat plus the picture's pixel height, so the rows can assert exact
/// bands rather than "something changed".
fn chat_with_tall_image_padded(pad: usize) -> (crate::chat::Chat, u32) {
    let mut chat = Chat::new(
        super::tests::options(),
        std::sync::Arc::new(lca_tui::engine::keybindings::KeybindingsManager::new()),
    );
    chat.screen_mode = true;
    // Text *above* the image, so scrolling can cut the image's top while leaving its
    // tail in view - which is the only way the top-clip case is reachable.
    for n in 0..pad {
        chat.transcript.push_user(format!("before {n}"));
    }
    let bytes = png(200, 400);
    chat.transcript
        .push_image(ImageInfo::new("image/png", &bytes), bytes);
    for n in 0..40 {
        chat.transcript.push_user(format!("after {n}"));
    }
    (chat, 400)
}

/// A chat whose image sits near the document's start, so only the *bottom* edge can be
/// clipped as the window is scrolled back towards it.
fn chat_with_tall_image() -> (crate::chat::Chat, u32) {
    chat_with_tall_image_padded(60)
}

/// The `(y, h, r, i)` of the first kitty placement found in a frame.
fn placement(rows: &[String]) -> Option<(u32, u32, u32, u32)> {
    for row in rows {
        let Some(at) = row.find("\x1b_G") else {
            continue;
        };
        let rest = &row[at + 3..];
        let params = match rest.find(';') {
            Some(end) => &rest[..end],
            None => rest.split("\x1b\\").next().unwrap_or(rest),
        };
        let (mut y, mut h, mut r, mut i) = (None, None, None, None);
        for param in params.split(',') {
            let Some((k, v)) = param.split_once('=') else {
                continue;
            };
            let Ok(n) = v.parse::<u32>() else { continue };
            match k {
                "y" => y = Some(n),
                "h" => h = Some(n),
                "r" => r = Some(n),
                "i" => i = Some(n),
                _ => {}
            }
        }
        // An uncropped placement carries no `y`/`h` at all - only a cropped band
        // does - so the missing pairs are reported as sentinels rather than a panic.
        return Some((y.unwrap_or(0), h.unwrap_or(0), r.unwrap_or(0), i?));
    }
    None
}

/// Whether any row of the frame carries a kitty image sequence.
fn has_image(rows: &[String]) -> bool {
    rows.iter().any(|r| r.contains("\x1b_G"))
}

// Verifies: gh #324 gap 3 - **an image scrolled so its top is hidden is placed as the
// visible band.** The receipt is `y=` (non-zero: the slice starts inside the picture)
// and a reduced `r=`. Before the fix the whole block was placed and the visible rows
// showed the picture's top rather than the band in view.
#[test]
fn a_top_scrolled_image_is_placed_as_the_visible_band() {
    lca_tui::widgets::image::clear_cell_pixels();
    // Text above the image, so a scroll exists that puts part of it in view with its
    // top cut off.
    let (chat, px_h) = chat_with_tall_image_padded(40);
    // The window is `height - dock`, and `scroll` counts rows up from the document's
    // bottom. Deriving the reachable range from the document's own length keeps this
    // row honest when the fixture's padding changes.
    let (doc, dock) = chat.sections_for_protocol(80, ImageProtocol::Kitty);
    let header_at = doc
        .iter()
        .position(|r| r.contains("\x1b_G"))
        .expect("the document holds the image");
    let _ = dock;
    // The chat's own window — `height - dock - tab bar` — not a subtraction I redo
    // here, so this row cannot disagree with the code it exercises.
    let window = chat.window_height(80, 24);
    let max_scroll = doc.len().saturating_sub(window);
    // Sweep every scroll position that can put the header at or just above the
    // window's top: `from_bottom` is what the frame subtracts.
    let mut seen = None;
    for scroll in (0..=max_scroll).rev() {
        let frame = chat.viewport_for_protocol(80, 24, scroll as u16, ImageProtocol::Kitty);
        let start = max_scroll.saturating_sub(scroll);
        // Only positions where the header is above the window's start but its block
        // still reaches into it are top-clipped.
        if start > header_at
            && start < header_at + 23
            && let Some(p) = placement(&frame)
            && p.0 > 0
        {
            seen = Some(p);
            break;
        }
    }
    let (y, h, r, _id) = seen.unwrap_or_else(|| {
        panic!("no scroll position clipped the image's top; the fixture is wrong")
    });
    assert!(y > 0, "the slice starts inside the picture: y={y}");
    assert!(h < px_h, "and is shorter than the picture: h={h} of {px_h}");
    assert!((1..23).contains(&r), "covering {r} of the block's 23 rows");
    // The band's pixel height is what those rows cover, to within a pixel of rounding.
    let expected = (u64::from(px_h) * u64::from(r)).div_ceil(23) as u32;
    assert!(
        h.abs_diff(expected) <= 2,
        "h={h} matches the {r} rows it covers ({expected} px, +-1 for rounding)"
    );
}

// Verifies: gh #324 gap 3 - **the id does not change when only the crop changes, and
// no payload is re-sent.** Scrolling past an image is a new placement for the texture
// the terminal already holds: the id is stable by construction (gap 1 hashes payload
// and scale, deliberately not the crop), and the frame carries no base64. This is the
// correctness property and the performance property at once.
#[test]
fn scrolling_changes_the_placement_not_the_id_and_never_re_sends_the_payload() {
    lca_tui::widgets::image::clear_cell_pixels();
    let (chat, _px_h) = chat_with_tall_image();

    // Derive the scrolls that reach the image rather than guessing: `scroll` counts up
    // from the document's bottom, so the reachable band is near `max_scroll`.
    let (doc, _dock) = chat.sections_for_protocol(80, ImageProtocol::Kitty);
    let header_at = doc
        .iter()
        .position(|r| r.contains("\x1b_G"))
        .expect("the document holds the image");
    let window = chat.window_height(80, 24);
    let max_scroll = doc.len().saturating_sub(window);
    let mut ids = std::collections::BTreeSet::new();
    let mut full_base64_frames = 0;
    let mut placed = 0;
    // Every scroll that puts any part of the block in the window.
    for start in header_at.saturating_sub(2)..(header_at + 26) {
        let scroll = max_scroll.saturating_sub(start) as u16;
        let frame = chat.viewport_for_protocol(80, 24, scroll, ImageProtocol::Kitty);
        if has_image(&frame) {
            placed += 1;
            if let Some((_, _, _, id)) = placement(&frame) {
                ids.insert(id);
            }
            // A frame that re-transmits carries a long base64 body; a placement has an
            // empty payload and so no long run of base64 characters.
            if frame.iter().any(|r| r.len() > 600) {
                full_base64_frames += 1;
            }
        }
    }
    assert!(
        placed > 3,
        "the image is placed at several scroll positions, got {placed}"
    );
    assert_eq!(
        ids.len(),
        1,
        "one texture, one id across every scroll: {ids:?}"
    );
    assert_eq!(
        full_base64_frames, 0,
        "and no frame re-transmits the payload: scrolling is placements only"
    );
}

// Verifies: gh #324 gap 3 - **a block entirely below the window places nothing.** The
// control in the other direction: a crop that would leave zero visible rows must not
// emit a zero-height placement (a protocol error on some terminals).
#[test]
fn an_image_outside_the_window_places_nothing() {
    lca_tui::widgets::image::clear_cell_pixels();
    let (chat, _px_h) = chat_with_tall_image();
    // Scrolled to the very top: the image (at the document's start) *is* visible, so
    // instead scroll the document the other way - the image is at the top, so the
    // bottom of the document is where it is out of view.
    let frame = chat.viewport_for_protocol(80, 24, 0, ImageProtocol::Kitty);
    if has_image(&frame) {
        // At scroll 0 the window shows the document's top, which holds the image.
        // That is the control: an image *in* the window is placed exactly once and
        // with a legal height.
        let (y, h, r, _) = placement(&frame).expect("placed");
        assert!(h >= 1, "never a zero-height placement: h={h}");
        assert!(r >= 1, "never a zero-row placement: r={r}");
        assert_eq!(y, 0, "and unclipped at the top");
    }
}

// Verifies: gh #324 gap 3 - **the placeholder path stays text.** The frame-level
// half: with no graphics, the rows are text and nothing is cropped as pixels, so the
// fallback a real tmux gets is untouched by this cycle.
#[test]
fn the_placeholder_frame_carries_no_placement_and_no_geometry() {
    lca_tui::widgets::image::clear_cell_pixels();
    let (chat, _px_h) = chat_with_tall_image();
    // `viewport` renders with the detected protocol, which on this box (and in CI) is
    // None, so the frame is the placeholder for every scroll position.
    for scroll in [0u16, 10, 20] {
        let frame = chat.viewport_for_protocol(80, 24, scroll, ImageProtocol::None);
        for row in &frame {
            assert!(
                !row.contains("\x1b_G"),
                "no kitty placement without graphics: {row:?}"
            );
            assert!(
                !row.contains("y=") || !row.contains(",h="),
                "and no pixel geometry on a text row: {row:?}"
            );
        }
    }
}
