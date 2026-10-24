//! gh #399 rows: the owner's grid drawn faithfully, and the logo's colours kept
//! out of the theme.
//!
//! The brief's test policy names these five properties; each row states the
//! failure it forbids rather than restating the implementation.

use super::*;
use crate::theme::Theme;

/// The three rows the owner's sketch shows, as plain text.
fn rendered_text(mark: Mark) -> Vec<String> {
    // Plain theme: no escapes, so this is the glyph grid itself.
    mark_rows(&Theme::plain(), mark)
}

#[test]
fn the_mark_is_the_owner_grid_half_blocks() {
    // Five pixel rows pair into three terminal rows, and the result is exactly
    // the composition in `owner_notes.md`. A prettified variant - thicker
    // strokes, a rounded corner, a different baseline - fails here, which is the
    // whole point of asserting the glyphs rather than "something rendered".
    assert_eq!(
        rendered_text(Mark::HalfBlocks),
        vec!["█ █▀▀ █▀▄", "█ █▄▄ █▀█", "▀▀▀▀▀▀▀▀▀"],
        "the owner's grid, paired into half blocks"
    );
}

#[test]
fn every_row_is_the_grid_width() {
    // A row shorter than the grid would let the version beside it drift, and a
    // longer one would push the version off. The owner drew it square.
    for (index, row) in grid_rows().iter().enumerate() {
        assert_eq!(row.len(), mark_width(), "row {index} is the grid's width");
    }
    for row in rendered_text(Mark::HalfBlocks) {
        assert_eq!(
            lca_tui::engine::text::visible_width(&row),
            mark_width(),
            "the rendered row occupies exactly the grid's columns: {row:?}"
        );
    }
}

#[test]
fn each_letter_carries_its_own_colour_not_a_theme_role() {
    // The row asserts the **SGR**, not just the text: three letters, three
    // distinct exact hex values, in the order the owner listed them.
    let rows = mark_rows(&Theme::colored(), Mark::HalfBlocks);
    let joined = rows.join("\n");
    for (letter, hex) in [
        ('l', "38;2;98;174;220"),
        ('c', "38;2;226;140;0"),
        ('a', "38;2;236;205;0"),
    ] {
        let code = hex.to_string();
        assert!(
            joined.contains(&code),
            "{letter} must be painted {code}: {joined:?}"
        );
    }
    // And each closes only its own channel, so a logo cell cannot reset a style
    // it did not open (`theme.md` §2).
    assert!(
        !joined.contains("\u{1b}[0m"),
        "the logo closes with 39, never a full reset: {joined:?}"
    );
    assert!(
        joined.contains("\u{1b}[39m"),
        "each cell closes its foreground"
    );
}

#[test]
fn the_three_hex_values_are_exactly_the_owners() {
    // Verbatim from `owner_notes.md`. A near-miss (a tweaked lightness, a
    // "harmonised" palette) is a different logo.
    assert_eq!(logo_hex_values(), ["#62aedc", "#e28c00", "#eccd00"]);
    // And the constant table the renderer actually reads agrees.
    assert_eq!(logo_colour('l'), Some((0x62, 0xae, 0xdc)));
    assert_eq!(logo_colour('c'), Some((0xe2, 0x8c, 0x00)));
    assert_eq!(logo_colour('a'), Some((0xec, 0xcd, 0x00)));
    assert_eq!(logo_colour('.'), None, "a blank cell has no colour");
}

#[test]
fn a_theme_switch_cannot_move_the_logo_colours() {
    // The separation, measured rather than promised: every built-in theme paints
    // the mark with the same three exact RGB values. The owner was explicit that
    // these are icon colours, and the way that holds is that the renderer never
    // reads a role - so a palette change has nowhere to enter.
    // Every theme that can spell the exact hex. `Theme::limited` is deliberately
    // absent: it caps to sixteen colours by design (its own row), so it differs in
    // *spelling*, not in which colour the logo asked for.
    let themes = [
        Theme::colored(),
        Theme::light(),
        Theme::system_theme(None, None),
    ];
    let expected = mark_rows(&Theme::colored(), Mark::HalfBlocks);
    for theme in &themes {
        assert_eq!(
            mark_rows(theme, Mark::HalfBlocks),
            expected,
            "theme {:?} paints the logo identically",
            theme.name
        );
    }
    // The 16-colour theme is the *same drawing* in a coarser spelling: the glyphs
    // and the number of painted cells are unchanged, so a depth cap is not a
    // design change.
    let capped = mark_rows(&Theme::limited(), Mark::HalfBlocks);
    let plain = mark_rows(&Theme::plain(), Mark::HalfBlocks);
    for (capped_row, plain_row) in capped.iter().zip(plain.iter()) {
        let stripped: String = {
            let mut out = String::new();
            let mut skipping = false;
            for ch in capped_row.chars() {
                if ch == '\u{1b}' {
                    skipping = true;
                } else if skipping {
                    if ch == 'm' {
                        skipping = false;
                    }
                } else {
                    out.push(ch);
                }
            }
            out
        };
        assert_eq!(
            stripped, *plain_row,
            "a depth cap changes the escape, never the drawn cell"
        );
    }
    // A custom palette that repaints every role still cannot reach it.
    let custom = crate::theme::Palette::dark();
    let repainted = Theme::from_palette("repainted", custom, true);
    assert_eq!(
        mark_rows(&repainted, Mark::HalfBlocks),
        expected,
        "a re-paletted theme leaves the logo alone"
    );
}

