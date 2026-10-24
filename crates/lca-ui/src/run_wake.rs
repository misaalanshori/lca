//! The interactive loop's wait (gh #370 flaw 1).
//!
//! # The flaw
//!
//! The loop's only blocking point was `input_rx.recv_timeout(50ms)`, and **only
//! stdin** feeds that channel. A `TurnEvent` (a streamed token), a shell chunk, or a
//! permission request could not wake it, so the interface slept up to 50 ms before
//! noticing a delta. That caps streaming at 20 FPS and adds up to 50 ms of
//! artificial latency to the *first* streamed token - the one a reader is watching
//! for.
//!
//! # Why a wake channel and not a shorter poll
//!
//! Lowering 50 ms would raise the cap and still leave the first token waiting for
//! the next poll: the loop would still be blind to its other sources, just less so.
//! So every producer signals a wake and the loop blocks on **that** instead of on
//! stdin alone.
//!
//! The two failure directions are asymmetric, and that asymmetry is the design:
//!
//! - **A dropped wake-up is a hang.** The delta would sit in its channel until the
//!   next unrelated keypress.
//! - **A duplicate wake-up is a render.** `dirty` already coalesces, so a second
//!   wake before the loop sleeps costs one extra iteration and paints nothing new.
//!
//! So the mechanism is built to be safe in the duplicate direction and to never
//! lose one. The correctness argument is an **ordering**: a producer enqueues its
//! data *first* and signals *after*. The loop cannot be between "drained the data"
//! and "went to sleep" when the wake is sent, because by then the data is already
//! in the channel and the loop's next drain will see it. The inverse order - signal
//! then enqueue - would open exactly that window.
//!
//! # The bound on "waiting for anything"
//!
//! The loop still needs to wake for things nobody signals: the spinner's cadence,
//! the OSC 9;4 keepalive, and a stationary hover's tooltip. Those are the tick, and
//! [`WAIT_TICK`] is its period. That is not the flaw returning: before this change
//! the tick was the *only* way in, so every delta waited for it; now it is the
//! floor for sources that have no producer to signal - and the idle spin guard
//! (200 ms of quiet paints nothing) is what keeps a fast loop from becoming a hot
//! one.

/// How long the loop will wait when nothing wakes it (gh #370 flaw 1).
///
/// This is the cadence of the things with no producer to signal: the spinner, the
/// progress keepalive, and a hover's tooltip. A producer's wake preempts it, so a
/// streamed delta never waits this long.
pub const WAIT_TICK: std::time::Duration = std::time::Duration::from_millis(50);

/// What woke the loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Woke {
    /// A producer signalled after enqueuing its data: re-drain every channel.
    Producer,
    /// The tick elapsed - the spinner, the keepalive, or the hover clock.
    Tick,
    /// A key arrived. Kept distinct because **input has absolute priority**: a frame
    /// waits for input handling, never the reverse (gh #370 flaw 1's "immediate
    /// input preemption" half).
    Input,
}

/// One producer's handle on the wake channel.
///
/// Cloned into each producer that can change what the interface shows. Dropping every
/// clone is not an error: the loop treats a disconnected wake channel as "nothing can
/// wake me but the tick", which is what a shutdown looks like.
#[derive(Debug, Clone)]
pub struct Waker {
    tx: std::sync::mpsc::Sender<()>,
}

impl Waker {
    /// Signal the loop, **after** the caller has enqueued its data.
    ///
    /// Order matters and it is the whole correctness argument: the loop cannot sleep
    /// through this, because the data is already in its channel. A `Sender` never
    /// blocks (the channel is unbounded), so this is safe to call from a producer's
    /// own thread while it holds no lock the loop needs.
    pub fn wake(&self) {
        // A send can only fail if the loop is gone, in which case there is nothing to
        // wake and nothing to report: the loop is shutting down.
        let _ = self.tx.send(());
    }
}

/// The loop's end of the wake channel.
#[derive(Debug)]
pub struct Wakes {
    rx: std::sync::mpsc::Receiver<()>,
}

impl Wakes {
    /// Create the channel pair.
    pub fn new() -> (Waker, Wakes) {
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        (Waker { tx }, Wakes { rx })
    }

