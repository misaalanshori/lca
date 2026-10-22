//! The synchronous-call driver (gh #286).
//!
//! # Why a thread exists at all
//!
//! The guest's WASI is linked with `add_to_linker_sync`, so a guest doing file
//! I/O blocks the OS thread it runs on. That is fine on a thread the host owns
//! and fatal on a runtime worker: blocking a Tokio worker stalls every other
//! future sharing it. So the *async* path (`pool_call`) hands its work to
//! `spawn_blocking`, and the *sync* path - `render`, `on_event`, `schema`,
//! `execute`, the ones the TUI calls per frame with no runtime polling - has to
//! get off the caller's thread somehow.
//!
//! Until this module that "somehow" was `std::thread::spawn(..).join()` **per
//! call**: one thread created, one stack allocated, one join, per TUI frame and
//! per stream delta. The guest work is the same either way; what cost hundreds of
//! times a second was the spawning.
//!
//! # What replaces it
//!
//! One long-lived driver thread per [`Inner`], created on first use, that owns
//! nothing and runs whatever closure it is handed. One thread per *extension*,
//! not per call - bounded by the number of loaded extensions.
//!
//! ADR-0014 line 23 rejected "a dedicated OS thread per active extension
//! instance". That rejection was about *scaling calls*: thread count growing with
//! extension count to give each guest its own execution context. This is not
//! that. The instance cache lives on whatever thread runs the call, and the
//! driver thread gives the guest no context of its own - it exists only to keep a
//! blocking guest off the caller's thread, created once instead of per call. The
//! ADR's own preferred alternative (async host functions cooperating on the
//! runtime) is what `pool_call` already does for the async path; the sync path
//! has no runtime to cooperate with, so it needs one thread, and one is the
//! smallest number it can need.
//!
//! # Why this cannot deadlock
//!
//! 1. **No re-entry.** A driver call cannot start another driver call on the same
//!    extension. The one place host code runs inside a guest is the
//!    `tools`/`completion` imports, and a nested tool call is handed to the
//!    *turn's* task over `NestedCall` (`host_imports.rs`): the guest thread blocks
//!    on `reply_rx`, the turn answers from its own task, and the driver is never
//!    asked for a second call.
//! 2. **The caller owns the wait; the driver holds no lock across a send.** Jobs
//!    arrive on one channel and each replies on its own. A busy driver makes the
//!    next caller queue, never spin.
//! 3. **Shutdown is observable, and does not join.** Dropping the job sender ends
//!    the `recv` loop, so the thread exits. It is not joined: a driver inside a
//!    guest call when its extension is dropped will finish or trap on its own
//!    epoch deadline, and joining there would hang the drop on a guest - the
//!    failure `disable` exists to avoid.
//!
//! # What it must not defeat
//!
//! **Epoch interruption** (`#103`/`#150`) is an engine-level property: a bump
//! reaches any store executing on any thread, and `arm` re-arms each store's
//! deadline at checkout. Nothing here touches the epoch, so a cancelled turn
//! still traps a guest mid-`blocking` call.
//!
//! **Trap isolation** is unchanged for the same reason: the cache lives behind
//! `Inner::cache`'s mutex and `classify` clears it on any wasm failure, whichever
//! thread noticed. The driver decides *where* a call runs, never whether the
//! cache survives it.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};

use super::{CallError, Inner};

/// How many driver threads this process has started.
///
/// The measurement the issue is about, made countable rather than timed: the
/// claimed win is "no thread spawn per call", and a counter states exactly that.
/// A wall-clock assertion would re-create the load sensitivity this repo has
/// spent cycles removing, and would be flaky on a shared CI runner by
/// construction.
///
/// Process-wide rather than per-driver because a test that runs several
/// extensions wants the total; a per-driver count is `1` and says nothing about
/// whether the *call* path spawned.
pub static DRIVER_SPAWNS: AtomicU64 = AtomicU64::new(0);

/// How many driver threads have *exited*: incremented as the `recv` loop ends,
/// which happens when the extension owning the sender is dropped.
///
/// Paired with [`DRIVER_SPAWNS`], this is what makes the no-leak claim checkable
/// without reading `/proc`: `spawns == exits` after dropping every extension
/// means every driver shut down. Counting threads in `/proc/self/task` does not
/// work for this - Wasmtime's own pools and the test harness contribute threads,
/// so a process-wide count moved 2 -> 7 in a measurement where the *driver* was
/// innocent (my first attempt at this row).
pub static DRIVER_EXITS: AtomicU64 = AtomicU64::new(0);

/// How many driver threads have exited since the process began.
#[must_use]
pub fn driver_exits() -> u64 {
    DRIVER_EXITS.load(Ordering::SeqCst)
}

