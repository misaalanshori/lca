//! The kitty image placement cache (gh #324 gap 1): a repaint **places**, it does
//! not transmit.
//!
//! # The problem
//!
//! A kitty image line carries its whole base64 payload. `a=T` uploads the texture
//! and places it in one command, which is right the first time and wrong every
//! time after: scrolling past an image and back, resizing the pane, or any frame
//! that repaints that row re-sends kilobytes the terminal already holds. pi keeps
//! an `uploadedKittyImages` map for exactly this and replaces a known image with
//! `getKittyImagePlacement()`'s ~40-byte `a=p`.
//!
//! # The keys
//!
//! The cache is a set of image ids, not of rows or entry indices, because the id
//! is what "the same upload" means ([`lca_tui::widgets::image::kitty_image_id`]:
//! a hash of the payload *and* the cell scale). A shrink to a new width derives a
//! new id, misses the set, and uploads once at the new scale - which is correct,
//! because a rescaled image is genuinely different bytes to the terminal.
//!
//! No payload is stored: a row that has not been uploaded carries its own base64,
//! so the cache only has to answer "have I seen this id", and the row is already
//! the transmission.
//!
//! # The bound
//!
//! LRU over [`MAX_TRACKED`] ids, shared with the rest of the engine (`gh #323`'s
//! one cache policy). Evicting an id costs an upload the next time that image is
//! painted, which is a stutter rather than a bug - and the alternative is a
//! session-long set that grows with every image ever scrolled.

use lca_tui::engine::lru::LruCache;
use lca_tui::widgets::image::{kitty_placement, kitty_reserved_rows};

/// How many image ids are remembered.
///
/// 64 is comfortably more than a transcript shows at once and far less than a
/// session accumulates; past it, an image re-uploads when it comes back.
pub const MAX_TRACKED: usize = 64;

/// The ids this terminal already holds the texture for.
#[derive(Debug)]
pub struct PlacementCache {
    seen: LruCache<u32, ()>,
}

impl Default for PlacementCache {
    fn default() -> Self {
        Self::new()
    }
}

impl PlacementCache {
    /// A cache that has uploaded nothing yet.
    #[must_use]
    pub fn new() -> Self {
        Self {
            seen: LruCache::new(MAX_TRACKED),
        }
    }

    /// Whether `id` is known. Marks it most-recently-used.
    pub fn contains(&mut self, id: u32) -> bool {
        self.seen.get(&id).is_some()
    }

    /// Record that `id`'s texture is in the terminal.
    pub fn record(&mut self, id: u32) {
        self.seen.insert(id, ());
    }

    /// How many ids are held (never more than [`MAX_TRACKED`]).
    ///
    /// `cfg(test)`: the only reader is a row asserting the bound. Exposing it in
    /// production would be public API with no caller, which the workspace's
    /// dead-code lint rejects - and rightly, since `len` on a cache invites
    /// callers to depend on an implementation detail.
    #[cfg(test)]
    #[must_use]
    pub fn len(&self) -> usize {
        self.seen.len()
    }

    /// Whether nothing is held.
    #[cfg(test)]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.seen.is_empty()
    }

    /// Rewrite every kitty row whose image the terminal already holds into a
    /// placement form, and record the rest as now-uploaded.
    ///
    /// A row is left byte-identical when it is the first sight of its id, which is
    /// what makes the first frame of an image the transmission and every later
    /// frame a placement. A non-image row (the placeholder arm, ordinary text) is
    /// untouched.
    pub fn substitute(&mut self, rows: &mut [String]) {
        for row in rows.iter_mut() {
            // A reserved blank row below a header: not an image, nothing to do.
            if kitty_reserved_rows(row).is_none() {
                continue;
            }
            let Some(id) = row_id(row) else { continue };
            if self.contains(id) {
                if let Some(placement) = kitty_placement(row) {
                    *row = placement;
                }
            } else {
                self.record(id);
            }
        }
    }
}

