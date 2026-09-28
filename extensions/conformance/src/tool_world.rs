wit_bindgen::generate!({
    path: "../../wit",
    world: "tool",
    export_macro_name: "export_tool",
    with: {
        "lca:host/log@0.4.0": generate,
        "lca:host/fs@0.4.0": generate,
        "lca:host/process@0.4.0": generate,
        "lca:host/pty@0.4.0": generate,
        "lca:host/resources@0.4.0": generate,
        "lca:host/state@0.4.0": generate,
    },
});

use lca::ext::types::ToolCall;
use lca::host::{fs, process, pty, resources, state};

use crate::{Cap, ModeOutcome, mode_and_args, run_shared, schema_json};
use exports::lca::ext::execute::Guest as ExecuteTrait;
use exports::lca::ext::execute::ToolResult as WasmResult;
use exports::lca::ext::tool_schema::{Guest as SchemaGuest, Schema};
use lca::host::log;

fn map_fs(err: fs::Error) -> crate::CapabilityError {
    use crate::CapabilityError as E;
    match err {
        fs::Error::Permission(d) => E::Permission(d),
        fs::Error::NotGranted(d) => E::NotGranted(d),
        fs::Error::NotFound(d) => E::NotFound(d),
        fs::Error::Io(d) => E::Io(d),
        fs::Error::Invalid(d) => E::Invalid(d),
    }
}

fn map_process(err: process::Error) -> crate::CapabilityError {
    use crate::CapabilityError as E;
    match err {
        process::Error::Permission(d) => E::Permission(d),
        process::Error::NotGranted(d) => E::NotGranted(d),
        process::Error::NotFound(d) => E::NotFound(d),
        process::Error::Io(d) => E::Io(d),
        process::Error::Invalid(d) => E::Invalid(d),
    }
}

fn map_pty(err: pty::Error) -> crate::CapabilityError {
    use crate::CapabilityError as E;
    match err {
        pty::Error::Permission(d) => E::Permission(d),
        pty::Error::NotGranted(d) => E::NotGranted(d),
        pty::Error::NotFound(d) => E::NotFound(d),
        pty::Error::Io(d) => E::Io(d),
        pty::Error::Invalid(d) => E::Invalid(d),
    }
}

fn map_resources(err: resources::Error) -> crate::CapabilityError {
    use crate::CapabilityError as E;
    match err {
        resources::Error::Permission(d) => E::Permission(d),
        resources::Error::NotFound(d) => E::NotFound(d),
        resources::Error::Invalid(d) => E::Invalid(d),
    }
}

fn map_state(err: state::Error) -> crate::CapabilityError {
    use crate::CapabilityError as E;
    match err {
        state::Error::Permission(d) => E::Permission(d),
        state::Error::Invalid(d) => E::Invalid(d),
        state::Error::Io(d) => E::Io(d),
    }
}

/// The guest's capability view: host imports behind every call.
struct GuestCap;

