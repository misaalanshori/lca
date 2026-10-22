//! The `serve-http` world's conformance guest (gh #363, ADR-0049).
//!
//! The obligation is the same one every capability carries (NFR-25): the
//! granted answer, the denied answer, and a parameter mismatch must agree
//! between the WASM component and the native twin. This world inverts the
//! direction — the host calls in — so the "answer" here is a pure function of
//! the stripped request, and both delivery modes run the same
//! [`crate::serve_http_answer`] so they cannot disagree.
//!
//! What this proves, and what it deliberately cannot:
//!
//! - **Proves** the prefix stripping contract end to end: the handler asserts
//!   its own path begins with `/` and echoes it, so a host that handed the
//!   mounted path through would produce a different answer in one mode.
//! - **Cannot prove** the auth token, because the guest never sees it (the host
//!   consumes it before the call). That is asserted host-side, in
//!   `tests/regressions/363-inbound-http-capability.rs`, against a real socket.

wit_bindgen::generate!({
    path: "../../wit",
    world: "serve-http",
    export_macro_name: "export_serve_http",
    with: {
        "lca:host/log@0.6.0": generate,
    },
});

use exports::lca::ext::handler::{Error as WasmError, Guest as HandlerGuest, Request, Response};

pub struct ServeHttpComponent;

impl HandlerGuest for ServeHttpComponent {
    fn handle(base_url: String, request: Request) -> Result<Response, WasmError> {
        let outcome = crate::serve_http_answer(
            &base_url,
            &crate::ServeHttpProbe {
                method: request.method,
                path: request.path,
                query: request.query,
                headers: request.headers,
                body: request.body,
            },
        );

        match outcome {
            crate::ServeHttpOutcome::Answer {
                status,
                headers,
                body,
            } => Ok(Response {
                status,
                headers,
                body,
            }),
            crate::ServeHttpOutcome::NotGranted(reason) => Err(WasmError::NotGranted(reason)),
            crate::ServeHttpOutcome::Failed(reason) => Err(WasmError::Failed(reason)),
        }
    }
}

export_serve_http!(ServeHttpComponent);
