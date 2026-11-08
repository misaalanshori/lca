//! Cooperative cancellation for one turn's work (FR-CONC-3).
//!
//! Lives beside the tools it cancels rather than in `lca-protocol`: it is a
//! runtime handle (it holds a `tokio::sync::Notify`), and the protocol crate
//! stays free of runtime types.
//!
//! Two ways to observe a cancel, for two kinds of caller:
//!
//! * [`CancelFlag::wait_cancelled`] — an async waiter, for a future that can be
//!   raced against it.
//! * [`CancelFlag::on_cancel`] — a registered callback, for a caller that is
//!   **not** a task: a synchronous WASM call blocks the thread it runs on, so it
//!   cannot await, and nothing it holds is reachable from the canceller. The
//!   callback runs on the cancelling thread, inside `cancel()`, before it
//!   returns (gh #547).

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

/// One registered callback, tagged with the key its guard removes it by.
type Listener = Box<dyn FnOnce() + Send + 'static>;

/// The shared state, behind the flag's `Arc`.
///
/// A struct rather than a tuple: `cancel()` and `CancelListener::drop` touch
/// different fields, and named fields make that disjointness obvious to a
/// reader instead of inferable from a destructuring pattern.
struct Inner {
    notify: tokio::sync::Notify,
    cancelled: AtomicBool,
    /// Registered callbacks, in registration order. A `Vec` because the
    /// population is one per turn - the turn registers one that fans out to the
    /// session's extensions - not one per extension or per call.
    listeners: Mutex<Vec<(u64, Listener)>>,
    /// Monotonic registration key. `drop` needs to find *its own* closure, and
    /// pointer identity cannot do that: a `FnOnce` boxed from a zero-sized
    /// closure shares its address with its siblings (gh #547).
    next_key: AtomicU64,
}

/// Cooperative cancellation for one turn's work (FR-CONC-3).
#[derive(Clone)]
pub struct CancelFlag {
    inner: Arc<Inner>,
}

impl Default for CancelFlag {
    fn default() -> Self {
        Self::new()
    }
}

impl CancelFlag {
    /// A fresh, uncancelled flag.
    pub fn new() -> CancelFlag {
        CancelFlag {
            inner: Arc::new(Inner {
                notify: tokio::sync::Notify::new(),
                cancelled: AtomicBool::new(false),
                listeners: Mutex::new(Vec::new()),
                next_key: AtomicU64::new(0),
            }),
        }
    }

    /// Trigger cancellation; every waiter wakes.
    ///
    /// Registered [`on_cancel`](CancelFlag::on_cancel) callbacks run here,
    /// synchronously, on this thread, **before this returns** - that is the
    /// whole point of the registry: a spinning extension's epoch bump is a
    /// store-visible side effect, not something to be delivered by a poller.
    ///
    /// The callbacks run **outside** the registry lock, so a callback that
    /// cancels again, or registers another listener, cannot deadlock. A
    /// callback that panics is caught and the remaining callbacks still run:
    /// one interrupted extension must not cost the others their interrupt.
    /// Two honest limits of that catch - it cannot act in a `panic = "abort"`
    /// build (this workspace's release profile), and the panic hook still
    /// prints before it fires - are why the guarantee is stated as "the rest
    /// still run" and not as "the panic disappears".
    pub fn cancel(&self) {
        self.inner.cancelled.store(true, Ordering::SeqCst);
        self.inner.notify.notify_waiters();
        // Take the whole list: these callbacks are consumed, and a second
        // `cancel()` must not fire them again.
        let drained = std::mem::take(&mut *self.inner.listeners());
        for (_, listener) in drained {
            if catch_unwind(AssertUnwindSafe(listener)).is_err() {
                // Swallowed deliberately: the cancelling thread keeps going, the
                // way any teardown's `let _ =` does. See the doc comment.
            }
        }
    }

