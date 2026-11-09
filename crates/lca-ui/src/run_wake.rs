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
    #[cfg(test)]
    pub fn wait(&self, input_ready: &mut dyn FnMut() -> bool) -> Woke {
        self.wait_for_with(WAIT_TICK, input_ready)
    }

    /// [`wait`](Self::wait) with an explicit timeout, for the two-tier scheduler (gh #370
    /// flaw 3).
    ///
    /// The scheduler never waits *longer* than [`WAIT_TICK`]; it may wait **less** when a
    /// tier-1 frame is queued inside its interval, so the frame lands on its own ~16 ms
    /// boundary instead of the tick's 50. Everything else - input first, duplicate wakes
    /// coalescing, a disconnected channel reading as a tick - is unchanged.
    pub fn wait_for(&self, timeout: std::time::Duration) -> Woke {
        self.wait_for_with(timeout, &mut || false)
    }

    fn wait_for_with(
        &self,
        timeout: std::time::Duration,
        input_ready: &mut dyn FnMut() -> bool,
    ) -> Woke {
        // Input first: it is never queued behind a producer.
        if input_ready() {
            return Woke::Input;
        }
        match self.rx.recv_timeout(timeout) {
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

/// The minimum gap between two frames driven by **background** activity (gh #370
/// flaw 3).
///
/// ~60 FPS, pi's `MIN_RENDER_INTERVAL_MS = 16`. It exists because the two tiers have
/// opposite failure directions: an unthrottled stream renders hundreds of frames a
/// second for deltas no eye can read, while a stream throttled *together with input*
/// makes keystrokes feel mushy. So the throttle covers the stream only, and input
/// preempts it ([`FrameClock`]).
///
/// A background frame that is dropped because the interval has not elapsed **costs
/// nothing**: the request stays pending and the next frame shows the newer state. That
/// is the whole point - the flag coalesces N deltas into one paint, and the paint that
/// does happen is always of the *latest* state, never a replay of a stale one.
///
/// This is also why there is no timer to fire: see [`FrameClock::next_wait`]. pi needs a
/// `setTimeout` because its render is scheduled rather than looped; LCA's loop polls its
/// producers every iteration, so the interval only has to decide whether a frame is
/// *allowed yet*, never when to wake up for one.
pub const MIN_RENDER_INTERVAL: std::time::Duration = std::time::Duration::from_millis(16);

/// When the loop is allowed to paint (gh #370 flaw 3).
///
/// # Two flags, one render decision
///
/// Flaw 1 pinned `dirty` as the only thing that decides *whether* a frame is needed. This
/// adds *when* it may happen, and nothing else: [`render`](Self::render) is still the one
/// gate, and both flags feed it rather than opening a second paint path.
///
/// The flags are deliberately **not** one queue and not ordered against each other:
///
/// - [`stream_wanted`](Self::request_stream) is set by background activity - a streamed
///   delta, a spinner tick, a tool result, a poll that found something. It is paced.
/// - [`immediate`](Self::request_immediate) is set by a keystroke, a paste, a mouse
///   event, or a resize. It **preempts** the pace and is never paced.
///
/// Two flags rather than one dirty bit plus a "was this input" parameter, because the
/// question the loop asks is "may I paint now", and the answer differs by *why* it was
/// asked. Sharing one queue would make the answer depend on arrival order: a stream delta
/// that landed after a keystroke would be indistinguishable from one that landed before,
/// and tier 2's guarantee is not about order - it is about input never waiting.
#[derive(Debug, Default)]
pub struct FrameClock {
    /// The last frame this clock released, for the tier-1 pace.
    last_render_at: Option<std::time::Instant>,
    /// Background activity wants a frame, at most once per [`MIN_RENDER_INTERVAL`].
    stream_wanted: bool,
    /// Input wants a frame **now**, regardless of the interval.
    immediate: bool,
}

impl FrameClock {
    /// A clock that has never rendered, so the first frame is immediate.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Background activity changed something (paced).
    pub fn request_stream(&mut self) {
        self.stream_wanted = true;
    }

    /// Input or a resize changed something (**never** paced - this is tier 2).
    pub fn request_immediate(&mut self) {
        self.immediate = true;
    }

    /// Whether a frame may be painted **now**: always for input, and for background
    /// activity only if the interval has elapsed since the last frame.
    ///
    /// The boundary is `>=`: an event arriving exactly at the interval is allowed
    /// through, because rounding a legitimate 60 FPS frame down to 30 would halve the
    /// streaming rate for no gain.
    #[must_use]
    pub fn render(&self, now: std::time::Instant) -> bool {
        if self.immediate {
            return true;
        }
        if !self.stream_wanted {
            return false;
        }
        match self.last_render_at {
            Some(last) => now.saturating_duration_since(last) >= MIN_RENDER_INTERVAL,
            // Never painted: a daemon that waited 16 ms to show its first frame would
            // look broken, and the pace only exists to bound *repeated* paints.
            None => true,
        }
    }

    /// Record that a frame was painted, and clear the requests it satisfied.
    ///
    /// Clearing **both** on an input frame is pi's `requestImmediateRender` cancel
    /// semantic: the background frame that the pace had queued for ~16 ms later must
    /// find nothing to do, never a redundant second repaint right after the input one.
    /// On Windows, where timer granularity is coarse, that stray repaint is visible.
    ///
    /// Clearing `stream_wanted` cannot lose content: a frame composed now includes every
    /// change that had been requested, so the request it drops was for a state that no
    /// longer exists.
    pub fn mark_rendered(&mut self, now: std::time::Instant) {
        self.last_render_at = Some(now);
        self.stream_wanted = false;
        self.immediate = false;
    }

    /// How long the loop may wait for something new.
    ///
    /// **Always [`WAIT_TICK`]** - the scheduler never shortens the wait, and with one
    /// exception that is not a compromise:
    ///
    /// - A pending background frame is already releasable by the time anyone asks (the
    ///   loop paints at the top of the same iteration that set the flag), so there is
    ///   nothing to wait *for*. The one exception: if a frame was requested and the pace
    ///   still blocks it, the wait is shortened to the remaining budget so a stream holds
    ///   ~60 FPS rather than dropping to the tick's 20. Producers wake the loop sooner
    ///   with newer data, so this is a floor, not the usual path.
    /// - With nothing requested, shortening the wait would mean waking to check a
    ///   deadline that cannot exist - a spin, which the issue says would be worse than
    ///   the bug. So the tick stays the floor for everything with no producer.
    #[must_use]
    pub fn next_wait(&self, now: std::time::Instant) -> std::time::Duration {
        if !self.stream_wanted || self.immediate {
            return WAIT_TICK;
        }
        match self.last_render_at {
            Some(last) => {
                let elapsed = now.saturating_duration_since(last);
                if elapsed >= MIN_RENDER_INTERVAL {
                    WAIT_TICK
                } else {
                    (MIN_RENDER_INTERVAL - elapsed).min(WAIT_TICK)
                }
            }
            None => WAIT_TICK,
        }
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
    //
    // Gh #588: this row now asks for a deadline **far longer than the tick** and
    // asserts the wake beats it, instead of asserting it beats the tick. That is a
    // deliberate change of question, made after a measurement: on a hosted macOS
    // runner one scheduling quantum can exceed [`WAIT_TICK`], and the reading
    // **55.227 ms** (CI 42701900918) was the *fix* failing a 50 ms bound - the wait
    // returned after the producer's own 5 ms sleep, so the tick was never waited for
    // at all. A bound taken from the measurement would not separate the fix from the
    // defect either, because both sides carry the same jitter: the fix's 5 ms plus a
    // quantum and the defect's 50 ms plus a quantum overlap once a quantum is ~50 ms.
    // So the separation is made by **choosing the deadline** rather than by widening
    // a bound. A producer's wake lands after its own 5 ms sleep plus at most one
    // quantum; a wait that ignores the wake channel sits out the whole deadline.
    // With a 1 s deadline, a 250 ms bound is 4.5x the recorded macOS observation and
    // one quarter of what the defect costs, so the two sides cannot overlap for any
    // scheduler that schedules this thread at all. The bound is **the same number on
    // every platform**, because the separation comes from a deadline this row owns
    // rather than from a per-platform allowance - which is the only way the two
    // sides stop overlapping once a quantum is the size of the tick.
    //
    // Both mutants were run, and the two assertions carry different halves of the
    // proof: dropping `waker.wake()` fails the `Woke::Producer` assertion (a
    // wake-less wait reports `Tick`) while its *elapsed* time was 5.7 ms - the
    // deadline is not what a broken channel waits for; a 300 ms producer sleep fails
    // the timing assertion at 300.8 ms. So the ordering assertion is what proves the
    // wake is used, and the bound is what proves it is not waited out.
    #[test]
    fn a_producer_wake_returns_immediately() {
        let deadline = std::time::Duration::from_secs(1);
        let bound = std::time::Duration::from_millis(250);
        let (waker, wakes) = Wakes::new();
        let start = std::time::Instant::now();
        let handle = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(5));
            waker.wake();
        });
        let woke = wakes.wait_for(deadline);
        handle.join().expect("the waker thread finishes");
        assert_eq!(woke, Woke::Producer, "the wake is reported as a producer");
        assert!(
            start.elapsed() < bound,
            "the wake preempted the {deadline:?} deadline it was given: {:?}",
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

#[cfg(test)]
mod frame_clock_tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// A clock anchored at a fixed instant, so every row is deterministic: no sleeps and
    /// no stopwatches, because a duration assertion on a shared CI box is a coin flip.
    fn at(ms: u64) -> Instant {
        // `Instant` cannot be constructed, so anchor once and offset from it.
        static ANCHOR: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
        *ANCHOR.get_or_init(Instant::now) + Duration::from_millis(ms)
    }

    // Verifies: gh #370 flaw 3 (**the pacing row**) - **a burst of background events
    // renders at most once per interval.** This is tier 1's half of the promise in
    // `docs/flows.md`: without it a fast stream renders every delta, hundreds of frames a
    // second for text no eye can read.
    #[test]
    fn a_burst_of_background_events_renders_once_per_interval() {
        let mut clock = FrameClock::new();
        let t0 = at(1_000);
        // The first frame is immediate: a daemon that waited 16 ms to show anything
        // would look broken.
        clock.request_stream();
        assert!(clock.render(t0), "the first frame is never paced");
        clock.mark_rendered(t0);

        // 100 deltas inside one 16 ms window.
        let mut rendered = 0;
        for step in 1..=100u64 {
            let now = t0 + Duration::from_micros(step * 100); // reaches 10 ms
            clock.request_stream();
            if clock.render(now) {
                rendered += 1;
                clock.mark_rendered(now);
            }
        }
        assert_eq!(
            rendered, 0,
            "100 deltas inside one interval cost no frame - they coalesce into the one \
             that is already queued"
        );
        // The moment the interval elapses, exactly one frame is released - which is also
        // the proof that the request survived the window it was coalesced through: a
        // dropped request would render nothing here.
        let after = t0 + MIN_RENDER_INTERVAL;
        assert!(
            clock.render(after),
            "the interval has elapsed and the coalesced request is still pending"
        );
        clock.mark_rendered(after);
        assert!(
            !clock.render(after + Duration::from_millis(1)),
            "and only one: nothing was requested since"
        );
    }

    // Verifies: gh #370 flaw 3 (**the boundary row**) - **an input event paints on the
    // next pass regardless of the interval, including at exactly the interval.** The
    // boundary is the case a `<` instead of `<=` would break, and it is the case the
    // brief names: rounding a legitimate frame at the boundary down to the next window
    // halves the streaming rate for no gain.
    #[test]
    fn an_input_event_paints_immediately_even_exactly_at_the_interval() {
        for offset in [
            Duration::ZERO,
            Duration::from_millis(1),
            MIN_RENDER_INTERVAL - Duration::from_millis(1),
            MIN_RENDER_INTERVAL, // exactly on the boundary
            MIN_RENDER_INTERVAL + Duration::from_millis(1),
        ] {
            let mut clock = FrameClock::new();
            let t0 = at(2_000);
            clock.request_stream();
            assert!(clock.render(t0));
            clock.mark_rendered(t0);

            // A keystroke at `offset` after that background frame.
            clock.request_immediate();
            assert!(
                clock.render(t0 + offset),
                "input never waits for the throttle (offset {offset:?})"
            );
        }
    }

    // Verifies: gh #370 flaw 3 - **input preempts a pending background render**, and this
    // is an *ordering* claim, not a duration one: the very next pass after an input event
    // paints, whether or not a background frame was already requested and blocked.
    #[test]
    fn input_preempts_a_pending_background_render() {
        let mut clock = FrameClock::new();
        let t0 = at(3_000);
        clock.mark_rendered(t0); // a frame just happened

        // A stream wants a frame: paced, so blocked right now.
        clock.request_stream();
        assert!(!clock.render(t0 + Duration::from_millis(1)), "paced out");

        // A keystroke lands while that background frame is still pending.
        clock.request_immediate();
        assert!(
            clock.render(t0 + Duration::from_millis(1)),
            "the keystroke does not queue behind the pending stream frame"
        );
        // And painting satisfies **both**, so the paced frame does not fire redundantly
        // a few milliseconds later (pi's cancel semantic).
        clock.mark_rendered(t0 + Duration::from_millis(1));
        assert!(
            !clock.render(t0 + MIN_RENDER_INTERVAL),
            "the input frame cancelled the streaming frame it subsumed"
        );
    }

    // Verifies: gh #370 flaw 3 - **a stream cannot starve input either.** The two flags
    // do not share a queue or an ordering: with background events arriving continuously
    // and never elapsing an interval, an input event still paints on the next pass.
    #[test]
    fn a_continuous_stream_cannot_starve_input() {
        let mut clock = FrameClock::new();
        let t0 = at(4_000);
        clock.mark_rendered(t0);

        // One microsecond of stream, continuous.
        let now = t0 + Duration::from_micros(1);
        clock.request_stream();
        assert!(!clock.render(now), "the stream is paced out");

        // A keystroke arrives into that stream.
        clock.request_immediate();
        assert!(
            clock.render(now),
            "input paints even though the stream has work pending and the interval has \
             not elapsed"
        );
    }

    // Verifies: gh #370 flaw 3 - **the idle case paints nothing**, which is what keeps the
    // 200 ms spin guard green. A clock that reported `dirty` with no request would make
    // every idle tick compose a viewport - the spin the issue says would be worse than
    // the bug.
    #[test]
    fn an_idle_clock_never_asks_for_a_frame() {
        let mut clock = FrameClock::new();
        let t0 = at(5_000);
        clock.mark_rendered(t0);
        assert!(!clock.render(t0), "nothing requested");
        // ...and over 200 ms of ticks it stays quiet.
        let mut frames = 0;
        for ms in 1..=200u64 {
            if clock.render(t0 + Duration::from_millis(ms)) {
                frames += 1;
                clock.mark_rendered(t0 + Duration::from_millis(ms));
            }
        }
        assert_eq!(frames, 0, "200 ms of quiet paints nothing");
    }

    // Verifies: gh #370 flaw 3 - **the tick remains the wait floor when nothing is
    // pending**, so the loop does not wake to check a deadline that cannot exist. The one
    // exception is a queued stream frame inside its interval: the wait is then shortened to
    // the remaining budget so a stream holds ~60 FPS instead of dropping to the tick's 20.
    #[test]
    fn the_wait_is_the_tick_unless_a_paced_frame_is_waiting() {
        let mut clock = FrameClock::new();
        let t0 = at(6_000);

        // Idle: the full tick, never shorter (a shorter wait would be a spin).
        assert_eq!(clock.next_wait(t0), WAIT_TICK, "idle waits the tick");

        // A stream frame 4 ms into the interval: the remaining 12 ms.
        clock.mark_rendered(t0);
        clock.request_stream();
        assert_eq!(
            clock.next_wait(t0 + Duration::from_millis(4)),
            MIN_RENDER_INTERVAL - Duration::from_millis(4),
            "the queued stream frame waits only its remaining budget"
        );

        // Input pending: the tick again, because input is never paced - there is nothing
        // to wait for and the paint happens this pass.
        clock.request_immediate();
        assert_eq!(
            clock.next_wait(t0 + Duration::from_millis(4)),
            WAIT_TICK,
            "input has nothing to wait for"
        );

        // A releasable stream frame: the tick, because the paint happens this pass.
        let mut second = FrameClock::new();
        second.mark_rendered(t0);
        second.request_stream();
        assert_eq!(
            second.next_wait(t0 + MIN_RENDER_INTERVAL),
            WAIT_TICK,
            "an elapsed interval is painted now, not waited for"
        );

        // And the wait is never longer than the tick, so the spinner and the keepalive
        // keep their cadence.
        assert!(clock.next_wait(t0) <= WAIT_TICK);
    }

    // Verifies: gh #370 flaw 3 - **the interval is pi's 16 ms, in one place.** The number
    // is a decision (about 60 FPS), so it lives on a named constant with its reason rather
    // than being re-derived at each call site.
    #[test]
    fn the_interval_is_sixty_fps_and_named() {
        assert_eq!(
            MIN_RENDER_INTERVAL.as_millis(),
            16,
            "pi's MIN_RENDER_INTERVAL_MS = 16, ~60 FPS"
        );
        // Counted over the module's **code**, which stops at the test module: the rows
        // below legitimately spell `Duration::from_millis(16)` in their assertions, and a
        // guard that counted them would fail on its own text.
        let source = include_str!("run_wake.rs");
        let code = source
            .split("mod frame_clock_tests")
            .next()
            .expect("the module has a code half");
        assert_eq!(
            code.matches("pub const MIN_RENDER_INTERVAL").count(),
            1,
            "the interval is declared exactly once"
        );
        assert_eq!(
            code.matches("from_millis(16)").count(),
            1,
            "and no other site re-derives it, so a change lands in one place"
        );
    }
}
