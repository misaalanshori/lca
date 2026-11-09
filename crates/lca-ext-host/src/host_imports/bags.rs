//! The **bags and state** capability domains: `log`, `resources`, `state`, and
//! the `lca:ext/types` instantiation (ADR-0030/0032, FR-EXT-9).

use lca_protocol::CapabilityError;

use crate::HostState;
use crate::lock;

impl lca_ext_abi::host::tool::lca::host::log::Host for HostState {
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

impl lca_ext_abi::host::tool::lca::host::resources::Host for HostState {
    fn list_resources(
        &mut self,
        prefix: String,
    ) -> Result<
        Vec<lca_ext_abi::host::tool::lca::host::resources::ResourceEntry>,
        lca_ext_abi::host::tool::lca::host::resources::Error,
    > {
        self.cap
            .resource_list(&prefix)
            .map(|entries| {
                entries
                    .into_iter()
                    .map(|(path, size)| {
                        lca_ext_abi::host::tool::lca::host::resources::ResourceEntry { path, size }
                    })
                    .collect()
            })
            .map_err(resources_error)
    }

    fn read(
        &mut self,
        path: String,
    ) -> Result<Vec<u8>, lca_ext_abi::host::tool::lca::host::resources::Error> {
        self.cap.resource_read(&path).map_err(resources_error)
    }
}

fn resources_error(err: CapabilityError) -> lca_ext_abi::host::tool::lca::host::resources::Error {
    use lca_ext_abi::host::tool::lca::host::resources::Error as E;
    match err {
        CapabilityError::Permission(detail) | CapabilityError::NotGranted(detail) => {
            E::Permission(detail)
        }
        CapabilityError::NotFound(detail) => E::NotFound(detail),
        CapabilityError::Io(detail)
        | CapabilityError::Invalid(detail)
        | CapabilityError::Timeout(detail) => E::Invalid(detail),
    }
}

impl lca_ext_abi::host::tool::lca::host::state::Host for HostState {
    fn read(&mut self, key: String) -> Option<Vec<u8>> {
        self.cap.state_read(&key).unwrap_or(None)
    }

    fn write(
        &mut self,
        key: String,
        value: Vec<u8>,
    ) -> Result<(), lca_ext_abi::host::tool::lca::host::state::Error> {
        self.cap.state_write(&key, &value).map_err(state_error)
    }

    fn delete(
        &mut self,
        key: String,
    ) -> Result<(), lca_ext_abi::host::tool::lca::host::state::Error> {
        self.cap.state_delete(&key).map_err(state_error)
    }

    fn list_keys(
        &mut self,
    ) -> Result<
        Vec<lca_ext_abi::host::tool::lca::host::state::StateEntry>,
        lca_ext_abi::host::tool::lca::host::state::Error,
    > {
        self.cap
            .state_list()
            .map(|entries| {
                entries
                    .into_iter()
                    .map(
                        |(key, size)| lca_ext_abi::host::tool::lca::host::state::StateEntry {
                            key,
                            size,
                        },
                    )
                    .collect()
            })
            .map_err(state_error)
    }
}

fn state_error(err: CapabilityError) -> lca_ext_abi::host::tool::lca::host::state::Error {
    use lca_ext_abi::host::tool::lca::host::state::Error as E;
    match err {
        CapabilityError::Invalid(detail) => E::Invalid(detail),
        CapabilityError::Permission(detail) | CapabilityError::NotGranted(detail) => {
            E::Permission(detail)
        }
        CapabilityError::NotFound(detail)
        | CapabilityError::Io(detail)
        | CapabilityError::Timeout(detail) => E::Io(detail),
    }
}

impl lca_ext_abi::host::tool::lca::ext::types::Host for HostState {}

impl HostState {
    pub(crate) fn record_log(&mut self, message: String) {
        let truncated = truncate_bytes(&message, self.log_limit);
        lock(&self.logs).push(truncated);
    }
}

pub(crate) fn truncate_bytes(message: &str, limit: usize) -> String {
    const MARKER: &str = "... [truncated]";
    if message.len() <= limit {
        return message.to_string();
    }
    let keep = limit.saturating_sub(MARKER.len());
    let mut cut = keep.min(message.len());
    while cut > 0 && !message.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}{}", &message[..cut], MARKER)
}
