//! gh #324 gap 1's transcript rows: image identity, reservation and the
//! placement cache.
//!
//! Split out of `tests.rs` for gate 11's 1,200-line ceiling. These are the rows the
//! filed bug is *about* - two images in one transcript - so they live beside the
//! rendering code rather than in the shared test module.

use super::*;

/// The theme the shared rows use: no SGR, so a width or id assertion is not
/// fighting style bytes.
fn plain() -> crate::theme::Theme {
    crate::theme::Theme::plain()
}

// Gh #324 gap 1 - image identity and row reservation.
//
// Two shapes are pinned here because the defect had two halves, and fixing only
// the id would leave the second image's rows painted over by the transcript row
// that follows it.

/// A minimal PNG header carrying real dimensions (the sniffer reads 16..24).
fn png_bytes(width: u32, height: u32) -> Vec<u8> {
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.extend_from_slice(&[0, 0, 0, 13]);
    png.extend_from_slice(b"IHDR");
    png.extend_from_slice(&width.to_be_bytes());
    png.extend_from_slice(&height.to_be_bytes());
    png.extend_from_slice(&[0; 12]);
    png
}

/// The kitty image ids any rendered row carries.
fn kitty_ids(line: &str) -> Vec<u32> {
    let mut ids = Vec::new();
    let mut rest = line;
    while let Some(at) = rest.find(",i=") {
        let tail = &rest[at + 3..];
        let digits: String = tail.chars().take_while(char::is_ascii_digit).collect();
        if let Ok(id) = digits.parse() {
            ids.push(id);
        }
        rest = &tail[digits.len()..];
    }
    ids
}

// Verifies: gh #324 gap 1 - two different images in one transcript keep distinct
// textures. The filed bug was a hardcoded `i=1`, so the second image overwrote the
// first: this asserts the ids differ *and* that both placements survive in the
// rendered document, not merely that rendering did not panic.
#[test]
fn two_transcript_images_get_distinct_kitty_ids_and_both_survive() {
    use lca_tui::widgets::image::ImageProtocol;
    let mut t = Transcript::new();
    let first = png_bytes(64, 64);
    let second = png_bytes(80, 80);
    t.push_image(ImageInfo::new("image/png", &first), first.clone());
    t.push_user("between them");
    t.push_image(ImageInfo::new("image/png", &second), second.clone());

    let rows = t.render_for_protocol(60, &plain(), ImageProtocol::Kitty);
    let mut ids = Vec::new();
    for row in &rows {
        ids.extend(kitty_ids(row));
    }
    assert_eq!(ids.len(), 2, "both images are placed: {ids:?}");
    assert_ne!(
        ids[0], ids[1],
        "the second image must not reuse the first's texture id"
    );
    // And each id belongs to *its own* payload: the id is derived from the bytes,
    // so recomputing it from the image the row was handed is the check that the
    // second image is not showing the first's texture.
    let expected: Vec<u32> = [&first, &second]
        .iter()
        .map(|bytes| {
            let info = ImageInfo::new("image/png", bytes);
            let (cols, rws) = lca_tui::widgets::image::image_cell_size_for_test(&info, 60);
            lca_tui::widgets::image::kitty_image_id(bytes, cols, rws)
        })
        .collect();
    assert_eq!(
        ids, expected,
        "each placement carries the id derived from its own bytes, in transcript order"
    );
}

// Verifies: gh #324 gap 1 - **identity is stable, not merely unique.** A
// monotonic counter fixes the collision and gives every repaint a new id, which
// re-uploads the bytes on every frame. The same image at the same width must
// derive the same id, so a second render is a placement; at a different width the
// id must follow (the scale is part of the placement, not the payload).
#[test]
fn the_same_image_keeps_its_id_across_renders_and_follows_the_width() {
    use lca_tui::widgets::image::ImageProtocol;
    // Deliberately larger than the pane: a small image is not scaled to fill, so
    // its cell scale would be identical at both widths and the row would prove
    // nothing about the scale being part of the id.
    let bytes = png_bytes(800, 600);
    let mut t = Transcript::new();
    // The default cap is 60 cells, so a wider *pane* alone does not reflow the
    // image - the cap has to move too, which is what narrowing the cap would do
    // for a real user's setting.
    t.apply_display(&crate::state::DisplayTuning {
        image_width_cells: 90,
        ..crate::state::DisplayTuning::default()
    });
    t.push_image(ImageInfo::new("image/png", &bytes), bytes.clone());

    let at_60 = kitty_ids(
        &t.render_for_protocol(60, &plain(), ImageProtocol::Kitty)
            .concat(),
    );
    let again = kitty_ids(
        &t.render_for_protocol(60, &plain(), ImageProtocol::Kitty)
            .concat(),
    );
    assert_eq!(at_60, again, "an unchanged frame re-derives the same id");

    let at_80 = kitty_ids(
        &t.render_for_protocol(80, &plain(), ImageProtocol::Kitty)
            .concat(),
    );
    assert_eq!(at_80.len(), 1);
    assert_ne!(
        at_60, at_80,
        "the id encodes the scale, so a reflowed image is a different upload"
    );

    // The complement, and the one that keeps a repaint a *placement*: an image
    // that already fits is not reflowed by a wider pane, so its id must not
    // change either - otherwise every resize would re-upload every small image.
    let small = png_bytes(48, 48);
    let mut t = Transcript::new();
    t.push_image(ImageInfo::new("image/png", &small), small.clone());
    let narrow = kitty_ids(
        &t.render_for_protocol(60, &plain(), ImageProtocol::Kitty)
            .concat(),
    );
    let wide = kitty_ids(
        &t.render_for_protocol(80, &plain(), ImageProtocol::Kitty)
            .concat(),
    );
    assert_eq!(
        narrow, wide,
        "an image whose scale did not change keeps its id"
    );
}