    /// Whether cancellation has been triggered.
    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::SeqCst)
    }

    /// Wait until cancellation triggers (returns immediately when already
    /// cancelled, so a cancel between poll wakeups is never missed).
    pub async fn wait_cancelled(&self) {
        loop {
            if self.is_cancelled() {
                return;
            }
            let notified = self.inner.notify.notified();
            tokio::pin!(notified);
            // Register interest before re-checking the flag.
            notified.as_mut().enable();
            if self.is_cancelled() {
                return;
            }
            notified.await;
        }
    }

    /// Run `f` when (or if) this flag is cancelled; the returned
    /// [`CancelListener`] unregisters it when dropped.
    ///
    /// **Fires immediately if the flag is already cancelled.** A cancel that
    /// lands before the turn body gets to register must not be lost, which is
    /// also the shape a pre-cancelled turn takes (a queued cancel arriving
    /// while the previous turn was still winding down).
    ///
    /// Drop the guard to stop being called: a hundred-turn session that
    /// registered one listener per turn accumulates nothing.
    pub fn on_cancel<F: FnOnce() + Send + 'static>(&self, f: F) -> CancelListener {
        let key = self.inner.next_key.fetch_add(1, Ordering::Relaxed);
        {
            let mut listeners = self.inner.listeners();
            // The check is taken under the same lock the drain takes, and
            // `cancel()` sets the flag *before* it takes the lock. So either
            // this sees the flag and fires below, or the registration is
            // already in the list the drain will pick up - never both, never
            // neither.
            if !self.inner.cancelled.load(Ordering::SeqCst) {
                listeners.push((key, Box::new(f)));
                return CancelListener {
                    inner: Arc::clone(&self.inner),
                    key,
                };
            }
        }
        f();
        CancelListener {
            inner: Arc::clone(&self.inner),
            key,
        }
    }
}

/// Keeps one [`CancelFlag::on_cancel`] registration alive; dropping it removes
/// the callback.
///
/// Not `Clone`: two owners would have to agree on when to unregister, and the
/// last drop is the only unambiguous answer.
#[must_use = "dropping the listener unregisters the callback; bind it to a name that lives as long as the work it guards"]
pub struct CancelListener {
    inner: Arc<Inner>,
    key: u64,
}

impl Drop for CancelListener {
    fn drop(&mut self) {
        // A `cancel()` that already fired this listener took it out of the
        // list, so there is nothing to remove; `retain` makes that a no-op
        // rather than a special case.
        self.inner.listeners().retain(|(key, _)| *key != self.key);
    }
}

impl Inner {
    /// The registry, poisoned or not: no code path panics while holding it, and
    /// a poisoned lock would otherwise leak every registration behind it.
    fn listeners(&self) -> MutexGuard<'_, Vec<(u64, Listener)>> {
        self.listeners
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    // Verifies: gh #547 - a cancel that lands before the work registers a
    // listener is not lost. The work has already been told to stop; the
    // listener must say so at registration, not wait for a second cancel.
    #[tokio::test]
    async fn a_listener_registered_after_the_cancel_fires_immediately() {
        let flag = CancelFlag::new();
        flag.cancel();
        let fired = Arc::new(AtomicBool::new(false));
        let seen = Arc::clone(&fired);
        let _listener = flag.on_cancel(move || {
            seen.store(true, Ordering::SeqCst);
        });
        assert!(
            fired.load(Ordering::SeqCst),
            "registration on an already-cancelled flag fires before it returns"
        );
    }

    // Verifies: gh #547 - the guard is the unregister. A turn that finished
    // normally leaves nothing behind, so a later cancel (a user pressing
    // Escape on the *next* turn) cannot reach the work of the turn that is
    // already over.
    #[test]
    fn a_dropped_listener_does_not_fire() {
        let flag = CancelFlag::new();
        let fired = Arc::new(AtomicUsize::new(0));
        let seen = Arc::clone(&fired);
        let listener = flag.on_cancel(move || {
            seen.fetch_add(1, Ordering::SeqCst);
        });
        assert_eq!(fired.load(Ordering::SeqCst), 0, "not before a cancel");
        drop(listener);
        flag.cancel();
        assert_eq!(
            fired.load(Ordering::SeqCst),
            0,
            "a dropped listener is silent"
        );
        assert!(
            flag.is_cancelled(),
            "and the cancel is still visible through the flag, so nothing \
             that polls it is affected"
        );
    }

