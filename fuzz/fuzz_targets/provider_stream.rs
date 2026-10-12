//! The provider stream surface (testing plan section 13, gh #99):
//! arbitrary bytes through the SSE decoder (FR-PROV-7's event split),
//! the tool-call accumulator (FR-PROV-8's discard path), and argument
//! validation. No panics, no hangs, and no amplification: everything
//! accumulated came from the input, so 10^6 bogus argument deltas cost
//! input-proportional memory, never an accumulated bomb.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Stream the input in fuzzer-shaped chunks: split points ride on
    // the input itself, so chunk edges land mid-frame, mid-UTF-8, and
    // mid-`data:` line, exactly like a real socket.
    let split = |at: usize| at.min(data.len());
    let cuts = [
        split(data.len() / 3),
        split(data.len() / 3 + data.len() / 7 + 1),
    ];
    let mut decoder = lca_wire_openai::SseDecoder::default();
    let mut events = Vec::new();
    let mut start = 0usize;
    for cut in cuts {
        if cut > start {
            decoder.feed(&data[start..cut], &mut |event| events.push(event));
            start = cut;
        }
    }
    decoder.feed(&data[start..], &mut |event| events.push(event));
    decoder.finish(&mut |event| events.push(event));

    let mut accumulator = lca_provider::ToolCallAccumulator::default();
    for event in events {
        accumulator.handle(event);
    }
    // A stream that ends mid-call discards the open call: it is
    // reported, never returned runnable (FR-PROV-8).
    let (closed, _errors) = accumulator.finish(true);
    let mut total_arguments = 0usize;
    for call in &closed {
        total_arguments += call.arguments.len();
        // Joined arguments parse cleanly or fail as data, never as a
        // host crash (ADR-0004: the host parses once, after joining).
        let _ = lca_provider::validate_arguments(call);
    }
    assert!(
        total_arguments <= data.len().saturating_mul(4),
        "accumulated arguments outgrew the input {total_arguments} vs {}",
        data.len()
    );
});
