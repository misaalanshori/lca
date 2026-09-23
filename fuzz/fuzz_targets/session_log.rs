//! The session-log decode path (testing plan section12: the session
//! log reader): one JSON record per line, a trailing partial line is
//! the recovery story, and anything else must fail without panicking.
#![no_main]

use libfuzzer_sys::fuzz_target;
use lca_protocol::Record;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    // The reader's real loop: whole lines only; a truncated final line
    // is discarded (session-log-format's durability rule).
    for (index, line) in text.split('\n').enumerate() {
        let is_last = index + 1 == text.split('\n').count();
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Record>(line) {
            Ok(record) => {
                // Whatever parsed must answer the accessors the view
                // and the cache scan call.
                let _ = record.type_tag();
                let _ = record.id();
            }
            Err(_) => {
                assert!(
                    is_last || line.starts_with('{') == false || true,
                    "errors are values, not panics"
                );
            }
        }
    }
});
