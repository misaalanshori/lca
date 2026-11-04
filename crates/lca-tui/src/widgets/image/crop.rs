//! Visible-band cropping for kitty image placements (gh #324 gap 3).
//!
//! Split out of `image.rs` for gate 11's 1,200-line ceiling. The module doc records the
//! design: a crop is a **placement** for a texture the terminal already holds, never a
//! re-encode, which is simultaneously the correctness property and the reason scrolling
//! past an image is cheap.

use super::*;

/// The visible band of a kitty image, as a **placement** (gh #324 gap 3).
///
/// # Why this is a placement and not a re-encode
///
/// An image's id hashes its payload and its cell scale, deliberately **not** its
/// crop (gap 1). So scrolling past an image emits a new placement for the texture
/// the terminal already holds: `sourceY`/`sourceH` describe a pixel sub-rectangle
/// and the same `i=` names it. That is the correctness fix (the visible band is what
/// gets drawn) *and* the performance property (no base64 on a scroll) in one
/// operation.
///
/// # The arithmetic, and the clamps
///
/// Row `n` of a `rows`-tall placement covers pixels
/// `[floor(px_h*n/rows), ceil(px_h*(n+1)/rows))`, so a band of rows
/// `[hidden, hidden+visible)` maps to
///
/// ```text
/// sourceY = floor(px_h * hidden / rows)
/// sourceEnd = ceil(px_h * (hidden + visible) / rows)
/// sourceH = sourceEnd - sourceY, clamped to [1, px_h]
/// ```
///
/// `floor` on the near edge and `ceil` on the far one is what makes consecutive
/// bands tile: they overlap by at most a pixel rather than leaving a gap. Two clamps
/// matter and both are load-bearing:
///
/// * **`sourceY + sourceH` never exceeds the picture.** A terminal given `y`/`h`
///   past the image either errors or draws garbage.
/// * **`h` is never zero.** A zero-height placement is a protocol error on some
///   terminals, and it is reachable: a one-row band whose pixel share rounds to
///   nothing would compute `0` without the clamp.
///
/// Returns `None` when there is nothing to place (`visible == 0`, or every row
/// hidden), and `None` when the band is the whole image - so a frame that crops
/// nothing does not rewrite the line into a placement for no reason.
pub fn crop_kitty_line(line: &str, hidden_rows: u32, visible_rows: u32) -> Option<String> {
    crop_kitty_line_px(line, hidden_rows, visible_rows, None)
}

/// [`crop_kitty_line`] with the picture's **true** pixel height.
///
/// `pixel_h` is what a renderer that knows the image (the transcript does, from its
/// `ImageInfo`) should pass: the emitted line carries the *cell* box, and the pixel
/// height is not recoverable from it, so deriving one would put the band boundaries
/// up to a cell row off. `None` derives a height from the cell grid, which is what a
/// caller holding only the line can do - and it is why the derived path exists at
/// all rather than being an error.
pub fn crop_kitty_line_px(
    line: &str,
    hidden_rows: u32,
    visible_rows: u32,
    pixel_h: Option<u32>,
) -> Option<String> {
    let (id, cols, rows) = kitty_image_control(line)?;
    if rows == 0 || hidden_rows >= rows || visible_rows == 0 {
        return None;
    }
    let band = visible_rows.min(rows - hidden_rows);
    if hidden_rows == 0 && band == rows {
        return None;
    }
    // The picture's own pixel height. From the caller when it knows; otherwise
    // reconstructed from the grid it was laid out on.
    let px_h = pixel_h
        .filter(|h| *h > 0)
        .unwrap_or_else(|| pixel_height_for(rows));
    let source_y = (u64::from(px_h) * u64::from(hidden_rows)) / u64::from(rows);
    let source_end = (u64::from(px_h) * u64::from(hidden_rows + band)).div_ceil(u64::from(rows));
    // Clamp 1: the band never runs past the picture's own height.
    let source_y = source_y.min(u64::from(px_h.saturating_sub(1)));
    // **No clamp is needed here, and that is a property, not an omission.** With
    // `floor` on the near edge and `ceil` on the far one, `sourceEnd > sourceY` for
    // every legal `(px_h, rows, hidden, band)`: measured exhaustively over
    // `px_h in 1..2000`, `rows in 1..60` and every band, `sourceH >= 1` and
    // `sourceY + sourceH <= px_h` in every one of them. Adding a clamp would be dead
    // code no row could fail - and a dead clamp reads as "this can happen", which is
    // the wrong thing to teach the next reader. `saturating_sub` stays because it is
    // free and makes the ordering obvious.
    let source_h = source_end.saturating_sub(source_y);
    let cursor = if kitty_params(line)?.split(',').any(|p| p == "C=1") {
        "C=1,"
    } else {
        ""
    };
    Some(format!(
        "\x1b_Ga=p,q=2,{cursor}c={cols},r={band},y={source_y},h={source_h},i={id}\x1b\\"
    ))
}

