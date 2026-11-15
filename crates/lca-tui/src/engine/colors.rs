//! Terminal color queries, ported from pi's
//! `packages/tui/src/terminal-colors.ts`
//! (`pi-tui-re/src_re/tui-engine/terminal-colors-native-index.md`).
//!
//! Parses the OSC 11 background-color reply in its three dialects and the
//! DEC color-scheme report (`CSI ? 997 ; n n`), used by the theme system's
//! dark/light autodetection.

/// An 8-bit RGB color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RgbColor {
    /// Red channel.
    pub r: u8,
    /// Green channel.
    pub g: u8,
    /// Blue channel.
    pub b: u8,
}

impl RgbColor {
    /// Whether a background of this color is dark or light, by sRGB
    /// relative luminance (pi's `getThemeForRgbColor`).
    pub fn scheme(self) -> ColorScheme {
        fn channel(value: u8) -> f64 {
            let v = f64::from(value) / 255.0;
            if v <= 0.039_28 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        }
        let luminance =
            0.2126 * channel(self.r) + 0.7152 * channel(self.g) + 0.0722 * channel(self.b);
        if luminance >= 0.5 {
            ColorScheme::Light
        } else {
            ColorScheme::Dark
        }
    }
}

/// The terminal's reported color scheme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorScheme {
    /// Dark background.
    Dark,
    /// Light background.
    Light,
}

fn hex_to_rgb(hex: &str) -> RgbColor {
    let normalized = hex.strip_prefix('#').unwrap_or(hex);
    let parse = |s: &str| u8::from_str_radix(s, 16).unwrap_or(0);
    RgbColor {
        r: parse(&normalized[0..2]),
        g: parse(&normalized[2..4]),
        b: parse(&normalized[4..6]),
    }
}

/// Parse one hexadecimal channel whose width implies its range, scaled to
/// 0-255 (`f` -> 255, `ff` -> 255, `ffff` -> 255). Rejects non-hex.
fn parse_osc_hex_channel(channel: &str) -> Option<u8> {
    // The input is an untrusted terminal reply: the `rgb:` dialect puts no
    // bound on a channel's width, and `16u64.pow(len)` overflows at 16
    // digits. Eight hex digits (32 bits) is already far past any real
    // channel, so anything wider is not a color.
    if channel.is_empty() || channel.len() > 8 || !channel.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let value = u64::from_str_radix(channel, 16).ok()?;
    // `len <= 8` keeps the shift at or below 32, and `value * 255` inside
    // u64, so neither the scale nor the round can overflow.
    let max = (1u64 << (4 * channel.len() as u32)) - 1;
    Some((((value * 255) + max / 2) / max) as u8)
}

/// The ASCII body of an OSC 11 reply (`ESC ] 11 ; body (BEL | ST)`).
fn osc11_body(data: &str) -> Option<&str> {
    let rest = data.strip_prefix("\x1b]11;")?;
    let body = rest
        .strip_suffix('\x07')
        .or_else(|| rest.strip_suffix("\x1b\\"))?;
    Some(body)
}

/// Whether the input is an OSC 11 background-color response.
pub fn is_osc11_background_color_response(data: &str) -> bool {
    osc11_body(data).is_some()
}

/// Parse an OSC 11 background-color reply into RGB.
pub fn parse_osc11_background_color(data: &str) -> Option<RgbColor> {
    let value = osc11_body(data)?.trim();
    if let Some(hex) = value.strip_prefix('#') {
        if hex.len() == 6 && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Some(hex_to_rgb(value));
        }
        if hex.len() == 12 && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            let r = parse_osc_hex_channel(&hex[0..4])?;
            let g = parse_osc_hex_channel(&hex[4..8])?;
            let b = parse_osc_hex_channel(&hex[8..12])?;
            return Some(RgbColor { r, g, b });
        }
        return None;
    }
    let rgb_value = value
        .strip_prefix("rgb:")
        .or_else(|| value.strip_prefix("rgba:"))
        .unwrap_or(value);
    let mut parts = rgb_value.split('/');
    let r = parse_osc_hex_channel(parts.next()?)?;
    let g = parse_osc_hex_channel(parts.next()?)?;
    let b = parse_osc_hex_channel(parts.next()?)?;
    if parts.next().is_some() {
        return None;
    }
    Some(RgbColor { r, g, b })
}

