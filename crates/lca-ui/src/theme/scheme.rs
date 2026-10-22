//! Terminal colour-scheme detection (gh #10, gh #387 gap 2).
//!
//! Split from `mod.rs` under the 1,200-line ceiling. The OSC 11 / DEC report
//! ladder is the engine's, consumed at negotiation; this is the `COLORFGBG`
//! fallback for terminals that set that variable and answer nothing.

/// Detect the terminal's color scheme from `COLORFGBG` (the OSC 11 / DEC
/// report ladder is the engine's, consumed at negotiation).
pub fn detect_scheme() -> Option<lca_tui::engine::colors::ColorScheme> {
    scheme_from_colorfgbg(&std::env::var("COLORFGBG").ok()?)
}

/// `COLORFGBG` → scheme, the testable half (gh #387 gap 2).
///
/// `COLORFGBG` is `fg;bg`, or `fg;xpm;bg` in rxvt: the background is the last
/// field, and it is an ANSI color index or `default` when the color is not in
/// the palette. The index names the terminal's own palette, whose colours are
/// unknowable here, so it is classified **by index**, the way Vim and pi do:
///
/// - `0..=6` and `8` are **dark**. Index 8 is "bright black" — the background
///   of Solarized Dark (`COLORFGBG=12;8`), and the index the old
///   `background >= 8` rule got exactly backwards.
/// - `7` and `9..=15` are **light**. Index 7 is white/light-gray, which the old
///   rule also inverted.
///
/// Anything else is `None`: a non-numeric field, an index above 15, a missing
/// value. A wrong scheme silently repaints every panel, so guessing is worse
/// than falling back to the built-in default.
#[must_use]
pub fn scheme_from_colorfgbg(value: &str) -> Option<lca_tui::engine::colors::ColorScheme> {
    use lca_tui::engine::colors::ColorScheme;
    let field = value.rsplit(';').next()?.trim();
    // pi's `^\d{1,2}$`: a literal `default` (which some terminals write for
    // "not in the palette") is rejected rather than read as 0.
    if field.is_empty() || field.len() > 2 || !field.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let index: u32 = field.parse().ok()?;
    if index > 15 {
        return None;
    }
    Some(if index <= 6 || index == 8 {
        ColorScheme::Dark
    } else {
        ColorScheme::Light
    })
}

#[cfg(test)]
mod scheme_detection_tests {
    use super::scheme_from_colorfgbg;
    use lca_tui::engine::colors::ColorScheme;

    // Verifies: gh #387 gap 2 - **the inverted range, as a table.** The old
    // rule was `background >= 8 -> Light`, which classified ANSI 7
    // (white/light-gray) as Dark and ANSI 8 (bright black, Solarized Dark's own
    // background) as Light: the exact opposite of reality for both indices.
    //
    // pi's rule, which is what a shell user's terminal actually means:
    // `0..=6` and `8` are dark; `7` and `9..=15` are light.
    #[test]
    fn every_ansi_background_index_classifies_like_pi() {
        for index in 0..=6 {
            assert_eq!(
                scheme_from_colorfgbg(&format!("15;{index}")),
                Some(ColorScheme::Dark),
                "index {index} is a dark background"
            );
        }
        assert_eq!(
            scheme_from_colorfgbg("15;7"),
            Some(ColorScheme::Light),
            "index 7 is white/light-gray: the old rule called it Dark"
        );
        assert_eq!(
            scheme_from_colorfgbg("15;8"),
            Some(ColorScheme::Dark),
            "index 8 is bright black - Solarized Dark's background: the old \
             rule called it Light"
        );
        for index in 9..=15 {
            assert_eq!(
                scheme_from_colorfgbg(&format!("0;{index}")),
                Some(ColorScheme::Light),
                "index {index} is a light background"
            );
        }
    }

    // Verifies: gh #387 gap 2 - the two real-world reports the bug broke,
    // spelled as the terminals actually set them.
    #[test]
    fn solarized_dark_and_xterm_light_are_recognised() {
        assert_eq!(
            scheme_from_colorfgbg("12;8"),
            Some(ColorScheme::Dark),
            "Solarized Dark: COLORFGBG=12;8"
        );
        assert_eq!(
            scheme_from_colorfgbg("10;8"),
            Some(ColorScheme::Dark),
            "Solarized Dark, the other common spelling"
        );
        assert_eq!(
            scheme_from_colorfgbg("0;7"),
            Some(ColorScheme::Light),
            "xterm's light default: white background"
        );
        assert_eq!(
            scheme_from_colorfgbg("15;0"),
            Some(ColorScheme::Dark),
            "the common dark default"
        );
    }

    // Verifies: gh #387 gap 2 - both shapes `COLORFGBG` takes. `fg;bg` is the
    // usual one; rxvt writes `fg;xpm;bg`, where the background is still the
    // last field.
    #[test]
    fn both_colorfgbg_shapes_read_the_last_field() {
        assert_eq!(
            scheme_from_colorfgbg("15;8"),
            Some(ColorScheme::Dark),
            "fg;bg"
        );
        assert_eq!(
            scheme_from_colorfgbg("15;default;8"),
            Some(ColorScheme::Dark),
            "fg;xpm;bg - the background is still last"
        );
        assert_eq!(
            scheme_from_colorfgbg("0;default;7"),
            Some(ColorScheme::Light),
            "and the xpm field does not shift it"
        );
    }

    // Verifies: gh #387 gap 2 - **garbage is `None`, not a guess.** pi rejects
    // an out-of-range index; a wrong scheme is worse than the built-in default
    // because it silently repaints every panel.
    #[test]
    fn malformed_reports_are_none_not_a_wrong_guess() {
        for garbage in [
            "", " ", ";", "abc", "15;", "15;16", "15;99", "15;-1", "15;7.5",
        ] {
            assert_eq!(
                scheme_from_colorfgbg(garbage),
                None,
                "`{garbage}` is not a usable report"
            );
        }
        // pi's regex is `^\d{1,2}$`, so the literal `default` (which some
        // terminals do write for "not in the palette") is rejected rather than
        // read as 0 - guessing there would be the same class of bug as the
        // inverted range.
        assert_eq!(
            scheme_from_colorfgbg("15;default"),
            None,
            "a non-numeric background is not a background index"
        );
    }
}
