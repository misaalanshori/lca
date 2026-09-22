//! Phase 0 guest: two exported functions plus a pull-based stream resource.

wit_bindgen::generate!({
    path: "../wit",
    world: "two-func",
});

use core::cell::Cell;

use exports::lca::spike::streams::{Event, Events, Guest as StreamsGuest, GuestEvents};
use lca::spike::api;

const TOTAL: u64 = 10_000;

struct EventStream {
    next_seq: Cell<u64>,
}

impl GuestEvents for EventStream {
    fn next(&self) -> Option<Event> {
        let seq = self.next_seq.get();
        if seq >= TOTAL {
            return None;
        }
        self.next_seq.set(seq + 1);
        Some(Event {
            seq: seq + 1,
            payload: String::from("event"),
        })
    }
}

struct Component;

impl Guest for Component {
    fn compute(n: u64) -> u64 {
        api::host_add(n, 1)
    }
}

impl StreamsGuest for Component {
    type Events = EventStream;

    fn start() -> Events {
        Events::new(EventStream { next_seq: Cell::new(0) })
    }
}

export!(Component);