/// How much colour a terminal can actually take (gh #600).
///
/// A **rung ladder**, not a boolean, because three genuinely different spellings
/// exist: 24-bit SGR (`38;2;r;g;b`), the 256-colour palette (`38;5;n`), and the
/// sixteen ANSI codes (`30..37`/`90..97`). Emitting a rung the terminal lacks is
/// the defect this closes: a `TERM=xterm-256color` session with `COLORTERM` unset
/// used to get 24-bit escapes it cannot render, and the only honest answer was
/// `ui.color = never` - no colour at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorDepth {
    /// 24-bit RGB sequences.
    Truecolor,
    /// The 256-colour palette: indices, `;5;n`.
    Ansi256,
    /// The sixteen ANSI colours.
    Ansi16,
    /// No escapes at all (FR-UI-5, and a pipe).
    None,
}

impl ColorDepth {
    /// Parse a [`FORCE_COLOR_DEPTH`] value: `truecolor`/`24bit`, `256`, `16`,
    /// `none`/`off`/`0`. Case-insensitive, trimmed; anything else is `None`
    /// so the caller can keep the ladder **and say it ignored the value**.
    #[must_use]
    pub fn parse(value: &str) -> Option<ColorDepth> {
        match value.trim().to_ascii_lowercase().as_str() {
            "truecolor" | "24bit" | "24" => Some(ColorDepth::Truecolor),
            "256" | "256color" => Some(ColorDepth::Ansi256),
            "16" | "16color" | "ansi" => Some(ColorDepth::Ansi16),
            "none" | "off" | "0" | "never" => Some(ColorDepth::None),
            _ => None,
        }
    }

    /// Whether any escapes are spent at this rung.
    #[must_use]
    pub fn paints(self) -> bool {
        self != ColorDepth::None
    }
}

/// The forcing variable, `LCA_HYPERLINKS`/`LCA_SHIFT_ENTER`/`LCA_WHEEL_ACCEL`'s
/// shape: a capability that detection can get wrong must be correctable without
/// a release, and a receipt must be drivable on a box whose terminal lies.
pub const FORCE_COLOR_DEPTH: &str = "LCA_COLOR_DEPTH";

/// Which rung this terminal is on, from the **facts** only.
///
/// `never` is `ui.color = never` (FR-UI-5), and it sits **above everything** -
/// including a forced rung - because a user who asked for no colour has said
/// something about their eyes, not about their terminal.
///
/// Then, in order: a forced rung; `COLORTERM` saying `truecolor`/`24bit`; a
/// `TERM` naming `256color`; any `TERM` that is not `dumb` (sixteen colours);
/// otherwise none. **`COLORTERM` never downgrades**: it can only raise, so a
/// `256color` `TERM` that also says `truecolor` gets truecolor - pi's rule
/// (`terminal-image.ts:179`'s `trueColor ? "truecolor" : "256color"`, one rung
/// above the answer it would otherwise have).
#[must_use]
pub fn color_depth(
    term: Option<&str>,
    colorterm: Option<&str>,
    color_mode_never: bool,
) -> ColorDepth {
    color_depth_forcing(term, colorterm, None, color_mode_never)
}