/// The `i=` of a kitty row, if it carries one.
fn row_id(row: &str) -> Option<u32> {
    let start = row.find(",i=")? + 3;
    let digits: String = row[start..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use lca_tui::widgets::image::{ImageInfo, ImageProtocol, render_image};

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend_from_slice(&[0, 0, 0, 13]);
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&width.to_be_bytes());
        png.extend_from_slice(&height.to_be_bytes());
        png.extend_from_slice(&[0; 12]);
        png
    }

    fn rows_for(bytes: &[u8], width: usize) -> Vec<String> {
        render_image(
            &ImageInfo::new("image/png", bytes),
            bytes,
            ImageProtocol::Kitty,
            width,
        )
    }

    // Verifies: gh #324 gap 1 - **the first frame transmits, every later frame
    // places.** This is the property a monotonic id would have broken: it fixes
    // the collision and re-uploads on every scroll.
    #[test]
    fn the_first_paint_transmits_and_a_repaint_places() {
        let bytes = png(64, 64);
        let mut cache = PlacementCache::new();
        let mut first = rows_for(&bytes, 60);
        let header = first[0].clone();
        cache.substitute(&mut first);
        assert_eq!(first[0], header, "the first sight uploads, untouched");
        assert!(first[0].contains("a=T"), "and it is a transmission");

        let mut second = rows_for(&bytes, 60);
        cache.substitute(&mut second);
        assert!(
            second[0].contains("a=p"),
            "a repaint is a placement: {:?}",
            second[0]
        );
        assert!(!second[0].contains("AAAA"), "no payload on a repaint");
        // The property is "the payload is gone"; a 64-byte image's base64 is too
        // short for any length factor to be meaningful. The fixture needs a body
        // that does not compress to nothing: an all-zero PNG base64s to a handful
        // of characters, which is why the first version of this row measured
        // 39-vs-98 on a *400x400* image and proved nothing.
        let mut big = png(400, 400);
        big.extend((0..60_000u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 24) as u8));
        let mut cache = PlacementCache::new();
        let mut first = rows_for(&big, 60);
        cache.substitute(&mut first);
        let mut second = rows_for(&big, 60);
        cache.substitute(&mut second);
        assert!(
            second[0].len() * 100 < first[0].len(),
            "at real sizes a placement is under a hundredth of an upload ({} vs {})",
            second[0].len(),
            first[0].len()
        );
        // The reserve still stands: the blanks below the header are unchanged, so
        // the row count a renderer walks is the same either way.
        assert_eq!(first.len(), second.len());
        assert_eq!(first[1..], second[1..]);
    }

    // Verifies: gh #324 gap 1 - **two images are tracked separately.** The filed
    // bug shared one id, so the second overwrote the first; here each repaints as
    // its own placement and neither is mistaken for the other.
    #[test]
    fn two_images_are_tracked_and_placed_separately() {
        let (a, b) = (png(64, 64), png(80, 80));
        let mut cache = PlacementCache::new();
        let mut frame = rows_for(&a, 60);
        frame.extend(rows_for(&b, 60));
        cache.substitute(&mut frame);
        assert!(frame[0].contains("a=T"), "the first uploads");
        assert!(
            frame.iter().filter(|r| r.contains("a=T")).count() == 2,
            "both upload on their first frame"
        );
        assert_eq!(cache.len(), 2, "two ids are tracked");

        let mut again = rows_for(&a, 60);
        again.extend(rows_for(&b, 60));
        cache.substitute(&mut again);
        assert!(
            again.iter().all(|r| !r.contains("a=T")),
            "neither re-uploads: {again:?}"
        );
        assert_eq!(again.iter().filter(|r| r.contains("a=p")).count(), 2);
    }

    // Verifies: gh #324 gap 1 - a **rescaled** image is a different upload, because
    // its cell scale is part of the id. A pane resize re-sends once at the new
    // scale rather than placing a texture the terminal cannot rescale.
    #[test]
    fn a_rescaled_image_is_a_new_upload() {
        let bytes = png(800, 600);
        let mut cache = PlacementCache::new();
        let mut wide = rows_for(&bytes, 80);
        cache.substitute(&mut wide);
        assert!(wide[0].contains("a=T"));

        let mut narrow = rows_for(&bytes, 40);
        cache.substitute(&mut narrow);
        assert!(
            narrow[0].contains("a=T"),
            "a new scale must upload, not place: {:?}",
            narrow[0]
        );
        assert_eq!(cache.len(), 2, "both scales are remembered");
    }

    // Verifies: gh #324 gap 1 - the cache is **bounded**, so a long session of
    // scrolling images cannot accumulate ids forever.
    #[test]
    fn the_cache_is_bounded_and_evicts_the_least_recently_used() {
        let mut cache = PlacementCache::new();
        for id in 1..=(MAX_TRACKED as u32 + 8) {
            cache.record(id);
        }
        assert_eq!(cache.len(), MAX_TRACKED, "the bound holds");
        assert!(!cache.contains(1), "the oldest id was evicted");
        assert!(
            cache.contains(MAX_TRACKED as u32 + 8),
            "the newest survived"
        );
    }

    // Verifies: gh #324 gap 1 - **the placeholder arm is untouched.** A transcript
    // with no graphics support renders text rows, and the cache must not rewrite
    // any of them.
    #[test]
    fn non_image_rows_are_left_alone() {
        let mut cache = PlacementCache::new();
        let mut rows = vec![
            "[image image/png · 64×64 · 12 B]".to_string(),
            String::new(),
        ];
        let before = rows.clone();
        cache.substitute(&mut rows);
        assert_eq!(rows, before, "a placeholder is not an image block");
        assert!(cache.is_empty());
    }
}
