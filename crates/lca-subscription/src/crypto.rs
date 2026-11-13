//! PKCE, base64url, and the randomness behind them (gh #567: split out of
//! `lib.rs` by OAuth concern).

use crate::IdentityFailure;

/// One PKCE S256 pair (gh #183, gh #185): the verifier crosses on the
/// code exchange, the challenge rides the authorize URL. The same
/// CSPRNG and base64url the kit's own flow uses.
pub fn pkce_pair() -> Result<(String, String), IdentityFailure> {
    let verifier = base64url(&random_bytes(32)?);
    let challenge = {
        use sha2::Digest as _;
        base64url(&sha2::Sha256::digest(verifier.as_bytes()))
    };
    Ok((verifier, challenge))
}

/// The URL-encoder for one authorize query value (gh #183, gh #185):
/// the form field encoder's shape, shared so the extensions build
/// URLs exactly like the kit's own flow does.
pub fn percent_encode(text: &str) -> String {
    percent(text)
}

/// Percent-encode a form field (uppercase hex, like every OAuth server
/// and `URLSearchParams` produce).
pub(crate) fn percent(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

pub(crate) fn base64url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity((bytes.len() * 4).div_ceil(3));
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(ALPHABET[((n >> 18) & 63) as usize] as char);
        out.push(ALPHABET[((n >> 12) & 63) as usize] as char);
        if chunk.len() > 1 {
            out.push(ALPHABET[((n >> 6) & 63) as usize] as char);
        }
        if chunk.len() > 2 {
            out.push(ALPHABET[(n & 63) as usize] as char);
        }
    }
    out
}

pub(crate) fn base64url_decode(text: &str) -> Option<Vec<u8>> {
    let unpadded = text.trim_end_matches('=');
    let mut bits = 0u32;
    let mut count = 0;
    let mut out = Vec::new();
    for byte in unpadded.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            _ => return None,
        } as u32;
        bits = (bits << 6) | value;
        count += 6;
        if count >= 8 {
            count -= 8;
            out.push((bits >> count) as u8);
            bits &= (1 << count) - 1;
        }
    }
    Some(out)
}

pub(crate) fn random_bytes(len: usize) -> Result<Vec<u8>, IdentityFailure> {
    let mut bytes = vec![0u8; len];
    getrandom::fill(&mut bytes)
        .map_err(|err| IdentityFailure(format!("the platform CSPRNG failed: {err}")))?;
    Ok(bytes)
}