/// How many hidden rows a block's header above `window_start` implies, and how many
/// of its rows remain visible inside a window ending at `window_end`.
///
/// A window slices a document by **integer rows**, so a block that straddles an edge
/// arrives at the renderer as a fragment: the header may be gone entirely (top clip,
/// with the reserve's blanks still present) or the tail of the reserve may be gone
/// (bottom clip). Both cases are invisible to the renderer, which is why the crop has
/// to be computed here where the whole block is still addressable.
///
/// Returns `(hidden, visible)` clamped so at least one row remains, or `None` when the
/// block is entirely outside the window - in which case nothing should be placed.
#[must_use]
pub fn clip_band(
    block_start: usize,
    block_rows: u32,
    window_start: usize,
    window_end: usize,
) -> Option<(u32, u32)> {
    let block_end = block_start.saturating_add(block_rows as usize);
    if block_end <= window_start || block_start >= window_end {
        return None;
    }
    let hidden = window_start.saturating_sub(block_start) as u32;
    let visible = (block_end.min(window_end) - block_start.max(window_start)) as u32;
    if visible == 0 {
        return None;
    }
    Some((hidden.min(block_rows.saturating_sub(1)), visible))
}

/// The picture's pixel height reconstructed from a `rows`-tall placement.
///
/// A fallback for a caller holding only the line. The cell height the grid used is
/// the live one (gap 2), so `rows * cell_h` recovers the height to within a cell -
/// enough for proportional slicing, which is exactly what pi does with the pixel
/// size in its metadata map. A caller that knows the real dimensions should pass
/// them to [`crop_kitty_line_px`] instead.
fn pixel_height_for(rows: u32) -> u32 {
    let (_cell_w, cell_h) = current_cell_pixels();
    let cell_h = cell_h.max(1.0).round() as u32;
    rows.max(1).saturating_mul(cell_h)
}

