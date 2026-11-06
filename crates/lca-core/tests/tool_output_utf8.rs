//! gh #405 gap 4: the **tool-output sink** must not invent replacement characters.
//!
//! The decoder is pinned on its own in `lca-protocol`'s `utf8_stream.rs`; this row
//! pins that the **seam which was broken** uses it. `crates/lca-core/src/turn/execute.rs`
//! decoded every pipe read with `String::from_utf8_lossy`, so a codepoint straddling a
//! read became `U+FFFD` and the bash card painted it. Measured before the fix:
//! `"output: 日本語の日本語の日本語の日本語"` in 8-byte reads produced **10** replacement
//! characters.
//!
//! A shell that writes a byte at a time is the worst case a real pipe can produce, and
//! it is the case a `from_utf8_lossy`-per-read cannot survive at all — so it is the
//! fixture.
//!
//! **Unix only, and not because the bug is.** The decoder is platform-neutral and its
//! own rows (`lca-protocol/tests/utf8_stream.rs`) run everywhere; what cannot run on
//! Windows is the *fixture*, which needs `od`/`while`/`printf` to emit one byte per
//! `write(2)` from a POSIX shell. `cmd.exe` has none of those, and the first version of
//! this row failed on `test (windows-latest)` exactly there — printing the command's own
//! help text as the "output". Gating the shape it cannot express is honest; editing the
//! assertion until Windows passed would have hidden the seam it exists to pin.

// The fixture is a POSIX shell pipeline (see the module doc): the *decoder* is
// platform-neutral and is pinned on every OS by `lca-protocol`'s rows.
#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code.
mod common;

use common::*;
use lca_core::TurnEvent;
use lca_testkit::FakeProvider;

/// The streamed output the turn emitted, concatenated in order.
fn streamed(sink: &CollectingSink) -> String {
    sink.events
        .iter()
        .filter_map(|e| match e {
            TurnEvent::ToolOutputChunk { chunk, .. } => Some(chunk.as_str()),
            _ => None,
        })
        .collect()
}

// Verifies: gh #405 gap 4 - **the text a card streams is the text the command
// wrote**, even when every codepoint crosses a read boundary.
//
// The fixture writes the document **one byte per `printf`**, so a UTF-8 codepoint is
// split across `write(2)` calls on every character. A shell that coalesced them would
// let a broken decoder pass; this one cannot.
#[tokio::test]
async fn streamed_tool_output_survives_a_split_codepoint() {
    let doc = "日本語の出力🎉テスト";
    // One byte per write: `od` turns the bytes into numbers, and the loop rebuilds
    // them into single-byte `printf` escapes.
    let script = format!(
        "printf '{doc}' | od -An -v -tu1 | tr -s ' ' '\\n' | while read -r b; do \
         [ -n \"$b\" ] && printf \"\\\\$(printf '%03o' \"$b\")\"; done"
    );
    // Two scripted turns: the tool call, then the answer the loop asks for once the
    // result is in. (One turn would end with an empty queue, which the fake reports
    // as a transport error - correct behaviour, wrong fixture.)
    let provider = FakeProvider::builder()
        .turn(|t| {
            t.tool_call(
                "shell",
                &serde_json::json!({ "command": script }).to_string(),
            )
            .usage(lca_testkit::fake_usage(10, 5, 0, 10))
        })
        .turn(|t| t.text("done").usage(lca_testkit::fake_usage(10, 5, 0, 10)))
        .build();
    let mut h = harness("gh405-toolout-utf8", provider, default_config());
    let mut sink = CollectingSink::default();
    let mut prompt = Prompt {
        answers: vec![lca_permissions::Decision::Always],
        asked: vec![],
    };
    let outcome = turn(&mut h, "go", &mut sink, &mut prompt).await;
    assert_eq!(outcome.status, lca_core::TurnStatus::Ok);

    let out = streamed(&sink);
    assert!(
        !out.is_empty(),
        "the shell's output streamed through the sink at all"
    );
    assert_eq!(
        out, doc,
        "the streamed text is the text the command wrote — no replacement characters \
         for codepoints that merely arrived late"
    );
    assert!(
        !out.contains(lca_protocol::REPLACEMENT),
        "and no U+FFFD survives into what the card paints: {out:?}"
    );
}