    // Verifies: gh #547 - the callback runs inside `cancel()`, on the
    // cancelling thread, before it returns. That is the 0 us propagation:
    // no poller, no wakeup, and the side effect is already visible to the
    // next line of the caller.
    #[test]
    fn the_callback_runs_before_cancel_returns() {
        let flag = CancelFlag::new();
        let bumped = Arc::new(AtomicBool::new(false));
        let seen = Arc::clone(&bumped);
        let _listener = flag.on_cancel(move || {
            seen.store(true, Ordering::SeqCst);
        });
        flag.cancel();
        assert!(
            bumped.load(Ordering::SeqCst),
            "the side effect is visible on the line after cancel()"
        );
    }

    // Verifies: gh #547 - one listener's panic does not cost the others their
    // interrupt. Extensions are independent: a broken one must not leave its
    // siblings spinning.
    #[test]
    fn a_panicking_listener_does_not_stop_the_rest() {
        let flag = CancelFlag::new();
        let second_ran = Arc::new(AtomicBool::new(false));
        let _first = flag.on_cancel(|| panic!("a broken interrupt handler"));
        let seen = Arc::clone(&second_ran);
        let _second = flag.on_cancel(move || {
            seen.store(true, Ordering::SeqCst);
        });
        flag.cancel();
        assert!(
            second_ran.load(Ordering::SeqCst),
            "the later listener still ran"
        );
        assert!(flag.is_cancelled(), "and the flag itself is still set");
    }

    // Verifies: gh #547 - a cancel is delivered once. `cancel()` is
    // idempotent (several paths can request it: the keyboard, a signal, a
    // timeout), so the callbacks must not run twice.
    #[test]
    fn a_second_cancel_does_not_refire_the_callbacks() {
        let flag = CancelFlag::new();
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = Arc::clone(&calls);
        let _listener = flag.on_cancel(move || {
            seen.fetch_add(1, Ordering::SeqCst);
        });
        flag.cancel();
        flag.cancel();
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "fired once, not once per cancel()"
        );
    }

    // Verifies: gh #547 - dropping one listener leaves the others registered,
    // which is the whole reason each registration carries its own key. It also
    // pins that unrelated listeners are not conflated: with pointer identity
    // two boxed zero-sized closures could share an address.
    #[test]
    fn dropping_one_listener_leaves_the_others_registered() {
        let flag = CancelFlag::new();
        let kept = Arc::new(AtomicUsize::new(0));
        let gone = Arc::new(AtomicUsize::new(0));
        let first = flag.on_cancel(|| panic!("dropped before it fires"));
        let kept_seen = Arc::clone(&kept);
        let _kept = flag.on_cancel(move || {
            kept_seen.fetch_add(1, Ordering::SeqCst);
        });
        let gone_seen = Arc::clone(&gone);
        let dropped = flag.on_cancel(move || {
            gone_seen.fetch_add(1, Ordering::SeqCst);
        });
        drop(first);
        drop(dropped);
        flag.cancel();
        assert_eq!(kept.load(Ordering::SeqCst), 1, "the surviving listener ran");
        assert_eq!(gone.load(Ordering::SeqCst), 0, "the dropped two did not");
    }

    // Verifies: gh #547 - a hundred-turn session accumulates nothing.
    //
    // This is the registry-leak half of the acceptance criteria: the same flag
    // outlives many turns, so the turn-scoped guard has to be what removes the
    // entry, not the flag's own lifetime.
    #[test]
    fn a_hundred_registrations_leave_no_listeners_behind() {
        let flag = CancelFlag::new();
        let ran = Arc::new(AtomicUsize::new(0));
        for _ in 0..100 {
            let seen = Arc::clone(&ran);
            let listener = flag.on_cancel(move || {
                seen.fetch_add(1, Ordering::SeqCst);
            });
            drop(listener);
        }
        assert!(
            flag.inner.listeners().is_empty(),
            "every turn's listener was unregistered by its own drop"
        );
        flag.cancel();
        assert_eq!(ran.load(Ordering::SeqCst), 0, "and none of them fired");
    }
}
