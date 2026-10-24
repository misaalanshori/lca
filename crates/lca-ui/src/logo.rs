//! The LCA logo (gh #399): the owner's grid, drawn as half blocks, with the
//! logo's own colours.
//!
//! # The grid is data, not art
//!
//! The owner supplied five rows of nine cells
//! (`pi-opencode_uiux-study/owner_notes.md`), each cell naming a letter or blank:
//!
//! ```text
//! l.ccc.aa.
//! l.c...a.a
//! l.c...aaa
//! l.ccc.a.a
//! lllllllll
//! ```
//!
//! Half blocks put **two pixel rows in one cell**: `▀` is the top pixel, `▄` the
//! bottom, `█` both. Five pixel rows therefore draw as three terminal rows, and the
//! conversion is mechanical — [`grid_rows`] pairs row `n` with row `n+1`. That is
//! why the shipped rows read `█ █▀▀ █▀▄` / `█ █▄▄ █▀█` / `▀▀▀▀▀▀▀▀▀`, which is
//! exactly the composition in the owner's sketch. A prettified variant would be a
//! different logo, so the grid above is the source of truth and the rows below are
//! derived from it rather than typed out.
//!
//! # The colours are the logo's, not the theme's
//!
//! `l` → `#62aedc`, `c` → `#e28c00`, `a` → `#eccd00`. The owner was explicit that
//! these are icon colours, not the app theme, so [`LOGO_COLOURS`] is a private
//! constant table that no [`Role`] refers to and that [`Theme::role`] never sees.
//! Nothing here reads the palette, which is what makes a `/theme` switch unable to
//! move them — the separation is structural, not a promise.
//!
//! They *do* follow the terminal's colour depth, the way pi's `foregroundAnsi`
//! takes a mode: truecolor prints the exact hex, and a 16-colour theme caps each
//! channel to its nearest ANSI index so the mark stays legible instead of
//! disappearing. The cap changes how the colour is spelled, never which colour the
//! logo asked for.
//!
//! # Why there is a wordmark
//!
//! Apple's Terminal misrenders half blocks — gaps between rows, and halves that do
//! not meet — so the mark becomes solid text (`LCA`, in the logo's colours). The
//! question is [`lca_tui::engine::terminal::is_apple_terminal`]; this module only
//! draws the answer. A terminal too narrow for the mark gets the same wordmark,
//! and then nothing, rather than a wrapped logo.

use super::theme::{ColorSupport, Theme};

/// How the mark is drawn at the current terminal.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mark {
    /// The owner's grid as half blocks — the full mark.
    HalfBlocks,
    /// `LCA` in the logo's own colours: a terminal that cannot draw the grid,
    /// or one too narrow to hold it beside the version.
    Wordmark,
    /// Nothing: too narrow even for the wordmark.
    None,
}

/// The owner's grid: five pixel rows, nine cells each. `.` is blank.
///
/// Kept as the letters the owner wrote rather than as derived escapes, so the
/// shipped rows can be checked against the source they came from.
pub const GRID: [&str; 5] = [
    "l.ccc.aa.",
    "l.c...a.a",
    "l.c...aaa",
    "l.ccc.a.a",
    "lllllllll",
];

/// The logo's three colours, as the owner specified them. Not theme roles.
const LOGO_COLOURS: [(char, (u8, u8, u8)); 3] = [
    ('l', (0x62, 0xae, 0xdc)),
    ('c', (0xe2, 0x8c, 0x00)),
    ('a', (0xec, 0xcd, 0x00)),
];

/// The RGB for one grid letter, or `None` for a blank cell.
pub fn logo_colour(letter: char) -> Option<(u8, u8, u8)> {
    LOGO_COLOURS
        .iter()
        .find(|(name, _)| *name == letter)
        .map(|(_, rgb)| *rgb)
}

/// One terminal row's glyph and the letter that colours it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Cell {
    /// The glyph: a full block, a half block, or a space.
    pub glyph: char,
    /// The grid letter whose colour paints it, when one does.
    pub letter: Option<char>,
}

/// The mark as terminal cells: five pixel rows pair into three terminal rows.
///
/// The pairing is the whole transformation, and it is mechanical on purpose: for
/// each pair, a cell that is filled top and bottom is `█`, top only `▀`, bottom
/// only `▄`, and neither a space. A row left over at the bottom pairs with blanks.
pub fn grid_rows() -> Vec<Vec<Cell>> {
    let mut rows = Vec::new();
    for pair in GRID.chunks(2) {
        let top = pair[0].chars().collect::<Vec<_>>();
        let bottom = pair
            .get(1)
            .map(|row| row.chars().collect::<Vec<_>>())
            .unwrap_or_else(|| vec!['.'; top.len()]);
        rows.push(
            (0..top.len())
                .map(|column| {
                    let upper = top.get(column).copied().unwrap_or('.');
                    let lower = bottom.get(column).copied().unwrap_or('.');
                    cell(upper, lower)
                })
                .collect::<Vec<_>>(),
        );
    }
    rows
}

/// One cell from its upper and lower pixel.
fn cell(upper: char, lower: char) -> Cell {
    match (upper == '.', lower == '.') {
        (true, true) => Cell {
            glyph: ' ',
            letter: None,
        },
        (false, false) => Cell {
            glyph: '█',
            letter: Some(upper),
        },
        (false, true) => Cell {
            glyph: '▀',
            letter: Some(upper),
        },
        (true, false) => Cell {
            glyph: '▄',
            letter: Some(lower),
        },
    }
}