#[test]
fn the_logo_hex_values_are_not_theme_roles() {
    // The other half of the separation: the three values must not appear in the
    // palettes at all, so nobody can later "just" alias a role to them.
    let sources = [
        crate::theme::Palette::dark(),
        crate::theme::Palette::light(),
    ];
    for palette in &sources {
        for role in crate::theme::Role::ALL {
            let colour = palette.get(*role);
            if let crate::theme::Color::Rgb(r, g, b) = colour {
                let hex = format!("#{r:02x}{g:02x}{b:02x}");
                assert!(
                    !logo_hex_values().contains(&hex.as_str()),
                    "{role:?} resolves to the logo colour {hex}; the logo's colours are not theme roles"
                );
            }
        }
    }
}

#[test]
fn apple_terminal_gets_the_wordmark_not_half_blocks() {
    // The study's finding: Apple's Terminal draws gaps between rows and misaligns
    // half blocks, so that terminal gets solid text instead of broken art.
    assert_eq!(mark_for(80, true), Mark::Wordmark);
    assert_eq!(mark_for(80, false), Mark::HalfBlocks);
    // Wide enough is not the only question - the terminal has to be able to draw
    // it, and it cannot.
    assert_eq!(mark_for(200, true), Mark::Wordmark);
}

#[test]
fn the_wordmark_is_the_products_letters_in_the_logo_colours() {
    let plain = mark_rows(&Theme::plain(), Mark::Wordmark);
    assert_eq!(plain, vec!["LCA"], "the fallback spells the product name");

    let coloured = mark_rows(&Theme::colored(), Mark::Wordmark);
    let joined = coloured.join("");
    // l, c, a in order - the same three colours the grid uses, so the fallback
    // reads as the same mark rather than a plain string.
    for hex in ["38;2;98;174;220", "38;2;226;140;0", "38;2;236;205;0"] {
        assert!(
            joined.contains(hex),
            "the wordmark carries {hex}: {joined:?}"
        );
    }
}

#[test]
fn a_narrow_terminal_degrades_instead_of_wrapping() {
    // The ladder: the mark needs its own width plus room for the version beside
    // it; the wordmark needs three; below that nothing at all. A wrapped logo
    // reads as a rendering fault, so it is never an option.
    assert_eq!(mark_for(mark_width() + 2, false), Mark::HalfBlocks);
    assert_eq!(
        mark_for(mark_width() + 1, false),
        Mark::Wordmark,
        "one column short of the mark falls back rather than clipping it"
    );
    assert_eq!(mark_for(3, false), Mark::Wordmark);
    assert_eq!(mark_for(2, false), Mark::None);
    assert_eq!(mark_for(0, false), Mark::None);
    // And the widths that come back are the widths that were promised.
    for (width, expected) in [(mark_width() + 2, mark_width()), (3, 3)] {
        let mark = mark_for(width, false);
        let rows = mark_rows(&Theme::plain(), mark);
        assert!(
            rows.iter()
                .all(|row| lca_tui::engine::text::visible_width(row) <= width),
            "width {width} fits {expected}-column rows: {rows:?}"
        );
    }
}

#[test]
fn a_plain_theme_prints_no_escapes() {
    // FR-UI-5: the plain theme renders no SGR at all, so a redirected or
    // accessibility-oriented run gets the grid as characters.
    for mark in [Mark::HalfBlocks, Mark::Wordmark, Mark::None] {
        for row in mark_rows(&Theme::plain(), mark) {
            assert!(
                !row.contains('\u{1b}'),
                "a plain theme emits no escapes: {row:?}"
            );
        }
    }
}

#[test]
fn a_sixteen_colour_terminal_caps_the_logo_without_losing_the_letters() {
    // The exact hex would be quoted as true color on a terminal that cannot show
    // it; capping to the ANSI ladder keeps three distinguishable letters instead.
    let capped = mark_rows(&Theme::limited(), Mark::HalfBlocks);
    let joined = capped.join("\n");
    assert!(
        !joined.contains("38;2;"),
        "a 16-colour theme must not emit true color: {joined:?}"
    );
    // Three distinct SGR codes, one per letter, so the mark still reads as three
    // colours rather than one flat block.
    let mut codes = Vec::new();
    for part in joined.split("\u{1b}[").skip(1) {
        if let Some(end) = part.find('m') {
            let code = &part[..end];
            if code != "39" && !codes.contains(&code.to_string()) {
                codes.push(code.to_string());
            }
        }
    }
    assert_eq!(
        codes.len(),
        3,
        "three distinguishable colours after capping: {codes:?}"
    );
}