// Verifies: gh #324 gap 1 - the image **reserves its own rows.** The filed bug
// returned a single line for a `rows`-tall image, so the transcript row that
// followed painted straight over it. The count comes from the cell arithmetic,
// so this asserts the number, not "some rows".
#[test]
fn an_image_reserves_its_own_cell_rows_and_the_next_entry_survives() {
    use lca_tui::widgets::image::ImageProtocol;
    let mut t = Transcript::new();
    let bytes = png_bytes(90, 180);
    t.push_image(ImageInfo::new("image/png", &bytes), bytes.clone());
    t.push_user("the row after the image");

    let rows = t.render_for_protocol(60, &plain(), ImageProtocol::Kitty);
    let header = rows
        .iter()
        .position(|r| !kitty_ids(r).is_empty())
        .unwrap_or_else(|| panic!("the image paints a header row: {rows:?}"));
    let reserved = lca_tui::widgets::image::kitty_reserved_rows(&rows[header])
        .unwrap_or_else(|| panic!("the header states its rows: {:?}", rows[header]));

    // 90x180 px at a 9x18 cell is 10 columns and 10 rows of payload, all of
    // which fit inside 60 columns.
    assert_eq!(reserved, 10, "the reservation is the cell height");
    for row in &rows[header + 1..header + reserved as usize] {
        assert!(
            row.trim().is_empty(),
            "a reserved row holds no text: {row:?}"
        );
    }
    let after = rows
        .iter()
        .position(|r| r.contains("the row after the image"))
        .expect("the next entry is still rendered");
    assert!(
        after >= header + reserved as usize,
        "the transcript row after the image starts below its last reserved row \
         (image at {header}, {reserved} rows, next entry at {after})"
    );
}

// Verifies: gh #324 gap 1 - a very tall image is **clamped**, and the clamp is
// stated rather than silent: the reservation is bounded by the same 200-row
// estimate `image_cell_size` already carried, so a 4000x40000 image cannot ask
// for thousands of rows.
#[test]
fn a_very_tall_image_reserves_a_clamped_number_of_rows() {
    use lca_tui::widgets::image::ImageProtocol;
    let bytes = png_bytes(400, 40000);
    let info = ImageInfo::new("image/png", &bytes);
    let rows = lca_tui::widgets::image::render_image(&info, &bytes, ImageProtocol::Kitty, 60);
    let reserved = lca_tui::widgets::image::kitty_reserved_rows(&rows[0])
        .expect("the clamped image still states its rows");
    assert_eq!(
        reserved as usize,
        rows.len(),
        "the vector is exactly the reservation"
    );
    assert!(
        reserved <= 200,
        "the estimate is bounded at 200 rows, measured {reserved}"
    );
    assert!(reserved > 1, "a tall image still reserves many rows");
}

// Verifies: gh #324 gap 1 - the placeholder path most terminals get is
// byte-identical to what shipped. `tmux` and any terminal without graphics takes
// this arm, so a change to the kitty encoder must not reach it.
#[test]
fn the_placeholder_arm_is_byte_identical() {
    use lca_tui::widgets::image::ImageProtocol;
    let bytes = png_bytes(64, 32);
    let info = ImageInfo::new("image/png", &bytes);
    let now = lca_tui::widgets::image::render_image(&info, &bytes, ImageProtocol::None, 60);
    let expected = lca_tui::widgets::image::render_image_placeholder(&info, 60);
    assert_eq!(now, expected, "the None arm is the placeholder, unchanged");
    assert!(!now.is_empty());
}

// Verifies: gh #324 gap 1 - **a repaint of the transcript places, it does not
// transmit.** The first frame carries base64; the second, byte-identical frame
// must carry `a=p` and no payload. This is the half a monotonic id would have
// broken: the collision row above passes either way, only this one pins the cost.
#[test]
fn repainting_the_transcript_places_images_instead_of_re_uploading_them() {
    use lca_tui::widgets::image::ImageProtocol;
    let bytes = png_bytes(64, 64);
    let mut t = Transcript::new();
    t.push_image(ImageInfo::new("image/png", &bytes), bytes.clone());
    t.push_user("text after the image");

    let first = t.render_for_protocol(60, &plain(), ImageProtocol::Kitty);
    let upload = first
        .iter()
        .find(|r| r.contains("a=T"))
        .unwrap_or_else(|| panic!("the first frame transmits: {first:?}"))
        .clone();

    // Force a re-render: the cache keys on width, so this asks the transcript to
    // render the same document again rather than replaying a cached frame.
    let again = t.render_for_protocol(60, &plain(), ImageProtocol::Kitty);
    let placed = again
        .iter()
        .find(|r| r.contains("a=p"))
        .unwrap_or_else(|| panic!("the second frame places: {again:?}"));
    assert!(
        !placed.contains("a=T"),
        "a placement is not also a transmission: {placed:?}"
    );
    assert!(
        placed.len() * 2 < upload.len(),
        "the placement is smaller than the upload ({} vs {})",
        placed.len(),
        upload.len()
    );
    // The reserve survives the rewrite: same number of rows, blanks unchanged.
    assert_eq!(first.len(), again.len(), "the row count is the reservation");
    assert!(
        again.iter().any(|r| r.contains("text after the image")),
        "the entry after the image still renders"
    );
}
