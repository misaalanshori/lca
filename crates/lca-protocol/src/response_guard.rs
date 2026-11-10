//! The one owner of a host response handle (gh #577, gh #579's follow-up).
//!
//! # Why this is a type at all
//!
//! Seven provider extensions acquire a handle from `net_request` and then make
//! **fallible** calls - `net_response_status`, and `net_read_body` inside the
//! non-2xx path - before the driver that would eventually own the handle exists.
//! A `?` in that window returns with the handle still in the engine's table, so
//! every failed request leaked one. Closing manually is only correct when it
//! happens on **every** path, and "every path" is exactly what a `?` defeats.
//!
//! [`ResponseGuard`] makes ownership start on the line after the call that produced
//! the handle: it closes on `Drop`, and [`ResponseGuard::close`] closes early for
//! the caller that is about to return a failure. One owner, one close - so a
//! close-count assertion in a test can say "exactly once" and mean it.
//!
//! # Why it lives in `lca-protocol`
//!
//! Same reason [`crate::StreamFailure`] does (gh #576): several crates have to
//! name it and only one direction of dependency exists. `lca-wire-openai` can
//! reach it, but `extensions/anthropic`'s kit has no wire engine at all, and the
//! other providers do not all depend on the OpenAI kit. `lca-protocol` is where
//! [`crate::ProviderCap`] itself is declared, so the guard sits beside the
//! contract whose lifetime it enforces.
//!
//! This crate still performs no I/O: the guard holds a *borrow* and calls the
//! trait method the implementor already implements. The bytes travel on the
//! capability engine's side, exactly as they do when the extension calls
//! `net_close_response` itself.
//!
//! # The handle is an `Option` on purpose
//!
//! [`ResponseGuard::close`] consumes `self`, so it takes the handle out and leaves
//! `Drop` nothing to do. Closing twice would be a `NotFound` to the host (harmless
//! there) but a *counted* double close in a test, which is what makes
//! "closed exactly once" unprovable. `handle()` answers `0` for a guard that has
//! already given up its handle; no caller can ask, because only `close` can do
//! that and it consumes the guard.

use crate::ProviderCap;

/// Owns one host response handle from the line after `net_request` to its close.
///
/// ```ignore
/// let handle = cap.net_request("POST", url, headers, Some(body))?;
/// let guard = ResponseGuard::new(cap, handle);   // ownership starts here
/// let status = cap.net_response_status(guard.handle())?;   // `?` is now safe
/// ```
///
/// The generic parameter defaults to `dyn ProviderCap`, so a driver over a
/// concrete capability (the sandboxed providers' `StreamDriver<'a, C>`) names
/// `ResponseGuard<'a, C>` and the object-safe form names `ResponseGuard<'a>`.
pub struct ResponseGuard<'a, C: ProviderCap + ?Sized = dyn ProviderCap + 'a> {
    /// The capability the handle belongs to; its `net_close_response` is the close.
    cap: &'a C,
    /// `None` once [`Self::close`] has taken it.
    handle: Option<u32>,
}

impl<'a, C: ProviderCap + ?Sized> ResponseGuard<'a, C> {
    /// Take ownership of `handle`; it is closed when this guard drops.
    #[must_use]
    pub fn new(cap: &'a C, handle: u32) -> ResponseGuard<'a, C> {
        ResponseGuard {
            cap,
            handle: Some(handle),
        }
    }

    /// The handle, for the driver's own reads.
    ///
    /// Answers `0` if the guard were ever closed first, which no caller does:
    /// [`Self::close`] consumes `self`, so a closed guard cannot be asked. Avoiding
    /// `expect` here is the workspace lint (`unwrap_used`/`expect_used` denied), and
    /// a programming error would surface as a host `NotFound` on the first read rather
    /// than as a panic in a library.
    #[must_use]
    pub fn handle(&self) -> u32 {
        self.handle.unwrap_or(0)
    }

    /// Close it **now**, leaving nothing for `Drop`.
    ///
    /// The non-2xx path uses this before it builds the failure: a table entry
    /// outliving a failed request is the leak this type removes, and closing there
    /// rather than only on the success path is the fix. A caller that still needs
    /// to read the response headers (gh #278's ordering) reads them first.
    pub fn close(mut self) {
        self.close_now();
    }

    /// The body of both closes, so `close` and `Drop` cannot drift apart.
    fn close_now(&mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = self.cap.net_close_response(handle);
        }
    }
}

