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
    // gh #336: remembered rather than returned, so a reply we can attribute wins
    // over an error we cannot.
    let mut unattributed: Option<String> = None;
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
            // A null-id error is a *fallback* answer, not a competitor: if our
            // own reply is in the same envelope, that reply is the answer and the
            // unattributable error is somebody else's noise. Scanning for it
            // after the loop (what the first #336 fix did) is what keeps this
            // precedence; taking it inline would let a server's parse-error
            // chatter shadow the result we actually asked for.
            Classified::UnattributedError(error) => unattributed = Some(error),
            Classified::Answer(Reply::Result(result)) => return Ok(result),
            Classified::Answer(Reply::Error(error)) => return Err(error),
        }
    }
    // gh #336: a null-id error *is* the answer when nothing attributable is in
    // the envelope. It is the server telling us why it refused, and "answered
    // without our response" would be a guess where the server had spoken.
    if let Some(error) = unattributed {
        return Err(error);
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
    /// A JSON-RPC error carrying `"id": null`: the server's answer to a request
    /// it could not identify (gh #336). Already formatted the way the host
    /// reports it, with the server's own payload inside.
    UnattributedError(String),
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
    // gh #336: a JSON-RPC error the server could not attach to an id.
    //
    // Per JSON-RPC 2.0 a server that cannot parse the request id - or any
    // transport-level failure (parse error, internal error, overloaded) -
    // answers with `"id": null`. That is a *protocol-level answer to our
    // request*, not a missing reply and not somebody else's: the server is
    // telling us why it refused, and dropping it leaves the caller with LCA's
    // guess instead of the server's diagnosis.
    //
    // It is also **unattributable**, which is why it is its own variant rather
    // than folded into `Foreign`: the callers must fail the *current* wait with
    // this text, never retry, and never match it to a different pending call.
    // A message with a `method` and no `id` returned above as a Notification, so
    // this arm only sees error/interstitial messages.
    if message.get("id").is_some_and(serde_json::Value::is_null)
        && let Some(error) = message.get("error")
    {
        return Classified::UnattributedError(format!("MCP server errored: {error}"));
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
            // gh #336 on the stdio path: the server told us why it refused. Fail
            // *this* wait with its words, rather than the desync text below,
            // which would be a claim about the stream that is not true.
            Classified::UnattributedError(error) => return Err(error),
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

#[cfg(test)]
mod null_id_error_tests {
    //! gh #336 on **both** selectors, plus the neighbours the fix could swallow.
    //!
    //! The first #336 fix (`ba7f7fce`, Phase C) only taught the SSE selector,
    //! `select_response`. The stdio selector, `response_for`, kept classifying a
    //! null-id error as "an id that is not ours" and reported
    //! `MCP server answered without an id ... the stream is desynchronized` -
    //! a claim about the stream that was simply false, with the server's
    //! `"message"` dropped on the floor. The stdio path is where the issue was
    //! originally filed from, so the gap mattered more than the part that was
    //! fixed.
    //!
    //! These rows drive `response_for` with a scripted line source, so they test
    //! the semantics without a pipe.

    use super::*;

    /// A `response_for` line source over a fixed script.
    fn lines(script: &[&str]) -> impl FnMut() -> Result<Option<String>, String> {
        let mut index = 0;
        let owned: Vec<String> = script.iter().map(|line| (*line).to_string()).collect();
        move || {
            if index < owned.len() {
                index += 1;
                Ok(Some(owned[index - 1].clone()))
            } else {
                Ok(None)
            }
        }
    }

    fn null_error() -> &'static str {
        r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"Parse error"}}"#
    }

    // Verifies: gh #336 - the stdio path surfaces the server's own error instead
    // of inventing a desync. This is the row that would have caught the gap the
    // first fix left.
    #[test]
    fn the_stdio_path_surfaces_a_null_id_error_verbatim() {
        let error = response_for(4, lines(&[null_error()])).expect_err("not our reply");
        assert!(
            error.contains("Parse error"),
            "the server's message crosses intact: {error}"
        );
        assert!(error.contains("-32700"), "and so does its code: {error}");
        assert!(
            !error.contains("without an id"),
            "the desync text must not be used for a server that answered: {error}"
        );
        assert!(
            !error.contains("desynchronized"),
            "and it must not claim the stream is broken when the server explained \
             itself: {error}"
        );
    }

    // Verifies: gh #336 design point 4 - the payload is verbatim, not
    // reinterpreted (#159's precedent). A long, odd message and an unusual code
    // both survive byte-for-byte inside LCA's context.
    #[test]
    fn the_error_payload_crosses_verbatim() {
        let long = "a".repeat(600);
        let script = format!(
            r#"{{"jsonrpc":"2.0","id":null,"error":{{"code":-32099,"message":"{}"}}}}"#,
            long
        );
        let error =
            response_for(1, lines(&[script.as_str()])).expect_err("the error is the answer");
        assert!(error.contains(&long), "a 600-byte message is not truncated");
        assert!(error.contains("-32099"), "an unusual code is preserved");
        // The server's JSON is not re-encoded or re-worded: the object arrives as
        // it was sent.
        assert!(
            error.contains(r#""code":-32099"#) && error.contains(r#""message":"aaa"#),
            "the payload is the server's own JSON, not a re-serialization: {error}"
        );
    }

    // Verifies: gh #336 design point 3 - a notification is still not an error.
    //
    // The path `bug-ext-mcp-stdio-round-trip-desyncs-on-notifications` was about:
    // a server interleaving a `method`-bearing message with no `id` must be
    // skipped and the wait continued, not classified as an answer.
    #[test]
    fn a_notification_is_still_skipped() {
        let reply = response_for(
            7,
            lines(&[
                r#"{"jsonrpc":"2.0","method":"notifications/message","params":{}}"#,
                r#"{"jsonrpc":"2.0","id":7,"result":{"ok":true}}"#,
            ]),
        )
        .expect("the notification is skipped and our reply wins");
        assert_eq!(reply, Reply::Result(serde_json::json!({"ok": true})));
    }

    // Verifies: gh #336 design point 3 - a response for a *different* id is still
    // a desync on the stdio path, and still says so.
    //
    // A null-id error got its own variant; a foreign numeric id did not, and must
    // not: one is the server answering us, the other is the server answering
    // someone else while we wait. Collapsing them would hide a genuine desync.
    #[test]
    fn a_foreign_id_is_still_a_desync() {
        let error = response_for(4, lines(&[r#"{"jsonrpc":"2.0","id":99,"result":{}}"#]))
            .expect_err("a foreign id is desync");
        assert!(
            error.contains("another call") && error.contains("99"),
            "it names the ids: {error}"
        );
        assert!(
            !error.contains("errored"),
            "and does not pretend it was an error payload: {error}"
        );
    }

    // Verifies: gh #336 - a null-id error does not consume a *later* good reply,
    // and our own reply in the same envelope still wins on the SSE selector.
    //
    // This is the precedence the first fix got right and my first attempt at the
    // shared variant broke: taking the unattributed error inline let it shadow a
    // result in the same envelope.
    #[test]
    fn our_own_reply_wins_over_a_null_id_error() {
        let envelope = parse_sse_messages(
            "data: {\"id\":null,\"error\":{\"code\":-32603,\"message\":\"Internal error\"}}\n\n\
             data: {\"id\":5,\"result\":{\"ok\":true}}\n\n",
        );
        assert_eq!(
            select_response(&envelope, 5).expect("our result wins"),
            serde_json::json!({"ok": true})
        );
    }

    // Verifies: gh #336 - the normal success path is untouched.
    #[test]
    fn the_success_path_is_unchanged() {
        let reply = response_for(3, lines(&[r#"{"jsonrpc":"2.0","id":3,"result":{"v":1}}"#]))
            .expect("a result");
        assert_eq!(reply, Reply::Result(serde_json::json!({"v": 1})));
        let envelope = parse_sse_messages("data: {\"id\":3,\"result\":{\"v\":1}}\n\n");
        assert_eq!(
            select_response(&envelope, 3).expect("a result"),
            serde_json::json!({"v": 1})
        );
    }

    // Verifies: gh #336 - an *error for our own id* is still its own answer, and
    // is not confused with the unattributable kind. The two carry different text
    // shapes and both must keep working (gh #291 draws this line).
    #[test]
    fn an_error_for_our_id_is_still_attributed() {
        let reply = response_for(
            2,
            lines(&[
                r#"{"jsonrpc":"2.0","id":2,"error":{"code":-32601,"message":"no such method"}}"#,
            ]),
        )
        .expect("our error is an answer, not a transport failure");
        match reply {
            Reply::Error(text) => {
                assert!(text.contains("no such method"), "{text}");
                assert!(text.contains("-32601"), "{text}");
            }
            other => panic!("expected an attributed error, got {other:?}"),
        }
    }
}