/// [`color_depth`] with the forcing value injected, so the whole ladder - the
/// escape hatch included - runs as a table on every platform.
#[must_use]
pub fn color_depth_forcing(
    term: Option<&str>,
    colorterm: Option<&str>,
    forced: Option<&str>,
    color_mode_never: bool,
) -> ColorDepth {
    // Rule 1, above everything: FR-UI-5's precedence is the user's, not the
    // terminal's and not the escape hatch's.
    if color_mode_never {
        return ColorDepth::None;
    }
    if let Some(forced) = forced {
        // An unparsable value keeps the ladder (rule 2's second half); the
        // caller that owns stderr says so, because silently ignoring a forcing
        // variable is how a "it did not work" report becomes unanswerable.
        if let Some(depth) = ColorDepth::parse(forced) {
            return depth;
        }
    }
    let term = term.unwrap_or("").trim().to_ascii_lowercase();
    let colorterm = colorterm.unwrap_or("").trim().to_ascii_lowercase();
    // Rule 3: COLORTERM raises. `24bit` is the other spelling terminals emit.
    if colorterm == "truecolor" || colorterm == "24bit" {
        return ColorDepth::Truecolor;
    }
    // Rule 4: the 256-colour palette, named by TERM itself.
    if term.contains("256color") {
        return ColorDepth::Ansi256;
    }
    // Rule 5: a terminal that names itself gets sixteen colours. `dumb` is
    // excluded here rather than checked first, because a TERM of `dumb` is
    // exactly the "do not emit escapes" answer.
    if !term.is_empty() && term != "dumb" {
        return ColorDepth::Ansi16;
    }
    // Rule 6: no TERM, a pipe, or `dumb`.
    ColorDepth::None
}

/// [`color_depth`] for this process, over the real environment.
///
/// A forcing value that does not parse is reported **once** on stderr: it is
/// the one case where the ladder silently wins over the user and the user would
/// otherwise never learn why. (The write lands before the alternate screen in
/// the ordinary startup order; a `/theme` rebuild that re-reads it does not
/// re-warn, because the notice is once per process.)
pub fn current_color_depth(color_mode_never: bool) -> ColorDepth {
    let forced = std::env::var(FORCE_COLOR_DEPTH).ok();
    if let Some(value) = forced.as_deref()
        && ColorDepth::parse(value).is_none()
    {
        // Not `unwrap_or`: a poisoned mutex here must not lose the answer, and
        // the set-once guard only exists to keep the line from repeating.
        static WARNED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
        if WARNED.set(()).is_ok() {
            eprintln!(
                "lca: {FORCE_COLOR_DEPTH}={value:?} is not a colour depth \
                 (truecolor|256|16|none); keeping the detected ladder"
            );
        }
    }
    color_depth_forcing(
        std::env::var("TERM").ok().as_deref(),
        std::env::var("COLORTERM").ok().as_deref(),
        forced.as_deref(),
        color_mode_never,
    )
}

/// The startup colour query: the background (OSC 11) and the colour-scheme
/// preference (DEC private `? 996`), followed by **`DA1` (`ESC [ c`)** as a
/// sentinel (gh #379 gap 1).
///
/// Every terminal answers `DA1` whether or not it understands `OSC 11`, and
/// terminals answer **in order**. So `DA1` arriving means "everything before this
/// has been answered or ignored" — which is what lets the theme resolve the
/// moment the query completes instead of waiting out a timeout on a terminal that
/// ignores colours. The ordering is the whole mechanism: put `DA1` first and it
/// proves nothing, because the `OSC 11` answer could still be in flight behind it.
pub const TERMINAL_COLOR_QUERY: &str = "\x1b]11;?\x07\x1b[?996n\x1b[c";

/// The cell-size query, sent **before** [`TERMINAL_COLOR_QUERY`]'s `DA1` sentinel
/// (gh #324 gap 2).
///
/// `CSI 16 t` asks the terminal for its character cell in pixels; the answer is
/// `CSI 6 ; height ; width t`, which [`super::core::parse_capability_reply`] has
/// parsed since before this query existed - and which nothing ever sent, so no
/// terminal ever answered. That is the defect: a parser with no producer.
///
/// **It must precede the sentinel.** Terminals answer in order, so `DA1` arriving
/// means everything before it has been answered or ignored; putting the cell query
/// after `DA1` would make its answer arrive in the same "late grace" window as a
/// stray, which is exactly what that window exists to reject. `terminal_cell_query`
/// composes the two in the right order.
pub const TERMINAL_CELL_QUERY: &str = "\x1b[16t";

/// The full startup query: the cell size first, then the colour query with its
/// `DA1` sentinel last.
///
/// One constant rather than two writes at the call site, because the ordering is
/// load-bearing and a call site that emitted them separately would be free to get
/// it backwards. See [`TERMINAL_CELL_QUERY`].
pub fn startup_query() -> String {
    format!("{TERMINAL_CELL_QUERY}{TERMINAL_COLOR_QUERY}")
}

