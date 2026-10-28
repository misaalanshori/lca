//! The OSC 52 copy and its honest outcome (gh #374 gap 4, R6).
//!
//! A separate module for the 1,200-line ceiling, and because the outcome type is
//! the thing the interface reads: `Unverified` is the honest answer a bare OSC 52
//! write can give, and the whole point of gap 4 is that the *interface* now says
//! so instead of showing a success.
//!
//! The engine still owns no clipboard: it writes the escape and reports that it
//! cannot verify it. The host's ladder (arboard first) is what can earn a
//! [`CopyOutcome::Verified`].

use super::AltScreenRenderer;
use crate::engine::terminal::Terminal;

/// How a copy completed, for honest reporting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyOutcome {
    /// A verified native clipboard accepted the text.
    Verified,
    /// Only an unverifiable OSC 52 write was possible.
    Unverified,
}

impl AltScreenRenderer {
    /// Render an OSC 52 copy. Returns whether the write was verifiable
    /// (never, without an injected native clipboard — the honest answer).
    pub fn copy_osc52(&self, term: &mut dyn Terminal, text: &str) -> CopyOutcome {
        let encoded = base64_encode(text.as_bytes());
        term.write(&format!("\x1b]52;c;{encoded}\x07"));
        CopyOutcome::Unverified
    }
}

// The engine has no base64 dependency; a tiny local encoder keeps OSC 52
// self-contained.
#[allow(clippy::needless_range_loop)]
fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[((n >> 6) & 63) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[(n & 63) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_encodes_osc52_payloads() {
        assert_eq!(base64_encode(b"hello"), "aGVsbG8=");
        assert_eq!(base64_encode(b"a"), "YQ==");
        assert_eq!(base64_encode(b""), "");
    }
}