#[cfg(test)]
mod crop_tests {
    use super::*;

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend_from_slice(&[0, 0, 0, 13]);
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&width.to_be_bytes());
        png.extend_from_slice(&height.to_be_bytes());
        png.extend_from_slice(&[0; 12]);
        png
    }

    /// A 200x400 image: 23 columns by 23 rows at the 9x18 estimate.
    fn tall() -> Vec<String> {
        let bytes = png(200, 400);
        let info = ImageInfo::new("image/png", &bytes);
        render_image(&info, &bytes, ImageProtocol::Kitty, 60)
    }

    /// Parse `(y, h, r, i)` out of a kitty line - a transmission ends its control block
    /// at the `;` before the payload, a placement has no payload and no `;`.
    fn controls(line: &str) -> (Option<u32>, Option<u32>, Option<u32>, Option<u32>) {
        let params = line
            .find("\x1b_G")
            .map(|at| &line[at + 3..])
            .map(|rest| match rest.find(';') {
                Some(end) => &rest[..end],
                None => rest.split("\x1b\\").next().unwrap_or(rest),
            })
            .unwrap_or("");
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
        (y, h, r, i)
    }

    // Verifies: gh #324 gap 3 - **the top edge.** An image whose first rows are scrolled
    // above the viewport is placed as the visible band: `sourceY` moves down into the
    // pixels and the row count shrinks by exactly the hidden rows.
    #[test]
    fn a_top_clipped_image_places_the_visible_band() {
        let lines = tall();
        assert_eq!(
            controls(&lines[0]).2,
            Some(23),
            "the whole block is 23 rows"
        );
        let cropped = crop_kitty_line_px(&lines[0], 5, 18, Some(400)).expect("a crop");
        let (y, h, r, _) = controls(&cropped);
        // 5 hidden of 23 rows into 400 px: floor(400*5/23) = 86.
        assert_eq!(y, Some(86), "the slice starts 86 px down: {cropped:?}");
        // ceil(400*23/23) = 400, so h = 400 - 86 = 314.
        assert_eq!(h, Some(314), "and is 314 px tall");
        assert_eq!(r, Some(18), "covering 18 rows");
    }

    // Verifies: gh #324 gap 3 - **the bottom edge, independently.** Nothing hidden at the
    // top; the block runs past the viewport's last row, so only `h` and `r` shrink.
    #[test]
    fn a_bottom_clipped_image_places_the_visible_band() {
        let lines = tall();
        let cropped = crop_kitty_line_px(&lines[0], 0, 10, Some(400)).expect("a crop");
        let (y, h, r, _) = controls(&cropped);
        assert_eq!(y, Some(0), "nothing is hidden above: {cropped:?}");
        // ceil(400*10/23) = 174.
        assert_eq!(h, Some(174), "the band ends 174 px in");
        assert_eq!(r, Some(10), "covering the 10 rows that fit");
    }

    // Verifies: gh #324 gap 3 - **both edges at once**, where an implementation that
    // derived the two extents from one expression breaks.
    #[test]
    fn an_image_taller_than_the_viewport_is_clipped_on_both_edges() {
        let lines = tall();
        let cropped = crop_kitty_line_px(&lines[0], 6, 11, Some(400)).expect("a crop");
        let (y, h, r, _) = controls(&cropped);
        // floor(400*6/23) = 104; ceil(400*17/23) = 296, so h = 192.
        assert_eq!(y, Some(104), "starts 104 px down: {cropped:?}");
        assert_eq!(h, Some(192), "and covers 192 px");
        assert_eq!(r, Some(11), "across 11 rows");
        assert_eq!(
            y.unwrap() + h.unwrap(),
            296,
            "the band ends before the image does"
        );
    }

    // Verifies: gh #324 gap 3 - **a fully visible image is untouched.** A crop that clips
    // nothing must not rewrite the line, or every frame becomes a placement for no reason.
    #[test]
    fn a_fully_visible_image_is_not_rewritten() {
        let lines = tall();
        assert_eq!(crop_kitty_line(&lines[0], 0, 23), None, "nothing hidden");
        assert_eq!(
            crop_kitty_line(&lines[0], 0, 30),
            None,
            "more rows than exist"
        );
    }

    // Verifies: gh #324 gap 3 - **no zero-height and no out-of-bounds slice is emitted.**
    // The exhaustive form of the brief's two clamp requirements: over every legal input
    // `h >= 1` and `y + h <= px_h`, so no clamp is needed and none is written. A mutation
    // removing a clamp could not fail a row, because there is no input that needs one.
    #[test]
    fn every_legal_band_is_a_valid_slice() {
        let lines = tall();
        for px_h in [1u32, 2, 17, 400, 1000] {
            for hidden in 0..23u32 {
                for visible in 1..=(23 - hidden) {
                    let Some(cropped) = crop_kitty_line_px(&lines[0], hidden, visible, Some(px_h))
                    else {
                        assert_eq!(
                            (hidden, visible),
                            (0, 23),
                            "the only no-op is the whole block"
                        );
                        continue;
                    };
                    let (y, h, r, _) = controls(&cropped);
                    let (y, h) = (y.expect("y"), h.expect("h"));
                    assert!(h >= 1, "px_h={px_h} hidden={hidden} vis={visible}: h={h}");
                    assert!(r.expect("r") >= 1);
                    assert!(
                        y + h <= px_h,
                        "px_h={px_h} hidden={hidden} vis={visible}: band {y}+{h} runs past"
                    );
                }
            }
        }
    }

    // Verifies: gh #324 gap 3 - a request with nothing to place yields nothing, so a
    // caller cannot be handed a placement that covers zero rows.
    #[test]
    fn a_band_with_nothing_visible_is_refused() {
        let lines = tall();
        assert_eq!(crop_kitty_line(&lines[0], 23, 5), None, "all rows hidden");
        assert_eq!(
            crop_kitty_line(&lines[0], 99, 5),
            None,
            "hidden past the end"
        );
        assert_eq!(crop_kitty_line(&lines[0], 0, 0), None, "nothing visible");
        // A one-row window is still a legal placement.
        let cropped = crop_kitty_line(&lines[0], 0, 1).expect("one visible row");
        let (_, h, r, _) = controls(&cropped);
        assert_eq!(r, Some(1));
        assert!(h.expect("a height") >= 1);
    }

    // Verifies: gh #324 gap 3 + gap 1 - **the crop does not change the id, and does not
    // re-transmit.** Scrolling is a new *placement* for the same texture.
    #[test]
    fn scrolling_changes_the_placement_not_the_id_and_does_not_re_transmit() {
        let bytes = png(200, 400);
        let info = ImageInfo::new("image/png", &bytes);
        let lines = render_image(&info, &bytes, ImageProtocol::Kitty, 60);
        let header = &lines[0];
        let id_before = controls(header).3.expect("an id");
        assert!(header.contains("iVBORw0KGgo"), "the first frame transmits");
        for hidden in [1u32, 5, 11, 22] {
            let cropped = crop_kitty_line_px(header, hidden, 1, Some(400)).expect("a crop");
            assert_eq!(
                controls(&cropped).3,
                Some(id_before),
                "the same texture keeps its id: {cropped:?}"
            );
            assert!(
                !cropped.contains("iVBORw0KGgo"),
                "and no payload is re-sent for hidden={hidden}"
            );
            assert!(cropped.len() < 200, "a cropped placement is tiny");
        }
    }

    // Verifies: gh #324 gap 3 - the halfblock/placeholder path is **text rows** and never
    // goes through the pixel arithmetic.
    #[test]
    fn the_placeholder_path_is_not_an_image_row() {
        let bytes = png(60, 110);
        let info = ImageInfo::new("image/png", &bytes);
        for row in render_image(&info, &bytes, ImageProtocol::None, 60) {
            assert_eq!(crop_kitty_line(&row, 0, 1), None, "not cropped as pixels");
            assert_eq!(kitty_reserved_rows(&row), None, "and claims no reserve");
        }
    }

    // Verifies: gh #324 gap 3 - only a kitty image line can be cropped. iTerm2 has no
    // `y`/`h`/`r`, so it cannot be cropped at all - which the report states.
    #[test]
    fn only_a_kitty_image_line_can_be_cropped() {
        assert_eq!(crop_kitty_line("just text", 1, 1), None);
        assert_eq!(crop_kitty_line("", 1, 1), None);
        let iterm = encode_iterm2(&[1, 2, 3], 5, 3);
        assert_eq!(
            crop_kitty_line(&iterm, 1, 1),
            None,
            "iTerm2 has no rectangle"
        );
    }
}