/// Whether this input is the `DA1` sentinel that closes [`TERMINAL_COLOR_QUERY`].
///
/// The reply is `CSI ? <params> c` with at least one parameter (`ESC [ ? 1 ; 2 c`
/// from an xterm, `ESC [ ? 62 ; 4 ; 6 ; 22 c` from others); `ESC [ ? c` with no
/// parameters is not a DA1 answer.
pub fn is_device_attributes_report(data: &str) -> bool {
    let Some(body) = data
        .strip_prefix("\x1b[?")
        .and_then(|s| s.strip_suffix('c'))
    else {
        return false;
    };
    !body.is_empty()
        && body.bytes().all(|b| b.is_ascii_digit() || b == b';')
        && body.bytes().any(|b| b.is_ascii_digit())
}

/// Whether the input is a **cell-size** reply (`CSI 6 ; h ; w t`), gh #324 gap 2.
///
/// Kept beside [`is_device_attributes_report`] because it has to be recognized by
/// the same dispatcher: a reply the reader does not classify as a reply falls
/// through to the composer and the user's next keystrokes contain `\x1b[6;16;8t`.
/// That is the failure mode a "wire the consumer" fix would have shipped, since
/// `is_startup_query_reply` never included this shape.
pub fn is_cell_size_report(data: &str) -> bool {
    let Some(body) = data
        .strip_prefix("\x1b[6;")
        .and_then(|s| s.strip_suffix('t'))
    else {
        return false;
    };
    let mut parts = body.split(';');
    let (Some(height), Some(width), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    !height.is_empty()
        && !width.is_empty()
        && height.bytes().all(|b| b.is_ascii_digit())
        && width.bytes().all(|b| b.is_ascii_digit())
}

/// Whether the input is a capability reply the startup query can elicit.
///
/// One recogniser for the three replies, so the *late* path and the *startup* path
/// cannot disagree about what counts as a reply - and so a reply-shaped sequence
/// that arrives after startup is still recognised as a reply rather than typed
/// into the composer (the guard, gh #379 gap 1).
pub fn is_startup_query_reply(data: &str) -> bool {
    is_osc11_background_color_response(data)
        || parse_terminal_color_scheme_report(data).is_some()
        || is_device_attributes_report(data)
        || is_cell_size_report(data)
}

/// Parse a DEC color-scheme report (`CSI ? 997 ; 1 n` dark / `; 2 n` light),
/// including a repeated form.
pub fn parse_terminal_color_scheme_report(data: &str) -> Option<ColorScheme> {
    let mut rest = data;
    let mut scheme = None;
    let mut saw_any = false;
    while let Some(after) = rest.strip_prefix("\x1b[?997;") {
        let bytes = after.as_bytes();
        let digit = *bytes.first()?;
        if !(digit == b'1' || digit == b'2') {
            return None;
        }
        if after.as_bytes().get(1) != Some(&b'n') {
            return None;
        }
        saw_any = true;
        scheme.get_or_insert(if digit == b'2' {
            ColorScheme::Light
        } else {
            ColorScheme::Dark
        });
        rest = &after[2..];
    }
    if saw_any && rest.is_empty() {
        scheme
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_three_osc11_dialects() {
        assert_eq!(
            parse_osc11_background_color("\x1b]11;#1e1e2e\x07"),
            Some(RgbColor {
                r: 0x1e,
                g: 0x1e,
                b: 0x2e
            })
        );
        assert_eq!(
            parse_osc11_background_color("\x1b]11;rgb:1e1e/1e1e/2e2e\x1b\\"),
            Some(RgbColor {
                r: 0x1e,
                g: 0x1e,
                b: 0x2e
            })
        );
        assert_eq!(
            parse_osc11_background_color("\x1b]11;rgb:ff/00/80\x07"),
            Some(RgbColor {
                r: 255,
                g: 0,
                b: 0x80
            })
        );
        assert!(is_osc11_background_color_response("\x1b]11;#000000\x07"));
        assert!(parse_osc11_background_color("\x1b]11;notacolor\x07").is_none());
    }

    // An over-wide `rgb:` channel is an untrusted terminal reply; it must be
    // refused, not overflow `16^len` and panic a debug build.
    #[test]
    fn an_over_wide_channel_is_refused_not_overflowed() {
        assert!(parse_osc11_background_color("\x1b]11;rgb:1111111111111111/00/00\x07").is_none());
        assert!(parse_osc11_background_color("\x1b]11;rgb:ffffffff/00/00\x07").is_some());
        assert!(parse_osc11_background_color("\x1b]11;rgb:000000000/00/00\x07").is_none());
    }

    #[test]
    fn parses_the_scheme_report() {
        assert_eq!(
            parse_terminal_color_scheme_report("\x1b[?997;1n"),
            Some(ColorScheme::Dark)
        );
        assert_eq!(
            parse_terminal_color_scheme_report("\x1b[?997;2n\x1b[?997;2n"),
            Some(ColorScheme::Light)
        );
        assert_eq!(parse_terminal_color_scheme_report("\x1b[?997;3n"), None);
    }

    // Verifies: R10 - background luminance picks the scheme.
    #[test]
    fn a_light_background_is_light_and_a_dark_one_dark() {
        assert_eq!(
            RgbColor {
                r: 255,
                g: 255,
                b: 255
            }
            .scheme(),
            ColorScheme::Light
        );
        assert_eq!(RgbColor { r: 0, g: 0, b: 0 }.scheme(), ColorScheme::Dark);
    }
    // Verifies: gh #379 gap 1 - the query ends with the DA1 sentinel, and the
    // sentinel is **last**. Ordering is the entire mechanism: a DA1 written first
    // would be answered before the colour replies could arrive, so its arrival
    // would prove nothing about them.
    #[test]
    fn the_query_asks_for_colours_then_closes_with_da1() {
        let query = TERMINAL_COLOR_QUERY;
        let da1 = query.find("\x1b[c").expect("the DA1 sentinel is present");
        let bg = query
            .find("\x1b]11;?")
            .expect("the OSC 11 query is present");
        let scheme = query
            .find("\x1b[?996n")
            .expect("the scheme query is present");
        assert!(bg < da1, "OSC 11 is asked before the sentinel: {query:?}");
        assert!(
            scheme < da1,
            "the colour-scheme query is asked before the sentinel: {query:?}"
        );
        assert_eq!(
            da1 + "\x1b[c".len(),
            query.len(),
            "the sentinel is the last thing written: {query:?}"
        );
    }

    // Verifies: gh #379 gap 1 - a DA1 reply is recognised in the shapes terminals
    // actually send, and a DA1-*shaped* thing that is not an answer (no
    // parameters) is not.
    #[test]
    fn a_device_attributes_report_is_recognised() {
        assert!(is_device_attributes_report("\x1b[?1;2c"));
        assert!(is_device_attributes_report("\x1b[?62;4;6;22c"));
        assert!(is_device_attributes_report("\x1b[?64c"));
        assert!(!is_device_attributes_report("\x1b[?c"), "no parameters");
        assert!(!is_device_attributes_report("\x1b[c"), "bare DA1 request");
        assert!(
            !is_device_attributes_report("\x1b[1;2c"),
            "not a private DA1"
        );
        assert!(!is_device_attributes_report("\x1b[?1;2n"), "a DSR report");
        assert!(!is_device_attributes_report("x"), "not an escape at all");
    }

    // Verifies: gh #379 gap 1 - **one** recogniser decides what a startup reply
    // is. The late-reply path and the composer guard both go through it, so a
    // reply-shape one of them accepts cannot be typed by the other.
    #[test]
    fn one_recogniser_covers_every_startup_reply() {
        for reply in [
            "\x1b]11;#1e1e2e\x07",
            "\x1b]11;rgb:1e1e/1e1e/2e2e\x1b\\",
            "\x1b[?997;1n",
            "\x1b[?997;2n\x1b[?997;2n",
            "\x1b[?1;2c",
        ] {
            assert!(
                is_startup_query_reply(reply),
                "{reply:?} is a startup reply"
            );
        }
        // Things that are neither a reply nor a sentinel must still be input.
        for typed in [
            "a",
            "\x1b[A",
            "\x1b[?1;2n",
            "\x1b]7501;state=idle:app=lca\x1b\\",
        ] {
            assert!(
                !is_startup_query_reply(typed),
                "{typed:?} must reach the interface as input"
            );
        }
    }
}

#[cfg(test)]
mod color_depth_tests {
    use super::*;

    // Verifies: gh #600 - **the ladder, as a table**, with every input a
    // parameter so this row runs on every platform. The defect it closes: a
    // `TERM=xterm-256color` session with `COLORTERM` unset used to get 24-bit
    // SGR, which such a terminal cannot render, and the only honest answer a
    // user had was `ui.color = never` - no colour at all.
    #[test]
    fn the_detection_ladder_reads_term_and_colorterm() {
        let cases: [(&str, Option<&str>, Option<&str>, ColorDepth); 12] = [
            // Rule 3: COLORTERM raises, in both of the spellings terminals use.
            (
                "xterm-256color",
                Some("truecolor"),
                None,
                ColorDepth::Truecolor,
            ),
            ("xterm-256color", Some("24bit"), None, ColorDepth::Truecolor),
            (
                "xterm-256color",
                Some(" TRUECOLOR "),
                None,
                ColorDepth::Truecolor,
            ),
            // Rule 4: TERM names the palette.
            ("xterm-256color", None, None, ColorDepth::Ansi256),
            ("screen-256color", Some("false"), None, ColorDepth::Ansi256),
            // Rule 5: any real terminal gets sixteen.
            ("xterm", None, None, ColorDepth::Ansi16),
            ("vt100", None, None, ColorDepth::Ansi16),
            // Rule 6: dumb, empty, and no TERM at all.
            ("dumb", None, None, ColorDepth::None),
            ("", Some("truecolor"), None, ColorDepth::Truecolor),
            // Rule 2: a forced rung wins over the ladder...
            (
                "xterm-256color",
                None,
                Some("truecolor"),
                ColorDepth::Truecolor,
            ),
            (
                "xterm-256color",
                Some("truecolor"),
                Some("16"),
                ColorDepth::Ansi16,
            ),
            // ...and an unparsable forcing keeps the ladder.
            (
                "xterm-256color",
                None,
                Some("chartreuse"),
                ColorDepth::Ansi256,
            ),
        ];
        for (term, colorterm, forced, expected) in cases {
            assert_eq!(
                color_depth_forcing(Some(term), colorterm, forced, false),
                expected,
                "term={term:?} colorterm={colorterm:?} forced={forced:?}"
            );
        }
        // Rule 6's last form: no TERM variable at all.
        assert_eq!(color_depth(None, None, false), ColorDepth::None);
    }

    // Verifies: gh #600 - **`COLORTERM` never downgrades.** The issue's own note:
    // it can only raise. A terminal that says `256color` and also says
    // `COLORTERM=truecolor` has truecolor, and one that says `COLORTERM` anything
    // else keeps whatever `TERM` implies rather than losing a rung.
    #[test]
    fn colorterm_raises_and_never_downgrades() {
        assert_eq!(
            color_depth(Some("xterm-256color"), Some("24bit"), false),
            ColorDepth::Truecolor
        );
        // The specific regression a "COLORTERM decides everything" rule would
        // cause: a 256color TERM plus a COLORTERM value that is neither
        // spelling must still be 256, not 16 and not none.
        assert_eq!(
            color_depth(Some("xterm-256color"), Some("yes"), false),
            ColorDepth::Ansi256
        );
        assert_eq!(
            color_depth(Some("xterm"), Some("yes"), false),
            ColorDepth::Ansi16
        );
    }

    // Verifies: gh #600 + FR-UI-5 - **`ui.color = never` sits above everything**,
    // including the forcing variable and a truecolor `COLORTERM`: a user who asked
    // for no colour has said something about their eyes, not about their terminal.
    #[test]
    fn ui_color_never_beats_every_depth() {
        for (term, colorterm, forced) in [
            ("xterm-256color", None, None),
            ("xterm-256color", Some("truecolor"), Some("truecolor")),
            ("xterm-kitty", Some("24bit"), Some("256")),
            ("dumb", None, None),
        ] {
            assert_eq!(
                color_depth_forcing(Some(term), colorterm, forced, true),
                ColorDepth::None,
                "never must win: term={term:?} colorterm={colorterm:?} forced={forced:?}"
            );
        }
    }

    // Verifies: gh #600 - the forcing variable's own spellings, so a receipt can
    // drive any rung and a misdetecting terminal can be corrected without a
    // release (`LCA_HYPERLINKS`'s shape).
    #[test]
    fn the_forcing_variable_names_every_rung() {
        for (value, expected) in [
            ("truecolor", ColorDepth::Truecolor),
            ("24bit", ColorDepth::Truecolor),
            ("256", ColorDepth::Ansi256),
            ("256color", ColorDepth::Ansi256),
            ("16", ColorDepth::Ansi16),
            ("ansi", ColorDepth::Ansi16),
            ("none", ColorDepth::None),
            ("never", ColorDepth::None),
            ("0", ColorDepth::None),
            (" TRUECOLOR ", ColorDepth::Truecolor),
        ] {
            assert_eq!(ColorDepth::parse(value), Some(expected), "value={value:?}");
        }
        for junk in ["", "chartreuse", "255", "true"] {
            assert_eq!(ColorDepth::parse(junk), None, "junk={junk:?}");
        }
    }

    // Verifies: gh #600 - `paints()` is the one answer to "does this rung spend
    // escapes", which is what a theme's `colored` flag derives from.
    #[test]
    fn paints_is_false_only_for_none() {
        assert!(ColorDepth::Truecolor.paints());
        assert!(ColorDepth::Ansi256.paints());
        assert!(ColorDepth::Ansi16.paints());
        assert!(!ColorDepth::None.paints());
    }
}

#[cfg(test)]
mod cell_query_tests {
    use super::*;

    // Verifies: gh #324 gap 2 - **the cell query is sent, and before the sentinel.**
    // The defect this closes was a parser with no producer: `CellSize` had been
    // parsed since `core.rs` grew it, and no terminal was ever asked, so no answer
    // could arrive. The ordering matters for the same reason the colour query's
    // does - terminals answer in order, and `DA1` is what proves the replies before
    // it are in.
    #[test]
    fn the_startup_query_asks_for_the_cell_size_before_the_sentinel() {
        let query = startup_query();
        let cell = query
            .find(TERMINAL_CELL_QUERY)
            .unwrap_or_else(|| panic!("the cell query is in the startup write: {query:?}"));
        let sentinel = query
            .find("\x1b[c")
            .unwrap_or_else(|| panic!("the sentinel is there: {query:?}"));
        assert!(
            cell < sentinel,
            "the cell query precedes the DA1 sentinel: {query:?}"
        );
        // The sentinel is still the very last byte, so the window still closes on
        // it (gh #379 gap 1's property, unchanged).
        assert!(query.ends_with("\x1b[c"), "the sentinel is last: {query:?}");
        // And the colour query is unchanged inside it.
        assert!(
            query.contains(TERMINAL_COLOR_QUERY),
            "the colour query is carried whole: {query:?}"
        );
    }

    // Verifies: gh #324 gap 2 - the reply is recognized as a reply. Without this the
    // query would be sent and its answer would fall through to the composer, so the
    // user's next keystrokes would contain `\x1b[6;16;8t`.
    #[test]
    fn a_cell_size_report_is_recognized_as_a_startup_reply() {
        assert!(is_cell_size_report("\x1b[6;16;8t"));
        assert!(is_cell_size_report("\x1b[6;18;9t"));
        assert!(is_startup_query_reply("\x1b[6;16;8t"));
        // Not a reply: other `CSI … t` reports, a missing field, a negative
        // (sign-prefixed) number, trailing junk.
        assert!(!is_cell_size_report("\x1b[6;16t"));
        assert!(!is_cell_size_report("\x1b[6;16;8;1t"));
        assert!(!is_cell_size_report("\x1b[7;16;8t"));
        assert!(!is_cell_size_report("\x1b[6;-1;8t"));
        assert!(!is_cell_size_report("\x1b[6;;8t"));
        assert!(!is_cell_size_report("\x1b[16t"));
        assert!(!is_cell_size_report(""));
    }
}
