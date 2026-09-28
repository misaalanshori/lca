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
}