/// How many driver threads have been started since the process began.
#[must_use]
pub fn driver_spawns() -> u64 {
    DRIVER_SPAWNS.load(Ordering::SeqCst)
}

/// One unit of work: runs the closure, which sends its own reply.
type Job = Box<dyn FnOnce(&Inner) + Send + 'static>;

/// The long-lived thread that runs every synchronous call for one extension.
pub(super) struct Driver {
    /// `None` until the thread is started. The sender lives here so the thread
    /// exits when the extension is dropped.
    jobs: Mutex<Option<Sender<Job>>>,
}

impl Driver {
    /// A driver with no thread yet: created on first use, so an extension that
    /// is loaded and never called costs nothing.
    pub(super) fn new() -> Driver {
        Driver {
            jobs: Mutex::new(None),
        }
    }

    /// Run `work` on the driver thread and return its result.
    ///
    /// If the thread cannot be started, `work` runs inline: the call still runs
    /// and still returns, so a failed spawn degrades to "no offload" rather than
    /// to an error the caller cannot act on. Inline is what the caller was doing
    /// before #286, minus the spawn.
    pub(super) fn call<T, F>(&self, inner: &Arc<Inner>, work: F) -> Result<T, CallError>
    where
        T: Send + 'static,
        F: FnOnce(&Inner) -> Result<T, CallError> + Send + 'static,
    {
        let Some(sender) = self.sender(inner) else {
            return work(inner);
        };
        let (reply_tx, reply_rx) = mpsc::channel::<Result<T, CallError>>();
        let job: Job = Box::new(move |inner: &Inner| {
            // A send error means the caller gave up; the work already ran, so
            // there is nothing left to do but drop the result.
            let _ = reply_tx.send(work(inner));
        });
        if sender.send(job).is_err() {
            // The driver is gone (it panicked, or the extension is being torn
            // down). Report it as disabled rather than losing the call.
            return Err(CallError::Disabled);
        }
        match reply_rx.recv() {
            Ok(result) => result,
            Err(_) => Err(CallError::Disabled),
        }
    }

    /// The driver's sender, starting the thread on first use.
    fn sender(&self, inner: &Arc<Inner>) -> Option<Sender<Job>> {
        let mut slot = super::lock(&self.jobs);
        if let Some(sender) = slot.as_ref() {
            return Some(sender.clone());
        }
        let (tx, rx) = mpsc::channel::<Job>();
        // `Weak`, not `Arc`: the driver must not keep its own extension alive.
        // `Inner` holds this `Driver` (and so the sender); the thread holding a
        // strong `Arc<Inner>` closes the loop, so the refcount never reaches
        // zero and neither the extension nor its instance cache is ever freed.
        // Found by `dropping_an_extension_shuts_its_driver_down` - the driver
        // never reported an exit.
        let owned = Arc::downgrade(inner);
        // `Builder::spawn` returns `Result`; `std::thread::spawn` panics on
        // failure, which is not acceptable in a render path.
        let started = std::thread::Builder::new()
            .name(format!("lca-ext-{}", inner.name))
            .spawn(move || driver_loop(rx, owned));
        match started {
            Ok(_handle) => {
                DRIVER_SPAWNS.fetch_add(1, Ordering::SeqCst);
                // The handle is dropped on purpose: the thread is detached and
                // exits when `jobs` drops (see the module docs).
                *slot = Some(tx.clone());
                Some(tx)
            }
            Err(_) => None,
        }
    }
}

/// The driver's receive loop: one job at a time, in arrival order.
///
/// Serial by design. A guest instance is not re-entrant - its store is `&mut`
/// for the duration of a call - so two calls on one extension must not run
/// concurrently. Before #286 they did not either: each got its own thread, and
/// `checkout` serialised them at the cache. Running them one at a time on one
/// thread keeps the same guarantee and removes the churn.
fn driver_loop(rx: mpsc::Receiver<Job>, inner: std::sync::Weak<Inner>) {
    while let Ok(job) = rx.recv() {
        // The extension may have been dropped between the send and this recv.
        // Nothing to run it against, so the job's `reply_tx` drops and the
        // caller sees a closed channel - which it already maps to `Disabled`.
        let Some(inner) = inner.upgrade() else {
            break;
        };
        // A host-glue panic disables the extension rather than killing the
        // driver, so the next call reports `Disabled` instead of hanging on a
        // dead channel. The release profile is `panic = "abort"`, where this
        // guard does not apply and a host panic is fatal - same as before.
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job(&inner))).is_err() {
            inner.disable();
        }
    }
    // The loop ends when every sender is gone: the `Driver` lived in `Inner`,
    // so this is the extension being dropped.
    DRIVER_EXITS.fetch_add(1, Ordering::SeqCst);
}
