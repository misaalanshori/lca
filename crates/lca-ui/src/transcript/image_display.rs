//! The display derivative: an image kitty cannot transmit is drawn as a PNG
//! (gh #599).
//!
//! # The shape, and the measurement that decided it
//!
//! A vision model's resize arrives as **JPEG q80** (`lca_tools::image`'s
//! `downscale_image`), and kitty's protocol names no code for JPEG (gh #598) - so a
//! resized attachment painted a label where a picture belongs, while iTerm2 (which
//! sniffs the payload) has always drawn it. The fix is a **render-time PNG
//! derivative** of the stored wire bytes, at the terminal's cell width, cached per
//! entry. It is deliberately **not** a second persisted attachment with a second
//! hash: a display copy's correct pixel size is the pane's, which the session cannot
//! know when it writes the record. Measured on a 2000×1333 photo-like fixture, the
//! display PNG is **2.06x** the wire JPEG at 60 cells and **0.85x** at 40, so a
//! persisted copy would be the wrong size at one of them and a re-encode at both.
//!
//! Nothing is stored, hashed, or walked differently: the record keeps its JPEG hash,
//! `assemble` sends the JPEG to the model, gc and export are untouched, and there is
//! no record field, no version bump and no migration.
//!
//! # The box is carried, not re-derived
//!
//! The encoder fits the picture *within* the cell box to keep the aspect ratio, so
//! the derivative's own pixel dimensions re-derive an **older** box: measured over 19
//! dimension pairs × 5 widths, 18 of 95 boxes disagree with the box they were encoded
//! for (3000×100 at 80 cells comes back as 60). The box is therefore what the
//! renderer chose - `c=`/`r=` then stretch the payload exactly as they stretch a PNG
//! today - and the crop's pixel height comes from the derivative, so a scrolled band
//! stays aligned with the texture the terminal holds.
//!
//! # The cache is the point
//!
//! A renderer that encodes is a renderer that can stutter, so the derivative is
//! memoised by `(entry index, cell width)` and the memo is **validated by
//! `Arc::ptr_eq`** against the wire bytes: a cleared-and-replayed transcript hands
//! fresh `Arc`s, which miss and re-encode, while a redraw at the same width hits. Two
//! frames at one width encode **once**; a new width encodes once more. The census is
//! a `Cell` because "did the encoder run again" is invisible in the painted bytes -
//! the same lesson as gh #324's derived id, one layer over.

use std::cell::{Cell, RefCell};
use std::sync::Arc;

use lca_tui::engine::lru::LruCache;
use lca_tui::widgets::image::{
    ImageInfo, ImageProtocol, encode_kitty, image_cell_size, kitty_format_for, kitty_image_id,
};

/// The media type of the derivative. It is not a claim about the wire bytes - it is
/// the format this module's own encoder produced, and `kitty_format_for` is the one
/// place the protocol's `f=` code is chosen (gh #598).
const DISPLAY_MEDIA_TYPE: &str = "image/png";

/// The wire type that gets a derivative: the one the resize path produces.
///
/// A webp or gif stays a placeholder, because gh #598's rows pin that refusal and
/// converting them would be a format change nobody asked for - a gif would also lose
/// its animation. A PNG needs no derivative: kitty transmits it already.
const DERIVED_MEDIA_TYPE: &str = "image/jpeg";

/// How many derivatives are remembered: a transcript's images at up to a few widths.
///
/// **Smaller than the placement cache's 64 on purpose**, because this memo stores the
/// payload's bytes and that one stores only ids: a derivative at the default 60-cell box
/// is ~190 KB for a photo-like picture (measured on the read tool's q80 fixture), so 16
/// of them is ~3 MB at the default cap; a user who raises `terminal.image_width_cells`
/// to 200 raises that to ~18 MB. Evicting one costs an encode on the next paint of that
/// image, which is a stutter rather than a bug - the same trade the placement cache
/// documents, one layer over.
const MAX_DERIVATIVES: usize = 16;

/// One encoded derivative, with the cell box it was produced for.
#[derive(Debug, Clone)]
pub(super) struct Derived {
    png: Arc<Vec<u8>>,
    /// The cell box the renderer chose (`c=`/`r=`), carried rather than re-derived.
    pub(super) columns: u32,
    /// The box's rows, which is also how many document rows the block reserves.
    pub(super) rows: u32,
    /// The payload's own pixel height, for the window crop's band arithmetic.
    pub(super) height: u32,
}

/// A memo key: which entry, at which cell width. The width is in the key because the
/// box (and therefore the payload) is the width's.
type MemoKey = (usize, u32);