impl<C: ProviderCap + ?Sized> Drop for ResponseGuard<'_, C> {
    fn drop(&mut self) {
        self.close_now();
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::ResponseGuard;
    use crate::{CapabilityError, ProviderCap};

    /// A capability that counts closes: the property under test is a count, so a
    /// boolean "was closed" would pass on a path that closes twice.
    struct Counting {
        closes: AtomicUsize,
        closed: Mutex<bool>,
    }

    impl Counting {
        fn new() -> Counting {
            Counting {
                closes: AtomicUsize::new(0),
                closed: Mutex::new(false),
            }
        }
        fn closes(&self) -> usize {
            self.closes.load(Ordering::SeqCst)
        }
    }

    impl ProviderCap for Counting {
        fn net_request(
            &self,
            _method: &str,
            _url: &str,
            _headers: &[(&str, &str)],
            _body: Option<&[u8]>,
        ) -> Result<u32, CapabilityError> {
            Ok(11)
        }
        fn net_response_status(&self, _handle: u32) -> Result<u16, CapabilityError> {
            Ok(200)
        }
        fn net_read_body(
            &self,
            _handle: u32,
            _max: usize,
        ) -> Result<Option<Vec<u8>>, CapabilityError> {
            if *self.closed.lock().expect("closed") {
                return Err(CapabilityError::NotFound("unknown handle 11".to_string()));
            }
            Ok(None)
        }
        fn net_close_response(&self, _handle: u32) -> Result<(), CapabilityError> {
            self.closes.fetch_add(1, Ordering::SeqCst);
            *self.closed.lock().expect("closed") = true;
            Ok(())
        }
        fn credentials_get(&self, _key: &str) -> Option<String> {
            None
        }
        fn credentials_set(&self, _key: &str, _value: &str) -> Result<(), CapabilityError> {
            Ok(())
        }
        fn credentials_delete(&self, _key: &str) -> Result<(), CapabilityError> {
            Ok(())
        }
    }

    // Verifies: gh #577 - **one owner closes once, on drop.** The count is the
    // assertion: "closed" would also pass on a guard that closed twice.
    #[test]
    fn a_guard_closes_its_handle_exactly_once_when_it_drops() {
        let cap = Counting::new();
        {
            let guard: ResponseGuard<'_> = ResponseGuard::new(&cap, 11);
            assert_eq!(guard.handle(), 11, "the handle is readable while owned");
            assert_eq!(cap.closes(), 0, "nothing closes while the guard lives");
        }
        assert_eq!(cap.closes(), 1, "dropping the guard is the one close");
    }

    // Verifies: gh #577 - **`close` closes now and leaves `Drop` nothing to do.**
    // This is the non-2xx path's shape; a guard that closed twice would read 2 here.
    #[test]
    fn close_closes_immediately_and_drop_does_not_close_again() {
        let cap = Counting::new();
        let guard = ResponseGuard::new(&cap, 11);
        guard.close();
        assert_eq!(cap.closes(), 1, "the early close happened once");
    }

    // Verifies: gh #577 - **a closed handle is really gone**, which is what makes
    // gh #278's ordering a real constraint rather than a comment: the double answers
    // `NotFound` after the close, exactly as the host does.
    #[test]
    fn the_handle_is_gone_from_the_capability_after_the_close() {
        let cap = Counting::new();
        ResponseGuard::new(&cap, 11).close();
        assert!(
            cap.net_read_body(11, 64 * 1024).is_err(),
            "a read after the close is `NotFound`, as the host answers"
        );
    }

    // Verifies: gh #577 - **the guard is generic over a concrete capability**, which
    // is the shape `openai-compatible`'s `StreamDriver<'a, C>` needs; the default
    // parameter covers the `dyn ProviderCap` drivers. Both forms are named here so a
    // future signature change breaks this row instead of six extensions.
    #[test]
    fn the_guard_names_both_a_concrete_capability_and_the_trait_object() {
        let cap = Counting::new();
        let concrete: ResponseGuard<'_, Counting> = ResponseGuard::new(&cap, 11);
        assert_eq!(concrete.handle(), 11);
        let dynamic: ResponseGuard<'_> = ResponseGuard::new(&cap, 12);
        assert_eq!(dynamic.handle(), 12);
        drop(concrete);
        drop(dynamic);
        assert_eq!(cap.closes(), 2, "each guard closed its own handle once");
    }
}
