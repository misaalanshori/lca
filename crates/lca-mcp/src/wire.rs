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
        match classify(message, id) {
            Classified::Notification => continue,
            Classified::Foreign(other) => {
                // In a batch, a foreign id is just another conversation on
                // the same envelope; the single-outstanding-request stdio
                // path is where it means desync (see
                // [`response_for`]).
                let _ = other;
                continue;
            }
            Classified::Answer(Reply::Result(result)) => return Ok(result),
            Classified::Answer(Reply::Error(error)) => return Err(error),
        }
    }
    // gh #336: a JSON-RPC error carrying `"id": null` is the server's
    // answer to *the* request it could not associate with an id -
    // exactly this one, since the envelope holds only our round. Without
    // this scan the server's own explanation ("Parse error", "Internal
    // error", overloaded) was dropped and the call read as a vanished
    // response, which is the least actionable thing to report.
    if let Some(error) = messages
        .iter()
        .find(|message| message.get("id").is_some_and(serde_json::Value::is_null))
        .and_then(|message| message.get("error"))
    {
        return Err(format!("MCP server errored: {error}"));
    }
    Err(format!(
        "MCP server answered without our response (id {id})"
    ))
}

/// Whether a JSON-RPC message is a server notification (an ordinary
/// `method`-bearing message with no `id`), which every MCP server is
/// free to interleave with replies.
pub fn is_notification(message: &serde_json::Value) -> bool {
    message.get("method").is_some() && message.get("id").is_none()
}

/// How one received message relates to the request we are waiting on.
enum Classified {
    /// `method`, no `id`: server chatter, discard and keep reading.
    Notification,
    /// An `id` that is not ours (or no usable `id` at all).
    Foreign(Option<u64>),
    /// Our reply.
    Answer(Reply),
}

/// A reply for our id: either the result, or the JSON-RPC error the
/// server answered with.
///
/// The two are distinct on purpose (gh #291). A JSON-RPC error for our
/// id is a *legitimate answer* — `resources/templates/list` being
/// unimplemented arrives this way and means "no templates", not a
/// failure. A transport problem (bad JSON, EOF, a desynced id) is a
/// `Err` and must never be read as an empty success. Collapsing them
/// would let a desynchronized stream look like an empty listing.
#[derive(Debug, Clone, PartialEq)]
pub enum Reply {
    /// The `result` member.
    Result(serde_json::Value),
    /// The `error` member, already formatted the way the host reports it.
    Error(String),
}

fn classify(message: &serde_json::Value, id: u64) -> Classified {
    if is_notification(message) {
        return Classified::Notification;
    }
    let found = message.get("id").and_then(|found| found.as_u64());
    if found != Some(id) {
        return Classified::Foreign(found);
    }
    if let Some(error) = message.get("error") {
        return Classified::Answer(Reply::Error(format!("MCP server errored: {error}")));
    }
    match message.get("result").cloned() {
        Some(result) => Classified::Answer(Reply::Result(result)),
        // Our id with neither member is not a legitimately empty answer.
        None => Classified::Answer(Reply::Error(format!(
            "MCP server answered without a result: {message}"
        ))),
    }
}

/// Read exactly one reply for `id` from a line-at-a-time source
/// (gh #291).
///
/// A single-outstanding-request transport cannot legitimately receive a
/// foreign id, so a mismatch is desync already in progress: it fails
/// loud and names the id rather than silently consuming a reply that
/// belongs to some other call. Notifications are ordinary MCP traffic
/// and are discarded.
///
/// `next_line` yields the next decoded line, or `Ok(None)` at EOF; the
/// transport (a pipe, a socket, a test script) owns the I/O and the
/// error text, which is why this lives here as semantics over values and
/// the extension keeps the reading loop's plumbing.
pub fn response_for(
    id: u64,
    mut next_line: impl FnMut() -> Result<Option<String>, String>,
) -> Result<Reply, String> {
    loop {
        let Some(line) = next_line()? else {
            // EOF without our reply: the reader's existing behaviour for
            // a closed stdout, named the same way every transport names it.
            return Err(EOF_WITHOUT_REPLY.to_string());
        };
        let message: serde_json::Value = serde_json::from_str(&line)
            .map_err(|err| format!("MCP server answered bad JSON: {err}"))?;
        match classify(&message, id) {
            Classified::Notification => continue,
            Classified::Foreign(found) => {
                return Err(match found {
                    Some(other) => format!(
                        "MCP server answered another call (wanted id {id}, got id {other}); \
                         the stream is desynchronized"
                    ),
                    None => format!(
                        "MCP server answered without an id (wanted id {id}); \
                         the stream is desynchronized"
                    ),
                });
            }
            Classified::Answer(answer) => return Ok(answer),
        }
    }
}

/// The EOF message [`response_for`] returns when a server closes its
/// stdout before answering. Shared so transports name it identically.
pub const EOF_WITHOUT_REPLY: &str = "MCP server closed its stdout";

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

    // Verifies: gh #418 Phase C#C3 (gh #336) - a server that answers a
    // JSON-RPC error with `"id": null` (the shape a transport-level
    // failure takes: parse errors, overloaded, internal) surfaces that
    // error, instead of reading as "no response at all". The null-id
    // error is round-scoped, so it may only answer the request the
    // envelope was collected for - a later request must not inherit it.
    #[test]
    fn a_null_id_error_answers_instead_of_missing_the_response() {
        let envelope = parse_sse_messages(
            "data: {\"jsonrpc\":\"2.0\",\"id\":null,\"error\":{\"code\":-32700,\"message\":\"Parse error\"}}\n\n",
        );
        let err = select_response(&envelope, 4).expect_err("the error is the answer");
        assert!(err.contains("MCP server errored"), "{err}");
        assert!(err.contains("Parse error"), "{err}");
        assert!(err.contains("-32700"), "{err}");

        // Our own id still wins over a null-id error in the same envelope.
        let mixed = parse_sse_messages(
            "data: {\"id\":null,\"error\":{\"code\":-32603,\"message\":\"Internal error\"}}\n\ndata: {\"id\":5,\"result\":{\"ok\":true}}\n\n",
        );
        assert_eq!(
            select_response(&mixed, 5).expect("our result"),
            serde_json::json!({"ok": true})
        );

        // A null-id *notification-shaped* message is not an answer.
        let notification = parse_sse_messages(
            "data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/message\"}\n\n",
        );
        assert!(select_response(&notification, 6).is_err());

        // A foreign numeric id is still a desync, not a null-id answer.
        let foreign = parse_sse_messages("data: {\"id\":99,\"result\":{}}\n\n");
        assert!(select_response(&foreign, 6).is_err());
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