impl Cap for GuestCap {
    fn fs_read(&self, scope: &str, path: &str) -> Result<Vec<u8>, crate::CapabilityError> {
        fs::read(scope, path).map_err(map_fs)
    }
    fn fs_write(
        &self,
        scope: &str,
        path: &str,
        bytes: &[u8],
    ) -> Result<(), crate::CapabilityError> {
        fs::write(scope, path, bytes).map_err(map_fs)
    }
    fn fs_stat(&self, scope: &str, path: &str) -> Result<(bool, u64), crate::CapabilityError> {
        let info = fs::stat(scope, path).map_err(map_fs)?;
        Ok((info.is_dir, info.len))
    }
    fn fs_list(&self, scope: &str, path: &str) -> Result<Vec<String>, crate::CapabilityError> {
        fs::list_entries(scope, path).map_err(map_fs)
    }
    fn resource_list(&self, prefix: &str) -> Result<Vec<(String, u64)>, crate::CapabilityError> {
        resources::list_resources(prefix)
            .map(|entries| {
                entries
                    .into_iter()
                    .map(|entry| (entry.path, entry.size))
                    .collect()
            })
            .map_err(map_resources)
    }
    fn resource_read(&self, path: &str) -> Result<Vec<u8>, crate::CapabilityError> {
        resources::read(path).map_err(map_resources)
    }
    fn state_read(&self, key: &str) -> Result<Option<Vec<u8>>, crate::CapabilityError> {
        Ok(state::read(key))
    }
    fn state_write(&self, key: &str, value: &[u8]) -> Result<(), crate::CapabilityError> {
        state::write(key, value).map_err(map_state)
    }
    fn state_delete(&self, key: &str) -> Result<(), crate::CapabilityError> {
        state::delete(key).map_err(map_state)
    }
    fn state_list(&self) -> Result<Vec<(String, u64)>, crate::CapabilityError> {
        state::list_keys()
            .map(|entries| {
                entries
                    .into_iter()
                    .map(|entry| (entry.key, entry.size))
                    .collect()
            })
            .map_err(map_state)
    }
    fn process_spawn(
        &self,
        program: &str,
        args: &[String],
        cwd: &str,
    ) -> Result<u32, crate::CapabilityError> {
        process::spawn(program, args, cwd).map_err(map_process)
    }
    fn process_read_stdout(
        &self,
        handle: u32,
        max: usize,
    ) -> Result<Option<Vec<u8>>, crate::CapabilityError> {
        process::read_stdout(handle, max as u64).map_err(map_process)
    }
    fn process_read_stderr(
        &self,
        handle: u32,
        max: usize,
    ) -> Result<Option<Vec<u8>>, crate::CapabilityError> {
        process::read_stderr(handle, max as u64).map_err(map_process)
    }
    fn process_write_stdin(
        &self,
        handle: u32,
        bytes: &[u8],
    ) -> Result<u64, crate::CapabilityError> {
        process::write_stdin(handle, bytes).map_err(map_process)
    }
    fn process_wait(&self, handle: u32) -> Result<i32, crate::CapabilityError> {
        process::wait(handle).map_err(map_process)
    }
    fn process_kill(&self, handle: u32) -> Result<(), crate::CapabilityError> {
        process::kill(handle).map_err(map_process)
    }
    fn pty_spawn(
        &self,
        program: &str,
        args: &[String],
        cwd: &str,
        rows: u16,
        cols: u16,
    ) -> Result<u32, crate::CapabilityError> {
        pty::spawn(program, args, cwd, rows, cols).map_err(map_pty)
    }
    fn pty_read(&self, handle: u32, max: usize) -> Result<Option<Vec<u8>>, crate::CapabilityError> {
        pty::read(handle, max as u64).map_err(map_pty)
    }
    fn pty_write(&self, handle: u32, bytes: &[u8]) -> Result<u64, crate::CapabilityError> {
        pty::write(handle, bytes).map_err(map_pty)
    }
    fn pty_resize(&self, handle: u32, rows: u16, cols: u16) -> Result<(), crate::CapabilityError> {
        pty::resize(handle, rows, cols).map_err(map_pty)
    }
    fn pty_wait(&self, handle: u32) -> Result<i32, crate::CapabilityError> {
        pty::wait(handle).map_err(map_pty)
    }
    fn pty_kill(&self, handle: u32) -> Result<(), crate::CapabilityError> {
        pty::kill(handle).map_err(map_pty)
    }
}

pub struct ToolComponent;

impl SchemaGuest for ToolComponent {
    fn get_schema() -> Schema {
        let (name, description, parameters) = schema_json();
        Schema {
            name,
            description,
            parameters,
            extras: Vec::new(),
        }
    }
}

impl ExecuteTrait for ToolComponent {
    fn run(call: ToolCall) -> WasmResult {
        let (mode, args) = mode_and_args(&call.arguments);
        let outcome = match mode.as_str() {
            "trap" => panic!("conformance trap requested"),
            "loop" => loop {
                std::hint::spin_loop();
            },
            "log" => {
                log::info(&"x".repeat(50_000));
                ModeOutcome {
                    ok: true,
                    text: "logged".to_string(),
                }
            }
            "alloc" => {
                let mut hog: Vec<Vec<u8>> = Vec::new();
                for i in 0..64u64 {
                    let mut block = vec![0u8; 4 * 1024 * 1024];
                    block[0] = i as u8;
                    hog.push(block);
                }
                ModeOutcome {
                    ok: true,
                    text: "allocated".to_string(),
                }
            }
            _ => run_shared(&GuestCap, &mode, &args),
        };
        WasmResult {
            call_id: call.call_id,
            status: if outcome.ok { "ok" } else { "error" }.to_string(),
            content: Some(outcome.text),
            truncated: false,
            extras: Vec::new(),
        }
    }
}

export_tool!(ToolComponent);