/// A memo entry: the wire bytes it was derived from, and the derivative. The pair is
/// what makes `Arc::ptr_eq` validation possible without a second map.
type MemoEntry = (Arc<Vec<u8>>, Derived);

/// The derived photos this transcript has encoded, and how many encodes it made.
#[derive(Debug)]
pub(super) struct ImageDisplay {
    cache: RefCell<LruCache<MemoKey, MemoEntry>>,
    encodes: Cell<usize>,
}

impl Default for ImageDisplay {
    fn default() -> Self {
        Self {
            cache: RefCell::new(LruCache::new(MAX_DERIVATIVES)),
            encodes: Cell::new(0),
        }
    }
}

impl ImageDisplay {
    /// The kitty rows for entry `index` at `max_width` columns, encoding at most once
    /// per `(entry, width)`.
    ///
    /// `None` means "use the ordinary ladder": the terminal is not kitty (iTerm2 sniffs
    /// and needs no derivative), or the image is not a JPEG, or the bytes do not
    /// decode - the caller then gets the placeholder or the native PNG, never a failed
    /// frame.
    pub(super) fn kitty_lines_for(
        &self,
        index: usize,
        info: &ImageInfo,
        bytes: &Arc<Vec<u8>>,
        max_width: usize,
        protocol: ImageProtocol,
    ) -> Option<Vec<String>> {
        if protocol != ImageProtocol::Kitty || info.media_type != DERIVED_MEDIA_TYPE {
            return None;
        }
        let key: MemoKey = (index, max_width as u32);
        let derived = match self.hit(&key, bytes) {
            Some(derived) => derived,
            None => {
                let (columns, rows) = image_cell_size(info, max_width);
                let (png, height) = lca_tools::display_png(bytes, (columns, rows), cell_pixels())?;
                self.encodes.set(self.encodes.get().wrapping_add(1));
                let derived = Derived {
                    png: Arc::new(png),
                    columns,
                    rows,
                    height,
                };
                self.cache
                    .borrow_mut()
                    .insert(key, (Arc::clone(bytes), derived.clone()));
                derived
            }
        };
        kitty_rows(&derived)
    }

    /// The derived pixel height for entry `index` at `max_width`, when one was
    /// already encoded. A cold cache answers `None`, and the caller keeps the wire
    /// height it has - a transcript nobody has painted has no image rows to crop.
    pub(super) fn height_for(
        &self,
        index: usize,
        bytes: &Arc<Vec<u8>>,
        max_width: usize,
    ) -> Option<u32> {
        self.hit(&(index, max_width as u32), bytes)
            .map(|derived| derived.height)
    }

    /// Forget every derivative, because the cell size or the display settings moved
    /// and the memo's pixels are the wrong ones. A `LruCache` has no `clear`, and
    /// replacing it is the same one line a clear would be.
    pub(super) fn clear(&self) {
        *self.cache.borrow_mut() = LruCache::new(MAX_DERIVATIVES);
    }

    /// How many encodes this transcript has made (the census `Transcript` exposes).
    pub(super) fn encodes(&self) -> usize {
        self.encodes.get()
    }

    /// A cached derivative, when the entry still holds the same wire bytes.
    fn hit(&self, key: &MemoKey, bytes: &Arc<Vec<u8>>) -> Option<Derived> {
        let mut cache = self.cache.borrow_mut();
        let (wire, derived) = cache.get(key)?;
        Arc::ptr_eq(&wire, bytes).then(|| derived.clone())
    }
}

/// One transmission row, then the reserved blanks, for a derivative.
///
/// `None` when the protocol has no code for a PNG, which cannot happen while
/// `kitty_format_for` maps `image/png` - and answering `None` rather than guessing a
/// code keeps the refusal in the encoder's one place.
fn kitty_rows(derived: &Derived) -> Option<Vec<String>> {
    let format = kitty_format_for(DISPLAY_MEDIA_TYPE)?;
    let id = kitty_image_id(&derived.png, derived.columns, derived.rows);
    let mut lines = Vec::with_capacity(derived.rows as usize);
    lines.push(encode_kitty(
        &derived.png,
        derived.columns,
        derived.rows,
        id,
        format,
    ));
    lines.resize(derived.rows as usize, String::new());
    Some(lines)
}

/// The cell size the terminal actually reports, or the estimate (gh #324 gap 2).
fn cell_pixels() -> (f64, f64) {
    lca_tui::widgets::image::current_cell_pixels()
}