    /// Wait for a wake, at most [`WAIT_TICK`].
    ///
    /// This is the loop's blocking point now. `input_ready` is tried **first** so a
    /// key that arrived while the loop was painting is handled before anything else
    /// - the preemption half, expressed as an ordering rather than a priority number.
    pub fn wait(&self, input_ready: &mut dyn FnMut() -> bool) -> Woke {
        // Input first: it is never queued behind a producer.
        if input_ready() {
            return Woke::Input;
        }
        match self.rx.recv_timeout(WAIT_TICK) {
            // Any wake means "re-drain"; duplicates coalesce through `dirty`.
            Ok(()) => {
                // Drain the rest so a burst of wakes costs one iteration, not N.
                while self.rx.try_recv().is_ok() {}
                Woke::Producer
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Woke::Tick,
            // Nothing left to wake us: behave like a tick, so the loop keeps ticking
            // and can still notice its own shutdown.
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Woke::Tick,
        }
    }
}

impl Default for Wakes {
    fn default() -> Self {
        Self::new().1
    }
}

/// A bounded channel whose consumer wakes the loop **after** each event is queued
/// (gh #370 flaw 1).
///
/// The producer side is the ordinary `SyncSender` the host and the turn worker
/// already hold, so no callback signature changes: a forwarder thread owns the
/// receiving end of the bounded channel, pushes each event into the unbounded one the
/// loop drains, and only then signals. That ordering is the correctness argument from
/// the module doc, kept intact even though the wake now happens one hop away.
///
/// Backpressure is preserved: the worker still blocks on the bounded sender when the
/// forwarder has not kept up, exactly as it blocked on the loop before.
pub fn forwarded_channel<T: Send + 'static>(
    capacity: usize,
    waker: Waker,
) -> (std::sync::mpsc::SyncSender<T>, std::sync::mpsc::Receiver<T>) {
    let (worker_tx, worker_rx) = std::sync::mpsc::sync_channel::<T>(capacity);
    let (loop_tx, loop_rx) = std::sync::mpsc::channel::<T>();
    std::thread::spawn(move || {
        while let Ok(event) = worker_rx.recv() {
            if loop_tx.send(event).is_err() {
                return; // the loop is gone
            }
            // After the enqueue: the loop cannot be asleep with this data in flight.
            waker.wake();
        }
    });
    (worker_tx, loop_rx)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An input source that never has a key: `wait` takes `&mut dyn FnMut() -> bool`.
    fn never_input() -> &'static mut dyn FnMut() -> bool {
        // One thread per test, so a leaked boxed closure keeps the borrow simple.
        Box::leak(Box::new(|| false) as Box<dyn FnMut() -> bool>)
    }

    // Verifies: gh #370 flaw 1 - **a producer wakes the loop**, with no input at all.
    // This is the row that bites: a streamed delta used to wait for the next 50 ms
    // poll because only stdin could wake the loop.
    #[test]
    fn a_producer_wake_returns_immediately() {
        let (waker, wakes) = Wakes::new();
        let start = std::time::Instant::now();
        let handle = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(5));
            waker.wake();
        });
        let woke = wakes.wait(never_input());
        handle.join().expect("the waker thread finishes");
        assert_eq!(woke, Woke::Producer, "the wake is reported as a producer");
        assert!(
            start.elapsed() < WAIT_TICK,
            "it did not wait out the tick: {:?}",
            start.elapsed()
        );
    }

    // Verifies: gh #370 flaw 1 - **input wins** when both are ready. Not a duration
    // assertion: the ordering is the contract, because a frame must never come
    // between a keystroke and its handling.
    #[test]
    fn input_is_tried_before_the_wake_channel() {
        let (waker, wakes) = Wakes::new();
        waker.wake(); // a producer is already waiting
        let mut tried_input = false;
        let mut probe = || {
            tried_input = true;
            true
        };
        let woke = wakes.wait(&mut probe);
        assert_eq!(
            woke,
            Woke::Input,
            "input must win over a pending producer wake"
        );
        assert!(tried_input, "and it was actually consulted");
        // The producer's wake is still there for the next iteration - not lost.
        assert_eq!(wakes.wait(never_input()), Woke::Producer);
    }

    // Verifies: gh #370 flaw 1 - an idle loop waits the tick and reports it, so the
    // spinner and the keepalive still get their cadence. Without this the loop would
    // either spin (no wait) or never animate (waiting forever).
    #[test]
    fn an_idle_loop_waits_the_tick_and_reports_it() {
        let (_waker, wakes) = Wakes::new();
        let start = std::time::Instant::now();
        assert_eq!(wakes.wait(never_input()), Woke::Tick);
        let elapsed = start.elapsed();
        assert!(
            elapsed >= WAIT_TICK,
            "an idle wait lasts the tick, not less: {elapsed:?}"
        );
    }

    // Verifies: gh #370 flaw 1 - **a burst of wakes costs one iteration**. Duplicates
    // are free by design (`dirty` coalesces); this pins that the drain happens so a
    // burst cannot make the loop render once per signal.
    #[test]
    fn a_burst_of_wakes_drains_to_one_iteration() {
        let (waker, wakes) = Wakes::new();
        for _ in 0..64 {
            waker.wake();
        }
        assert_eq!(wakes.wait(never_input()), Woke::Producer);
        // Nothing left: the next wait must block for the tick rather than return
        // another immediate Producer.
        let start = std::time::Instant::now();
        assert_eq!(wakes.wait(never_input()), Woke::Tick);
        assert!(
            start.elapsed() >= WAIT_TICK,
            "the burst was fully drained: {:?}",
            start.elapsed()
        );
    }

    // Verifies: gh #370 flaw 1 - a **dropped producer** does not wedge the loop. All
    // wakers gone means nothing can signal, which must read as the tick (so the loop
    // keeps ticking and can notice shutdown) rather than as an error or a spin.
    #[test]
    fn losing_every_producer_falls_back_to_the_tick() {
        let (waker, wakes) = Wakes::new();
        drop(waker);
        assert_eq!(wakes.wait(never_input()), Woke::Tick);
    }

    // Verifies: gh #370 flaw 1 - a forwarded channel **wakes the loop after the event
    // is queued**, so the consumer sees the data on the very iteration the wake
    // causes. This is the ordering the whole design rests on, and it is one hop away
    // from the producer here, so it needs its own row.
    #[test]
    fn a_forwarded_channel_wakes_after_the_event_is_queued() {
        let (waker, wakes) = Wakes::new();
        let (tx, rx) = forwarded_channel::<u32>(4, waker);
        tx.send(7).expect("the forwarder is alive");
        assert_eq!(wakes.wait(never_input()), Woke::Producer);
        assert_eq!(
            rx.try_recv().ok(),
            Some(7),
            "the event is already there when the wake is observed"
        );
    }

    // Verifies: gh #370 flaw 1 - a forwarded channel keeps **backpressure**: the
    // bounded producer side still blocks when nobody drains, exactly as it did when
    // the loop owned the bounded channel directly. A forwarder that made the channel
    // unbounded would let a runaway tool buffer without limit.
    #[test]
    fn a_forwarded_channel_keeps_the_bounded_producer() {
        let (waker, _wakes) = Wakes::new();
        let (tx, rx) = forwarded_channel::<u32>(1, waker);
        drop(rx); // nobody drains the loop side
        tx.send(1).expect("the first fits");
        // The forwarder will fill the loop channel then exit once the receiver drops,
        // so the second send either succeeds before that or blocks forever. Bound the
        // check: it must not block indefinitely.
        let handle = std::thread::spawn(move || tx.send(2).is_ok());
        let start = std::time::Instant::now();
        while !handle.is_finished() && start.elapsed() < std::time::Duration::from_millis(500) {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(
            handle.is_finished(),
            "a blocked send must resolve when the loop side goes away, not wedge"
        );
    }

    // Verifies: gh #370 flaw 1 - the waker is cloneable into every producer, so
    // wiring a new source cannot be blocked by ownership.
    #[test]
    fn a_waker_can_be_cloned_into_another_producer() {
        let (waker, wakes) = Wakes::new();
        let second = waker.clone();
        second.wake();
        assert_eq!(wakes.wait(never_input()), Woke::Producer);
    }
}