/// How wide the mark is in cells: the grid's own width, which is the same for
/// every row because the owner drew it square.
pub fn mark_width() -> usize {
    GRID[0].chars().count()
}

/// The mark's rows painted with the logo colours, or wordmark rows when the
/// terminal cannot take half blocks.
///
/// `coloured` comes from the theme (a plain theme prints no escapes at all,
/// FR-UI-5), and `support` decides whether the exact hex survives or is capped to
/// the ANSI ladder. Neither changes *which* colour, only how it is spelled.
pub fn mark_rows(theme: &Theme, mark: Mark) -> Vec<String> {
    match mark {
        Mark::HalfBlocks => grid_rows()
            .into_iter()
            .map(|row| {
                row.into_iter()
                    .map(|cell| {
                        match cell.letter.and_then(logo_colour) {
                            Some(rgb) => paint(theme, rgb, cell.glyph),
                            // A blank cell carries no background, so it prints the
                            // space the grid asked for and nothing else.
                            None => " ".to_string(),
                        }
                    })
                    .collect::<String>()
            })
            .collect(),
        Mark::Wordmark => vec![wordmark(theme)],
        Mark::None => Vec::new(),
    }
}

/// `LCA` in the logo's colours: the fallback's own shape.
///
/// Each letter takes its grid colour, so the wordmark reads as the same mark
/// rather than as a plain string that happens to be the product name.
pub fn wordmark(theme: &Theme) -> String {
    ['L', 'C', 'A']
        .iter()
        .enumerate()
        .map(|(index, letter)| {
            let rgb = logo_colour(match index {
                0 => 'l',
                1 => 'c',
                _ => 'a',
            })
            .unwrap_or((0x62, 0xae, 0xdc));
            paint(theme, rgb, *letter)
        })
        .collect()
}

/// Paint one glyph in a logo colour, or return it bare when the theme is plain.
///
/// The close is `39` (foreground default) rather than `0`: a logo cell must not
/// reset a style it did not open (`theme.md` §2's channel-scoped reset).
fn paint(theme: &Theme, rgb: (u8, u8, u8), glyph: char) -> String {
    if !theme.colored {
        return glyph.to_string();
    }
    let (r, g, b) = rgb;
    let colour = match theme.color_support {
        ColorSupport::Ansi16 => {
            // The exact hex would be quoted as true color on a terminal that
            // cannot show it; cap to the nearest of the sixteen instead so the
            // three letters stay distinguishable.
            let index = nearest_16(r, g, b);
            let base = if index < 8 { 30 } else { 90 };
            format!("{}", base + (index % 8))
        }
        _ => format!("38;2;{r};{g};{b}"),
    };
    format!("\u{1b}[{colour}m{glyph}\u{1b}[39m")
}

/// The nearest of the sixteen ANSI colours: squared distance over the VGA table,
/// the same table the theme's own cap uses.
fn nearest_16(r: u8, g: u8, b: u8) -> u8 {
    const TABLE: [(u8, u8, u8); 16] = [
        (0, 0, 0),
        (170, 0, 0),
        (0, 170, 0),
        (170, 85, 0),
        (0, 0, 170),
        (170, 0, 170),
        (0, 170, 170),
        (170, 170, 170),
        (85, 85, 85),
        (255, 85, 85),
        (85, 255, 85),
        (255, 255, 85),
        (85, 85, 255),
        (255, 85, 255),
        (85, 255, 255),
        (255, 255, 255),
    ];
    let mut best = 0u8;
    let mut best_distance = u32::MAX;
    for (index, (tr, tg, tb)) in TABLE.iter().enumerate() {
        let distance = (i32::from(r) - i32::from(*tr)).pow(2)
            + (i32::from(g) - i32::from(*tg)).pow(2)
            + (i32::from(b) - i32::from(*tb)).pow(2);
        if (distance as u32) < best_distance {
            best_distance = distance as u32;
            best = index as u8;
        }
    }
    best
}

/// Which drawing to use for a terminal `width` columns wide and this
/// [`lca_tui::engine::terminal::is_apple_terminal`] answer.
///
/// The rule is a ladder, and each rung exists for a reason: the half-block mark
/// needs its own nine columns plus the version beside it; the wordmark is three;
/// below that nothing rather than a logo wrapped across two rows, which would read
/// as a rendering fault.
pub fn mark_for(width: usize, apple_terminal: bool) -> Mark {
    if width >= mark_width() + 2 {
        if apple_terminal {
            Mark::Wordmark
        } else {
            Mark::HalfBlocks
        }
    } else if width >= 3 {
        Mark::Wordmark
    } else {
        Mark::None
    }
}

/// The logo hex values, for the row that keeps them out of the palette.
///
/// Exposed so a test can assert the exact three values the owner specified, and
/// that the palette source contains none of them — the guard for "the colours are
/// the logo's, not the theme's".
pub fn logo_hex_values() -> [&'static str; 3] {
    ["#62aedc", "#e28c00", "#eccd00"]
}

#[cfg(test)]
#[path = "logo_tests.rs"]
mod tests;
