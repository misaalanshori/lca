//! Released 0.5.0 defect: a malformed OSC 11 background-color reply could
//! overflow in the hex-channel decoder (`16u64.pow(len) - 1` at 16 digits)
//! and, in a debug build, panic. The reply is untrusted terminal input - any
//! process writing to the tty can send it - so it is a trust-boundary
//! defect, fixed after the 0.5.0 tag and headlined in 0.5.1.
//!
//! The fix caps a channel at 8 hex digits, which is xterm's own width. The
//! parser must never panic and must still read the well-formed widths.

use lca_tui::engine::colors::parse_osc11_background_color;

// Verifies: NFR-24 (a released defect's guard), the 0.5.1 trust-boundary fix.
#[test]
fn an_overlong_osc11_hex_channel_does_not_panic() {
    // 16 hex digits once overflowed the `pow`; 9 exceeds the 8-digit cap.
    assert!(parse_osc11_background_color("\x1b]11;rgb:1111111111111111/00/00\x07").is_none());
    assert!(parse_osc11_background_color("\x1b]11;rgb:ffffffffff/00/00\x07").is_none());
    // xterm's widths (1, 2, 4 digits) still parse.
    assert!(parse_osc11_background_color("\x1b]11;rgb:ff/00/80\x07").is_some());
    assert!(parse_osc11_background_color("\x1b]11;rgb:ffff/0000/8080\x07").is_some());
    // Nothing in the parser's reach panics on a hostile reply.
    let _ = parse_osc11_background_color("\x1b]11;rgb:garbage\x07");
    let _ = parse_osc11_background_color("\x1b]11;\x07");
}