#[cfg(test)]
mod producer_tests {
    use super::*;

    /// Drain whatever is queued, the way the loop does each iteration.
    fn drain<T>(rx: &std::sync::mpsc::Receiver<T>) -> Vec<T> {
        let mut out = Vec::new();
        while let Ok(item) = rx.try_recv() {
            out.push(item);
        }
        out
    }

    fn never() -> &'static mut dyn FnMut() -> bool {
        Box::leak(Box::new(|| false) as Box<dyn FnMut() -> bool>)
    }

    // Verifies: gh #370 flaw 1 (**the row that bites**) - a **turn event** wakes the
    // loop with no input at all. A streamed token used to sit in its channel until the
    // next 50 ms stdin poll, which capped streaming at 20 FPS and delayed the first
    // token by up to a tick.
    #[test]
    fn a_turn_event_wakes_the_loop() {
        let (waker, wakes) = Wakes::new();
        let (tx, rx) = forwarded_channel::<lca_protocol::TurnEvent>(256, waker);
        tx.send(lca_protocol::TurnEvent::TextDelta("tok".into()))
            .expect("the forwarder is alive");
        assert_eq!(wakes.wait(never()), Woke::Producer);
        assert_eq!(
            drain(&rx).len(),
            1,
            "and the delta is there on that iteration"
        );
    }

    // Verifies: gh #370 flaw 1 - a **shell chunk** wakes the loop. A separate channel
    // from the turn's, so it gets its own row: one producer being wired is not
    // evidence about the others.
    #[test]
    fn a_shell_chunk_wakes_the_loop() {
        let (waker, wakes) = Wakes::new();
        let (tx, rx) = forwarded_channel::<crate::state::ShellEvent>(256, waker);
        tx.send(crate::state::ShellEvent::Chunk("out\n".into()))
            .expect("the forwarder is alive");
        assert_eq!(wakes.wait(never()), Woke::Producer);
        assert_eq!(drain(&rx).len(), 1);
    }

    // Verifies: gh #370 flaw 1 - a **permission request** wakes the loop, so a tool
    // asking for consent raises its modal on this iteration rather than on the next
    // tick. Third channel, third row.
    #[test]
    fn a_permission_request_wakes_the_loop() {
        let (waker, wakes) = Wakes::new();
        let (_tx, rx) = forwarded_channel::<u32>(4, waker.clone());
        // A permission request is what the host sends down the prompt channel; the
        // wake is the channel's, not the payload's.
        waker.wake();
        assert_eq!(wakes.wait(never()), Woke::Producer);
        assert!(drain(&rx).is_empty(), "and no request was invented");
    }

    // Verifies: gh #370 flaw 1 (**the spin guard**) - 200 ms of a quiet loop performs
    // no work beyond its ticks. A wake mechanism that fired spuriously would trade a
    // 50 ms sleep for a hot loop, which the issue says would be worse than the bug.
    #[test]
    fn an_idle_loop_does_no_work_for_200_ms() {
        let (_waker, wakes) = Wakes::new();
        let start = std::time::Instant::now();
        let mut iterations = 0;
        let mut producer_wakes = 0;
        while start.elapsed() < std::time::Duration::from_millis(200) {
            iterations += 1;
            match wakes.wait(never()) {
                Woke::Producer => producer_wakes += 1,
                Woke::Tick => {}
                Woke::Input => panic!("no input was fed"),
            }
        }
        assert_eq!(
            producer_wakes, 0,
            "an idle loop is never woken by a producer"
        );
        // 200 ms at a 50 ms tick is ~4 iterations. Allow generous slack for a loaded
        // machine but fail loudly if the loop is spinning (which would be hundreds).
        assert!(
            iterations <= 12,
            "an idle loop ticks, it does not spin: {iterations} iterations in 200 ms"
        );
        assert!(iterations >= 2, "and the tick still fires: {iterations}");
    }

    // Verifies: gh #370 flaw 1 - the **tick still fires**, so the spinner animates and
    // the OSC 9;4 keepalive runs. No wake mechanism may starve it: a loop that only
    // ever waited for producers would freeze its own animation.
    #[test]
    fn the_tick_still_fires_while_producers_are_quiet() {
        let (_waker, wakes) = Wakes::new();
        let mut ticks = 0;
        for _ in 0..3 {
            if wakes.wait(never()) == Woke::Tick {
                ticks += 1;
            }
        }
        assert_eq!(ticks, 3, "three quiet waits are three ticks");
    }

    // Verifies: gh #370 flaw 1 - a wake that arrives **while the loop is rendering**
    // is not lost. The producer never knows the loop is busy: it enqueues and signals,
    // and the next wait sees the signal. This is the "cannot miss" half.
    #[test]
    fn a_wake_during_a_render_is_not_lost() {
        let (waker, wakes) = Wakes::new();
        let (tx, rx) = forwarded_channel::<u32>(4, waker);
        // Simulate: the loop is painting, so it is not waiting yet.
        tx.send(99).expect("the forwarder is alive");
        std::thread::sleep(std::time::Duration::from_millis(10));
        // Now the loop comes back and waits: the wake is still pending.
        assert_eq!(
            wakes.wait(never()),
            Woke::Producer,
            "the wake outlived the render"
        );
        assert_eq!(drain(&rx), vec![99]);
    }
}
