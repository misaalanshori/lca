//! The MCP wire-protocol kit (gh #53 phase 2): the streamable-HTTP
//! envelope and the JSON-RPC response selector. Pure functions over
//! plain values - no transport, no I/O - so the stdio session, the
//! HTTP session, and any later transport decode identically.

/// Split one SSE body into its JSON messages: `data:` lines form one
/// message per blank-separated event, `:` comments never do.
pub fn parse_sse_messages(body: &str) -> Vec<serde_json::Value> {
    let mut messages = Vec::new();
    let mut data = String::new();
    for line in body.lines() {
        if line.is_empty() {
            if !data.is_empty()
                && let Ok(message) = serde_json::from_str(&data)
            {
                messages.push(message);
            }
            data.clear();
            continue;
        }
        if line.starts_with(':') {
            continue;
        }
        if let Some(payload) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(payload.strip_prefix(' ').unwrap_or(payload));
        }
    }
    if !data.is_empty()
        && let Ok(message) = serde_json::from_str(&data)
    {
        messages.push(message);
    }
    messages
}

/// Pick our response out of one envelope: the message carrying our
/// id. A JSON-RPC error for our id is the server's answer, not
/// transport noise: it returns as an error.
pub fn select_response(
    messages: &[serde_json::Value],
    id: u64,
) -> Result<serde_json::Value, String> {
    for message in messages {
        let matches = message.get("id").and_then(|found| found.as_u64()) == Some(id);
        if !matches {
            continue;
        }
        if let Some(error) = message.get("error") {
            return Err(format!("MCP server errored: {error}"));
        }
        return message
            .get("result")
            .cloned()
            .ok_or_else(|| format!("MCP server answered without a result: {message}"));
    }
    Err(format!(
        "MCP server answered without our response (id {id})"
    ))
}

/// Decode standard base64 (MCP `blob` content). Whitespace refused;
/// padding required in the last quantum, like every strict decoder.
pub fn base64_decode(text: &str) -> Result<Vec<u8>, String> {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut values = Vec::with_capacity(text.len());
    for byte in text.bytes() {
        if byte == b'=' {
            values.push(64);
            continue;
        }
        let Some(value) = ALPHABET.iter().position(|digit| *digit == byte) else {
            return Err("bad base64 character".to_string());
        };
        values.push(value);
    }
    if values.len() % 4 != 0 {
        return Err("bad base64 length".to_string());
    }
    let mut out = Vec::with_capacity(values.len() / 4 * 3);
    for quantum in values.chunks(4) {
        let pad = quantum
            .iter()
            .rev()
            .take_while(|digit| **digit == 64)
            .count();
        if pad > 2 {
            return Err("bad base64 padding".to_string());
        }
        let digits: Vec<usize> = quantum
            .iter()
            .map(|digit| if *digit == 64 { 0 } else { *digit })
            .collect();
        let triple = (digits[0] << 18) | (digits[1] << 12) | (digits[2] << 6) | digits[3];
        out.push((triple >> 16) as u8);
        if pad < 2 {
            out.push((triple >> 8) as u8);
        }
        if pad < 1 {
            out.push(triple as u8);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Verifies: gh #53 - the envelope shapes (comments skipped,
    // multi-line data joined, id selection, error answers).
    #[test]
    fn envelope_shapes_hold() {
        let messages =
            parse_sse_messages(":keep-alive\n\nevent: message\ndata: {\"id\":7,\"result\":{}}\n\n");
        assert_eq!(messages.len(), 1);
        assert_eq!(
            select_response(&messages, 7).expect("our id selects"),
            serde_json::json!({})
        );
        assert!(select_response(&messages, 8).is_err());
        let errors = parse_sse_messages("data: {\"id\":9,\"error\":{\"code\":1}}\n\n");
        assert!(select_response(&errors, 9).is_err());
    }

    // Verifies: gh #53 - the blob decoder round-trips (padding
    // included) and refuses alphabet and length violations.
    #[test]
    fn base64_vectors_hold() {
        assert_eq!(base64_decode("aGk=").expect("hi"), b"hi");
        assert_eq!(base64_decode("aGk6").expect("hi:"), b"hi:");
        assert_eq!(base64_decode("").expect("empty"), b"");
        assert!(base64_decode("aGk").is_err());
        assert!(base64_decode("aGk*").is_err());
        assert!(base64_decode("====").is_err());
    }
}
