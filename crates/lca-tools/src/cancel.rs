//! Cooperative cancellation for one turn's work (FR-CONC-3).
//!
//! Lives beside the tools it cancels rather than in `lca-protocol`: it is a
//! runtime handle (it holds a `tokio::sync::Notify`), and the protocol crate
//! stays free of runtime types.

use std::sync::Arc;

/// Cooperative cancellation for one turn's work (FR-CONC-3).
#[derive(Clone)]
pub struct CancelFlag {
    inner: Arc<(tokio::sync::Notify, std::sync::atomic::AtomicBool)>,
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
            inner: Arc::new((
                tokio::sync::Notify::new(),
                std::sync::atomic::AtomicBool::new(false),
            )),
        }
    }

    /// Trigger cancellation; every waiter wakes.
    pub fn cancel(&self) {
        self.inner
            .1
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.inner.0.notify_waiters();
    }

    /// Whether cancellation has been triggered.
    pub fn is_cancelled(&self) -> bool {
        self.inner.1.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Wait until cancellation triggers (returns immediately when already
    /// cancelled, so a cancel between poll wakeups is never missed).
    pub async fn wait_cancelled(&self) {
        loop {
            if self.is_cancelled() {
                return;
            }
            let notified = self.inner.0.notified();
            tokio::pin!(notified);
            // Register interest before re-checking the flag.
            notified.as_mut().enable();
            if self.is_cancelled() {
                return;
            }
            notified.await;
        }
    }
}
