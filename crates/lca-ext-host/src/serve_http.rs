//! The `serve-http` world's host half (gh #363, ADR-0049): one
//! [`lca_tools::ServeHttpNamespace`] per loaded extension, so the listener's
//! router can call a guest and get a response back.
//!
//! Two things are worth stating because they are not obvious from the code:
//!
//! - **The guest call runs on the extension's driver thread**, the same one
//!   `render` and every other synchronous world use. The listener's accept
//!   loop is a plain thread with no runtime, which is exactly the blocking
//!   region ADR-0014 requires for synchronous WASI.
//! - **A trap disables the extension but not the listener.** A guest that
//!   exhausts fuel mid-response is a bad extension; taking the port down with
//!   it would break every other extension's UI on the same port, so the
//!   listener answers 500 and keeps serving.
//!
//! The mount holds a `Weak<Inner>` rather than a strong reference: the
//! extension's handle owns the cache and driver the guest call needs, and a
//! strong reference from the mount back to the handle would be a cycle that
//! keeps an unloaded extension's guest cache alive.

use super::*;
use lca_ext_abi::host::serve_http::exports::lca::ext as serve_exports;

/// The mount a loaded extension serves on: its identity, its session id, and a
/// way back to the handle that owns its guest cache.
pub(super) struct ServeHttpMount {
    extension: String,
    session: String,
    /// `Weak` so the listener holding a mount does not keep the extension
    /// alive after it is unloaded.
    inner: std::sync::Weak<Inner>,
}

/// The dispatch-trait entry point (gh #363): `None` when the manifest did not
/// declare `serve-http`, so there is no route to mount rather than a route that
/// refuses.
pub(crate) fn mount_for(
    handle: &WasmExtension,
    session: &str,
) -> Option<Arc<dyn lca_protocol::http::ServeHttpNamespace>> {
    handle.inner.serve_http.as_ref()?;
    Some(Arc::new(ServeHttpMount::new(
        handle.inner.name.clone(),
        session.to_string(),
        Arc::downgrade(&handle.inner),
    )) as Arc<dyn lca_protocol::http::ServeHttpNamespace>)
}

impl ServeHttpMount {
    /// Build the mount for a loaded extension.
    pub(super) fn new(
        extension: impl Into<String>,
        session: impl Into<String>,
        inner: std::sync::Weak<Inner>,
    ) -> Self {
        ServeHttpMount {
            extension: extension.into(),
            session: session.into(),
            inner,
        }
    }
}

impl lca_protocol::http::ServeHttpNamespace for ServeHttpMount {
    fn extension(&self) -> &str {
        &self.extension
    }

    fn session(&self) -> &str {
        &self.session
    }

    fn answer(
        &self,
        base_url: &str,
        request: lca_protocol::http::HttpRequest,
    ) -> Result<lca_protocol::http::HttpResponse, CapabilityError> {
        let Some(inner) = self.inner.upgrade() else {
            return Err(CapabilityError::NotFound(
                "the extension was unloaded".to_string(),
            ));
        };
        // The capability gate: a manifest that declared the world but whose
        // grant was revoked answers not-granted rather than serving. The
        // listener only mounts granted extensions, so this fires on a
        // revocation between mount and request.
        if !inner.cap.serves_http() {
            return Err(inner.cap.refused_for(
                "serve-http",
                &request.path,
                CapabilityError::NotGranted("serve-http is not granted".into()),
            ));
        }
        let base_url = base_url.to_string();
        // The driver speaks `CallError`; the capability boundary speaks
        // `CapabilityError`. The guest's own refusal is carried in the
        // message so the listener can put it in the 500 body, which is what a
        // developer looking at their browser needs to see.
        match inner
            .driver
            .call(&inner, move |inner| serve_work(inner, &base_url, request))
        {
            Ok(response) => Ok(response),
            Err(CallError::GuestReason(err)) => Err(err),
            Err(other) => Err(CapabilityError::Io(other.to_string())),
        }
    }

    fn record_refusal(&self, parameter: &str, reason: &str) {
        if let Some(inner) = self.inner.upgrade() {
            inner.cap.note_serve_http(parameter, reason);
        }
    }
}

/// Perform one guest call and translate the WIT result.
fn serve_work(
    inner: &Inner,
    base_url: &str,
    request: lca_tools::IncomingRequest,
) -> Result<lca_protocol::http::HttpResponse, CallError> {
    use serve_exports::handler::{Error as WasmError, Request as WasmRequest};
    let wasm_request = WasmRequest {
        method: request.method,
        path: request.path,
        query: request.query,
        headers: request.headers,
        body: request.body,
    };
    let (mut store, instance) = inner.checkout_serve_http()?;
    let outcome = instance
        .lca_ext_handler()
        .call_handle(&mut store, base_url, &wasm_request)
        .map_err(|err| inner.classify(err))?;
    inner.checkin_serve_http(store, instance);
    match outcome {
        Ok(response) => Ok(lca_protocol::http::HttpResponse {
            status: response.status,
            headers: response.headers,
            body: response.body,
        }),
        // A guest that answered with the WIT `error` variant: the host turns
        // it into a written refusal, because a browser receiving a partial
        // body hangs and a connection closed with no status is worse.
        Err(WasmError::NotGranted(reason)) => {
            Err(CallError::GuestReason(CapabilityError::NotGranted(reason)))
        }
        Err(WasmError::Failed(reason)) => {
            Err(CallError::GuestReason(CapabilityError::Permission(reason)))
        }
    }
}