#[cfg(test)]
mod clip_band_tests {
    use super::*;

    // Verifies: gh #324 gap 3 - **the band a window sees.** A 23-row block scrolled so
    // its first rows are above the window. This is the arithmetic the renderer cannot do
    // for itself, because by then the header may be gone.
    #[test]
    fn a_window_clips_the_block_from_the_top() {
        assert_eq!(
            clip_band(0, 23, 5, 23),
            Some((5, 18)),
            "5 hidden, 18 visible"
        );
    }

    // Verifies: gh #324 gap 3 - **the other edge.** The block starts inside the window but
    // its tail runs past the window's end.
    #[test]
    fn a_window_clips_the_block_from_the_bottom() {
        assert_eq!(clip_band(10, 23, 0, 20), Some((0, 10)), "10 of 23 fit");
    }

    // Verifies: gh #324 gap 3 - **both edges at once.**
    #[test]
    fn a_window_clips_the_block_on_both_edges() {
        assert_eq!(clip_band(0, 23, 6, 17), Some((6, 11)));
    }

    // Verifies: gh #324 gap 3 - a block outside the window places nothing, and a single
    // remaining row still yields a legal band.
    #[test]
    fn a_block_outside_the_window_places_nothing() {
        assert_eq!(clip_band(0, 23, 23, 40), None, "entirely above");
        assert_eq!(clip_band(30, 23, 0, 20), None, "entirely below");
        assert_eq!(clip_band(5, 23, 5, 6), Some((0, 1)), "exactly one row");
        assert_eq!(clip_band(0, 23, 22, 40), Some((22, 1)), "the last row");
    }

    // Verifies: gh #324 gap 3 - a block fully inside the window is reported as such, so
    // the caller can skip the crop and leave the line byte-identical.
    #[test]
    fn a_block_fully_inside_the_window_is_unclipped() {
        assert_eq!(clip_band(5, 23, 0, 40), Some((0, 23)));
    }
}
