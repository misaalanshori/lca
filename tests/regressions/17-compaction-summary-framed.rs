//! Released defect (0.2.0): a compaction summary was injected as a bare
//! `user` message, so a resumed model treated its own compacted memory as an
//! untrusted note from the user (cycle-3 kink 2). The summary now carries
//! self-describing framing that names it as compacted history; the wire role
//! stays `user` (no ABI change).
//!
//! Verifies: FR-CTX-1.

use lca_core::assemble;
use lca_protocol::{FORMAT_VERSION, Record};

#[test]
fn a_compaction_summary_reaches_the_model_framed_as_compacted_history() {
    let records = vec![Record::Compaction {
        v: FORMAT_VERSION,
        ts: 1,
        id: "c1".into(),
        replaced_from: "u1".into(),
        replaced_to: "a1".into(),
        summary: "the codeword is BANANA".into(),
        strategy: "compaction-default".into(),
        usage: None,
    }];
    let assembled = assemble(&records, "sys");
    let body = assembled
        .messages
        .iter()
        .flat_map(|message| message.content.iter())
        .find_map(|block| match block {
            lca_protocol::ContentBlock::Text { text } if text.contains("BANANA") => {
                Some(text.clone())
            }
            _ => None,
        })
        .expect("the summary is on the wire");
    assert!(
        body.starts_with(
            "The conversation history before this point was compacted into the following summary:"
        ),
        "framed as compacted history, not a user note: {body}"
    );
    assert!(
        body.contains("<summary>") && body.contains("</summary>"),
        "self-describing tags: {body}"
    );
}
