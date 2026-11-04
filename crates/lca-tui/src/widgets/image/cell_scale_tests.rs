//! The cell-size probe's rows (gh #324 gap 2): the reply changes the arithmetic, the grid
//! is chosen by least distortion, and no reply still renders.
//!
//! Split out of `image.rs` for gate 11's 1,200-line ceiling.

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

// Verifies: gh #324 gap 2 - **a `CellSize` reply changes the arithmetic.**
// Every terminal that is not 9x18 got a distorted image because the scale was
// hardcoded; this is the wiring row, and it asserts the *number of rows*, not
// just that something moved.
#[test]
fn a_cell_size_reply_changes_the_reserved_rows() {
    let bytes = png(90, 180);
    let info = ImageInfo::new("image/png", &bytes);
    // 90x180 px on a 9x18 cell is 10 columns by 10 rows.
    let estimated = image_cell_size(&info, 60);
    assert_eq!(estimated, (10, 10), "the 9x18 estimate");

    // A taller cell (22 px) fits fewer rows of the same image.
    let tall = image_cell_size_with(&info, 60, (9.0, 22.0));
    let wide = image_cell_size_with(&info, 60, (12.0, 18.0));
    assert_ne!(tall, estimated, "a taller cell changes the rows: {tall:?}");
    assert_ne!(
        wide, estimated,
        "a wider cell changes the columns and, through the aspect ratio, the rows"
    );
}

// Verifies: gh #324 gap 2 - **the grid is chosen by least distortion**, not by
// rounding up. pi compares the achieved aspect ratio against the image's and
// picks `ceil` or `ceil - 1`, whichever distorts less; a rounded-up grid can be
// twice as tall as the picture asks for.
//
// The metric is `max(count/ideal, ideal/count)` - symmetric, so a grid 2x too
// tall and one 2x too wide score the same, which is what "distortion" means for
// an image.
#[test]
fn the_grid_is_chosen_by_least_distortion_not_by_rounding_up() {
    // A case where the naive answer differs. `ceil` says 3; the crossover for
    // upper = 3 is at sqrt(6) ~= 2.449 (measured, not guessed - `ideal = 2.49`
    // sits just *above* it, where 3 really is the closer grid, which is why my
    // first version of this row asserted the wrong answer).
    assert_eq!(
        choose_less_distorted_cell_count(3, 2.4),
        2,
        "2.4 ideal: ceil says 3, least-distortion says 2 (3 is 25% too tall, 2 only 20% too short)"
    );
    assert_eq!(
        choose_less_distorted_cell_count(3, 2.45),
        3,
        "2.45 is past the sqrt(6) crossover, so ceil wins"
    );
    // The complement: `ceil` is closer, so it stays.
    assert_eq!(
        choose_less_distorted_cell_count(3, 2.99),
        3,
        "2.99 ideal: ceil is the closer grid"
    );
    // Ties go to the upper count, which is what pi does
    // (`lowerDistortion < upperDistortion`), so a 2.5 ideal keeps 3.
    assert_eq!(
        choose_less_distorted_cell_count(3, 2.5),
        3,
        "a tie keeps ceil"
    );
    // Never below one row.
    assert_eq!(choose_less_distorted_cell_count(1, 3.0), 1, "one row floor");
}

// Verifies: gh #324 gap 2 - the metric reaches `image_cell_size_with`, not just
// the helper, and it **changes the answer** from naive rounding.
//
// A 60x110 picture on a 9x18 cell: 7 columns, and 110/18 = 6.11 rows scaled by
// 7/6.67 and rounded up to **7**. The ideal is 6.417, so the naive grid is 9%
// too tall while `ceil - 1` is 6.5% too short - the metric takes 6, and naive
// rounding would have said 7. Pinned as a number because a row that recomputes
// the comparison would agree with whatever the implementation happened to do.
#[test]
fn the_distortion_metric_reaches_the_cell_size_calculation() {
    let bytes = png(60, 110);
    let info = ImageInfo::new("image/png", &bytes);
    assert_eq!(
        image_cell_size_with(&info, 60, DEFAULT_CELL_PIXELS),
        (7, 6),
        "7 columns by 6 rows: the metric's answer, where `ceil` would say 7 rows"
    );
    // The same picture at the naive rounding, written the way the code under
    // test computes it: `ceil((h / cell_h) * (cols / natural_cols))`.
    let natural_cols = (60.0_f64 / 9.0).ceil();
    let ceil_rows = ((110.0_f64 / 18.0) * (7.0 / natural_cols)).ceil();
    assert_eq!(
        ceil_rows, 7.0,
        "and `ceil` alone really would have said 7 rows"
    );
    assert_eq!(
        choose_less_distorted_cell_count(7, 6.417),
        6,
        "the helper's answer for that pair"
    );
}

// Verifies: gh #324 gap 2 - **no reply still renders, with the labelled
// estimate.** A terminal that never answers must not lose its image, and the
// estimate must be the documented constant rather than a second literal.
#[test]
fn no_reply_uses_the_named_estimate_and_still_renders() {
    let bytes = png(90, 180);
    let info = ImageInfo::new("image/png", &bytes);
    // With nothing installed, the scale is exactly `DEFAULT_CELL_PIXELS`.
    assert_eq!(current_cell_pixels(), DEFAULT_CELL_PIXELS);
    assert_eq!(
        image_cell_size(&info, 60),
        image_cell_size_with(&info, 60, DEFAULT_CELL_PIXELS),
        "the no-probe path is the estimate, not an accident"
    );
    let lines = render_image(&info, &bytes, ImageProtocol::Kitty, 60);
    assert!(!lines.is_empty(), "an image renders without a probe");
    assert!(lines[0].contains("\x1b_G"), "and it is a real placement");
}

// Verifies: gh #324 gap 2 - **the scale is part of the image's identity.** Gap
// 1 established that the id hashes the payload *and* the cell scale, so a probe
// arriving after first paint must produce a new id (a new upload) rather than a
// silent re-scale that leaves the terminal holding a texture whose geometry no
// longer matches its placement.
#[test]
fn a_probe_arriving_after_first_paint_changes_the_id() {
    let bytes = png(90, 180);
    let info = ImageInfo::new("image/png", &bytes);
    let before = image_cell_size(&info, 60);
    let id_before = kitty_image_id(&bytes, before.0, before.1);

    assert!(probe_cell_pixels(9, 22), "the probe changes the scale");
    let after = image_cell_size(&info, 60);
    let id_after = kitty_image_id(&bytes, after.0, after.1);
    clear_cell_pixels();

    assert_ne!(before, after, "the probe moved the grid");
    assert_ne!(
        id_before, id_after,
        "so the id moves too: a probe is a new upload, not a re-scale"
    );
    // And the store is back to the estimate, so no later row inherits it.
    assert_eq!(current_cell_pixels(), DEFAULT_CELL_PIXELS);
}
