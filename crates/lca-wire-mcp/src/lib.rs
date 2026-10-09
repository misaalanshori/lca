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
}
