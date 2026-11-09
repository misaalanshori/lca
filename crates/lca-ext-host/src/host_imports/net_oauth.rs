//! The **network and identity** domains: `net` (one body instantiated per
//! world, gh #302), `oauth`, `credentials`, and the `log` the provider world
//! imports (FR-PERM-6/7, ADR-0012).

use lca_protocol::CapabilityError;

use crate::HostState;

use lca_ext_abi::host::provider::lca::host as provider_host;

fn net_error(err: CapabilityError) -> provider_host::net::Error {
    use provider_host::net::Error as E;
    match err {
        CapabilityError::Permission(detail) => E::Permission(detail),
        CapabilityError::NotGranted(detail) => E::NotGranted(detail),
        CapabilityError::NotFound(detail) => E::Invalid(detail),
        CapabilityError::Io(detail) => E::Io(detail),
        CapabilityError::Invalid(detail) => E::Invalid(detail),
        CapabilityError::Timeout(detail) => E::Io(format!("timed out: {detail}")),
    }
}

/// The tool world's `net` error type carries the same variants as the
/// provider world's (both come from `wit/deps/lca-host`), so one mapping
/// is derived from the other rather than restated - a variant added to the
/// WIT then breaks exactly one match, not two.
fn net_error_tool(err: CapabilityError) -> tool_host_net::Error {
    use tool_host_net::Error as E;
    match net_error(err) {
        provider_host_net::Error::Permission(detail) => E::Permission(detail),
        provider_host_net::Error::NotGranted(detail) => E::NotGranted(detail),
        provider_host_net::Error::Dns(detail) => E::Dns(detail),
        provider_host_net::Error::Tls(detail) => E::Tls(detail),
        provider_host_net::Error::Io(detail) => E::Io(detail),
        provider_host_net::Error::Invalid(detail) => E::Invalid(detail),
    }
}

fn oauth_error(err: CapabilityError) -> provider_host::oauth::Error {
    use provider_host::oauth::Error as E;
    match err {
        CapabilityError::Permission(detail) => E::Permission(detail),
        CapabilityError::NotGranted(detail) => E::NotGranted(detail),
        CapabilityError::NotFound(_) => E::Invalid("not found".to_string()),
        CapabilityError::Io(detail) => E::Io(detail),
        CapabilityError::Invalid(detail) => E::Invalid(detail),
        CapabilityError::Timeout(detail) => E::Timeout(detail),
    }
}

fn credentials_error(err: CapabilityError) -> provider_host::credentials::Error {
    use provider_host::credentials::Error as E;
    match err {
        CapabilityError::Permission(detail) => E::Permission(detail),
        CapabilityError::NotGranted(detail) => E::NotGranted(detail),
        CapabilityError::NotFound(_) | CapabilityError::Timeout(_) => {
            E::Io("unavailable".to_string())
        }
        CapabilityError::Io(detail) => E::Io(detail),
        CapabilityError::Invalid(detail) => E::Invalid(detail),
    }
}

impl provider_host::log::Host for HostState {
    fn trace(&mut self, message: String) {
        self.record_log(message);
    }
    fn debug(&mut self, message: String) {
        self.record_log(message);
    }
    fn info(&mut self, message: String) {
        self.record_log(message);
    }
    fn warn(&mut self, message: String) {
        self.record_log(message);
    }
    fn error(&mut self, message: String) {
        self.record_log(message);
    }
}

impl lca_ext_abi::host::provider::lca::ext::types::Host for HostState {}

use lca_ext_abi::host::tool::lca::host::net as tool_host_net;
use provider_host::net as provider_host_net;

/// The `net` import's host half, spelled once and instantiated per world
/// that imports it (gh #302 added the tool world to the provider world).
///
/// One body, not two copies: the consent rule, the debug events and the
/// error mapping must not be able to drift between worlds, and the only
/// per-world difference is the generated error type.
macro_rules! net_host_for {
    ($world:ident, $map:path) => {
        impl $world::Host for HostState {
            fn request(
                &mut self,
                method: String,
                url: String,
                headers: Vec<(String, String)>,
                body: Option<Vec<u8>>,
            ) -> Result<u32, $world::Error> {
                // Gh #258: method plus redacted URL (never headers or body).
                lca_debug::wasm::event(
                    "import.net.request",
                    &format!("{} {}", method, lca_debug::redact::redact_url(&url)),
                );
                let started = std::time::Instant::now();
                let refs: Vec<(&str, &str)> = headers
                    .iter()
                    .map(|(key, value)| (key.as_str(), value.as_str()))
                    .collect();
                // Gh #302: a request from the single-tool world is a tool
                // call the model (or the user) deliberately made, so an
                // ungranted host asks for the ad hoc grant - the same rule
                // webfetch's native path has used since the consent half
                // landed. Every other world keeps the plain deny: a
                // provider's discovery or login probe must never pop a
                // modal nobody triggered. The deny stays the enforcement
                // backstop either way.
                let outcome = if self.tool_world_call {
                    self.cap
                        .net_request_asking(&method, &url, &refs, body.as_deref())
                } else {
                    self.cap.net_request(&method, &url, &refs, body.as_deref())
                }
                .map_err($map);
                lca_debug::wasm::event(
                    "import.net.done",
                    &format!(
                        "ms={} ok={}",
                        started.elapsed().as_millis(),
                        outcome.is_ok()
                    ),
                );
                outcome
            }

            fn response_status(&mut self, handle: u32) -> Result<u16, $world::Error> {
                // Gh #258: handle plus status (bodies stay behind their own read).
                let status = self.cap.net_response_status(handle).map_err($map);
                if let Ok(code) = status {
                    lca_debug::wasm::event(
                        "import.net.status",
                        &format!("handle={handle} status={code}"),
                    );
                }
                status
            }

            fn response_headers(
                &mut self,
                handle: u32,
            ) -> Result<Vec<(String, String)>, $world::Error> {
                self.cap.net_response_headers(handle).map_err($map)
            }

            fn read_body(
                &mut self,
                handle: u32,
                max: u64,
            ) -> Result<Option<Vec<u8>>, $world::Error> {
                self.cap.net_read_body(handle, max as usize).map_err($map)
            }

            fn close_response(&mut self, handle: u32) -> Result<(), $world::Error> {
                self.cap.net_close_response(handle).map_err($map)
            }
        }
    };
}

net_host_for!(provider_host_net, net_error);
net_host_for!(tool_host_net, net_error_tool);

impl provider_host::oauth::Host for HostState {
    fn begin(
        &mut self,
        redirect_path: String,
    ) -> Result<(String, u32), provider_host::oauth::Error> {
        self.cap.oauth_begin(&redirect_path).map_err(oauth_error)
    }

    fn open(&mut self, url: String) -> Result<(), provider_host::oauth::Error> {
        self.cap.oauth_open(&url).map_err(oauth_error)
    }

    fn await_callback(
        &mut self,
        handle: u32,
    ) -> Result<Vec<(String, String)>, provider_host::oauth::Error> {
        self.cap.oauth_await(handle).map_err(oauth_error)
    }

    fn end_flow(&mut self, handle: u32) -> Result<(), provider_host::oauth::Error> {
        self.cap.oauth_end(handle).map_err(oauth_error)
    }
}

impl provider_host::credentials::Host for HostState {
    fn get(&mut self, key: String) -> Option<String> {
        // Denial reads as absence (capability catalog): checking for an
        // existing login needs no denial/absence distinction.
        let value = self.cap.credentials_get(&key).unwrap_or(None);
        // Gh #258: the key name plus a prefix (never the secret).
        lca_debug::wasm::event(
            "import.credentials.get",
            &lca_debug::redact::redact_credential(&key, value.as_deref()),
        );
        value
    }

    fn set(&mut self, key: String, value: String) -> Result<(), provider_host::credentials::Error> {
        // Gh #258: the key name only (never the value).
        lca_debug::wasm::event("import.credentials.set", &format!("key={key}"));
        self.cap
            .credentials_set(&key, &value)
            .map_err(credentials_error)
    }

    fn delete(&mut self, key: String) -> Result<(), provider_host::credentials::Error> {
        lca_debug::wasm::event("import.credentials.delete", &format!("key={key}"));
        self.cap.credentials_delete(&key).map_err(credentials_error)
    }
}
