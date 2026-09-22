//! The capability engine shared by both delivery modes: the WASM host's
//! import functions and a native-linked extension call the exact same
//! code, which is what makes conformance results identical by
//! construction (ADR-0013, capability catalog).
//!
//! Every refusal records a [`Denial`] so `lca ext info` can show what an
//! extension attempted (FR-EXT-9), and every user-facing decision runs
//! through the same prompt and grant store the model's own commands use
//! (`docs/flows.md`).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use lca_permissions::{
    Action, GrantStore, PermissionPrompt, Proposals, ScopeGrant, ScopeRoots, authorize,
};
use lca_protocol::CapabilityError;

/// One recorded attempt: identity, what was tried, and why it failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Denial {
    /// The capability family: `fs`, `process`, or `pty`.
    pub capability: String,
    /// The parameter the guest supplied.
    pub parameter: String,
    /// Why it was refused.
    pub reason: String,
}

/// What one extension's manifest declared (FR-PERM-1), resolved into the
/// grants the engine enforces. Phase5's install flow intersects this with
/// user approval; until then the declared set is the granted set.
#[derive(Debug, Clone, Default)]
pub struct CapabilityGrants {
    /// Approved `fs` scopes and modes.
    pub fs: Vec<ScopeGrant>,
    /// The `fs` capability was declared at all (FR-PERM-1); an empty
    /// declared set is rejected at manifest parse, so this is exactly
    /// "the manifest declared fs".
    pub fs_declared: bool,
    /// The `process` capability was declared.
    pub process: bool,
    /// The `pty` capability was declared.
    pub pty: bool,
}

enum HandleEntry {
    Process {
        child: crate::process::TreeChild,
        stdout: Option<std::process::ChildStdout>,
        stderr: Option<std::process::ChildStderr>,
        stdin: Option<std::process::ChildStdin>,
    },
    Pty(crate::pty::PtyChild),
}

#[derive(Default)]
struct HandleTable {
    next: u32,
    entries: HashMap<u32, HandleEntry>,
}

/// The engine: one per loaded extension.
pub struct Capabilities {
    name: String,
    grants: CapabilityGrants,
    roots: ScopeRoots,
    prompt: Arc<Mutex<dyn PermissionPrompt>>,
    store: Arc<Mutex<GrantStore>>,
    project: PathBuf,
    proposals: Option<Proposals>,
    denials: Arc<Mutex<Vec<Denial>>>,
    handles: Arc<Mutex<HandleTable>>,
}

impl Capabilities {
    /// Build the engine for one extension. `roots.private` is the base
    /// directory; each extension gets its own subdirectory inside it
    /// (capability catalog: `private` is per-extension).
    pub fn new(
        name: impl Into<String>,
        grants: CapabilityGrants,
        roots: ScopeRoots,
        prompt: Arc<Mutex<dyn PermissionPrompt>>,
        store: Arc<Mutex<GrantStore>>,
        project: PathBuf,
        proposals: Option<Proposals>,
    ) -> Capabilities {
        let name = name.into();
        let mut roots = roots;
        roots.private = roots.private.join(&name);
        Capabilities {
            name,
            grants,
            roots,
            prompt,
            store,
            project,
            proposals,
            denials: Arc::new(Mutex::new(Vec::new())),
            handles: Arc::new(Mutex::new(HandleTable::default())),
        }
    }

    /// The extension's identity.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Every recorded refusal (FR-EXT-9's data).
    pub fn denials(&self) -> Vec<Denial> {
        self.denials.lock().expect("denial lock").clone()
    }

    /// How many attempts were refused (FR-EXT-9).
    pub fn denial_count(&self) -> usize {
        self.denials.lock().expect("denial lock").len()
    }

    fn record(&self, capability: &str, parameter: &str, reason: &str) {
        self.denials.lock().expect("denial lock").push(Denial {
            capability: capability.to_string(),
            parameter: parameter.to_string(),
            reason: reason.to_string(),
        });
    }

    fn undeclared(&self, capability: &str) -> CapabilityError {
        let err = CapabilityError::NotGranted(format!(
            "the manifest does not declare the {capability} capability"
        ));
        self.record(capability, capability, &err.to_string());
        err
    }

    fn refused(&self, capability: &str, parameter: &str, err: CapabilityError) -> CapabilityError {
        self.record(capability, parameter, &err.to_string());
        err
    }

    fn resolve(&self, scope: &str, path: &str, write: bool) -> Result<PathBuf, CapabilityError> {
        self.roots
            .resolve(&self.grants.fs, scope, path, write)
            .map_err(|violation| {
                let err = CapabilityError::from(violation.clone());
                self.record("fs", &format!("{scope}:{path}"), &violation.to_string());
                err
            })
    }

    // ------------------------------------------------------------------
    // fs
    // ------------------------------------------------------------------

    /// Read a file inside a granted scope.
    pub fn fs_read(&self, scope: &str, path: &str) -> Result<Vec<u8>, CapabilityError> {
        if !self.grants.fs_declared {
            return Err(self.undeclared("fs"));
        }
        let resolved = self.resolve(scope, path, false)?;
        Ok(std::fs::read(resolved)?)
    }

    /// Write a file inside a granted scope, creating its parent.
    pub fn fs_write(&self, scope: &str, path: &str, bytes: &[u8]) -> Result<(), CapabilityError> {
        if !self.grants.fs_declared {
            return Err(self.undeclared("fs"));
        }
        let resolved = self.resolve(scope, path, true)?;
        if let Some(parent) = resolved.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(resolved, bytes)?;
        Ok(())
    }

    /// List a directory inside a granted scope; sorted for determinism.
    pub fn fs_list(&self, scope: &str, path: &str) -> Result<Vec<String>, CapabilityError> {
        if !self.grants.fs_declared {
            return Err(self.undeclared("fs"));
        }
        let resolved = self.resolve(scope, path, false)?;
        let mut names: Vec<String> = std::fs::read_dir(resolved)?
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        Ok(names)
    }

    /// Stat a path inside a granted scope.
    pub fn fs_stat(&self, scope: &str, path: &str) -> Result<(bool, u64), CapabilityError> {
        if !self.grants.fs_declared {
            return Err(self.undeclared("fs"));
        }
        let resolved = self.resolve(scope, path, false)?;
        let meta = std::fs::metadata(resolved)?;
        Ok((meta.is_dir(), meta.len()))
    }

    /// Resolve a granted scope's own directory: the working directory for
    /// spawned programs must be a scope the extension can already see
    /// (capability catalog).
    fn scope_dir(&self, scope: &str) -> Result<PathBuf, CapabilityError> {
        if !self.grants.fs_declared {
            return Err(self.undeclared("fs"));
        }
        self.resolve(scope, ".", false)
    }

    // ------------------------------------------------------------------
    // process
    // ------------------------------------------------------------------

    /// Spawn a program (argv, no shell) in a granted scope, through the
    /// same prompt and grant store the model's commands use.
    pub fn process_spawn(
        &self,
        program: &str,
        args: &[String],
        cwd_scope: &str,
    ) -> Result<u32, CapabilityError> {
        if !self.grants.process {
            return Err(self.undeclared("process"));
        }
        let dir = self.scope_dir(cwd_scope).inspect_err(|err| {
            self.record(
                "process",
                &format!("{program} (in {cwd_scope})"),
                &err.to_string(),
            );
        })?;
        let display = format!("{program} {}", args.join(" "));
        let action = Action::Shell {
            command: display.clone(),
            cwd: dir.clone(),
        };
        let decision = {
            let mut store = self.store.lock().expect("grant store lock");
            let mut prompt = self.prompt.lock().expect("prompt lock");
            authorize(
                &mut store,
                &self.project,
                &action,
                self.proposals.as_ref(),
                &mut *prompt,
            )
        };
        match decision {
            Ok(outcome) if outcome.allowed => {}
            Ok(_) => {
                return Err(self.refused(
                    "process",
                    &display,
                    CapabilityError::Permission(format!("the user declined {display}")),
                ));
            }
            Err(err) => {
                return Err(self.refused(
                    "process",
                    &display,
                    CapabilityError::Io(err.to_string()),
                ));
            }
        }
        let child = crate::process::spawn_direct(program, args, &dir).map_err(|err| {
            let err = CapabilityError::from(err);
            self.record("process", &display, &err.to_string());
            err
        })?;
        let mut table = self.handles.lock().expect("handle lock");
        let id = table.next;
        table.next += 1;
        table.entries.insert(
            id,
            HandleEntry::Process {
                child,
                stdout: None,
                stderr: None,
                stdin: None,
            },
        );
        Ok(id)
    }

    /// Read up to `max` bytes of the child's stdout; `None` at EOF.
    pub fn process_read_stdout(
        &self,
        handle: u32,
        max: usize,
    ) -> Result<Option<Vec<u8>>, CapabilityError> {
        let mut table = self.handles.lock().expect("handle lock");
        let entry = table
            .entries
            .get_mut(&handle)
            .ok_or_else(|| CapabilityError::NotFound(format!("unknown handle {handle}")))?;
        let HandleEntry::Process { child, stdout, .. } = entry else {
            return Err(CapabilityError::Invalid(format!(
                "handle {handle} is not a process"
            )));
        };
        let reader =
            stdout.get_or_insert_with(|| child.stdout().expect("stdout was piped at spawn"));
        Ok(crate::process::read_up_to(reader, max)?)
    }

    /// Read up to `max` bytes of the child's stderr; `None` at EOF.
    pub fn process_read_stderr(
        &self,
        handle: u32,
        max: usize,
    ) -> Result<Option<Vec<u8>>, CapabilityError> {
        let mut table = self.handles.lock().expect("handle lock");
        let entry = table
            .entries
            .get_mut(&handle)
            .ok_or_else(|| CapabilityError::NotFound(format!("unknown handle {handle}")))?;
        let HandleEntry::Process { child, stderr, .. } = entry else {
            return Err(CapabilityError::Invalid(format!(
                "handle {handle} is not a process"
            )));
        };
        let reader =
            stderr.get_or_insert_with(|| child.stderr().expect("stderr was piped at spawn"));
        Ok(crate::process::read_up_to(reader, max)?)
    }

    /// Write to the child's stdin.
    pub fn process_write_stdin(&self, handle: u32, bytes: &[u8]) -> Result<u64, CapabilityError> {
        let mut table = self.handles.lock().expect("handle lock");
        let entry = table
            .entries
            .get_mut(&handle)
            .ok_or_else(|| CapabilityError::NotFound(format!("unknown handle {handle}")))?;
        let HandleEntry::Process { child, stdin, .. } = entry else {
            return Err(CapabilityError::Invalid(format!(
                "handle {handle} is not a process"
            )));
        };
        let writer = stdin.get_or_insert_with(|| child.stdin().expect("stdin was piped at spawn"));
        Ok(crate::process::write_all(writer, bytes)?)
    }

    /// Wait for the child; returns its exit code.
    pub fn process_wait(&self, handle: u32) -> Result<i32, CapabilityError> {
        let mut table = self.handles.lock().expect("handle lock");
        let entry = table
            .entries
            .get_mut(&handle)
            .ok_or_else(|| CapabilityError::NotFound(format!("unknown handle {handle}")))?;
        let HandleEntry::Process { child, .. } = entry else {
            return Err(CapabilityError::Invalid(format!(
                "handle {handle} is not a process"
            )));
        };
        Ok(child.wait()?)
    }

    /// Kill the child's whole tree and release the handle.
    pub fn process_kill(&self, handle: u32) -> Result<(), CapabilityError> {
        let mut table = self.handles.lock().expect("handle lock");
        match table.entries.remove(&handle) {
            Some(HandleEntry::Process { mut child, .. }) => {
                child.kill_tree();
                Ok(())
            }
            Some(_) => Err(CapabilityError::Invalid(format!(
                "handle {handle} is not a process"
            ))),
            None => Err(CapabilityError::NotFound(format!(
                "unknown handle {handle}"
            ))),
        }
    }

    // ------------------------------------------------------------------
    // pty
    // ------------------------------------------------------------------

    /// Spawn a program attached to a new pseudo-terminal (ADR-0016).
    pub fn pty_spawn(
        &self,
        program: &str,
        args: &[String],
        cwd_scope: &str,
        rows: u16,
        cols: u16,
    ) -> Result<u32, CapabilityError> {
        if !self.grants.pty {
            return Err(self.undeclared("pty"));
        }
        if rows == 0 || cols == 0 {
            return Err(CapabilityError::Invalid(
                "terminal dimensions must be non-zero".to_string(),
            ));
        }
        let dir = self.scope_dir(cwd_scope).inspect_err(|err| {
            self.record(
                "pty",
                &format!("{program} (in {cwd_scope})"),
                &err.to_string(),
            );
        })?;
        let display = format!("{program} {}", args.join(" "));
        let action = Action::Shell {
            command: display.clone(),
            cwd: dir.clone(),
        };
        let decision = {
            let mut store = self.store.lock().expect("grant store lock");
            let mut prompt = self.prompt.lock().expect("prompt lock");
            authorize(
                &mut store,
                &self.project,
                &action,
                self.proposals.as_ref(),
                &mut *prompt,
            )
        };
        match decision {
            Ok(outcome) if outcome.allowed => {}
            Ok(_) => {
                return Err(self.refused(
                    "pty",
                    &display,
                    CapabilityError::Permission(format!("the user declined {display}")),
                ));
            }
            Err(err) => {
                return Err(self.refused("pty", &display, CapabilityError::Io(err.to_string())));
            }
        }
        let child =
            crate::pty::PtyChild::spawn(program, args, &dir, rows, cols).map_err(|err| {
                let err = CapabilityError::from(err);
                self.record("pty", &display, &err.to_string());
                err
            })?;
        let mut table = self.handles.lock().expect("handle lock");
        let id = table.next;
        table.next += 1;
        table.entries.insert(id, HandleEntry::Pty(child));
        Ok(id)
    }

    /// Read up to `max` terminal bytes; `None` when the session ended.
    pub fn pty_read(&self, handle: u32, max: usize) -> Result<Option<Vec<u8>>, CapabilityError> {
        let mut table = self.handles.lock().expect("handle lock");
        let entry = table
            .entries
            .get_mut(&handle)
            .ok_or_else(|| CapabilityError::NotFound(format!("unknown handle {handle}")))?;
        let HandleEntry::Pty(pty) = entry else {
            return Err(CapabilityError::Invalid(format!(
                "handle {handle} is not a pty"
            )));
        };
        Ok(pty.read(max)?)
    }

    /// Forward keystrokes to the program.
    pub fn pty_write(&self, handle: u32, bytes: &[u8]) -> Result<u64, CapabilityError> {
        let mut table = self.handles.lock().expect("handle lock");
        let entry = table
            .entries
            .get_mut(&handle)
            .ok_or_else(|| CapabilityError::NotFound(format!("unknown handle {handle}")))?;
        let HandleEntry::Pty(pty) = entry else {
            return Err(CapabilityError::Invalid(format!(
                "handle {handle} is not a pty"
            )));
        };
        Ok(pty.write(bytes)?)
    }

    /// Resize the terminal.
    pub fn pty_resize(&self, handle: u32, rows: u16, cols: u16) -> Result<(), CapabilityError> {
        let mut table = self.handles.lock().expect("handle lock");
        let entry = table
            .entries
            .get_mut(&handle)
            .ok_or_else(|| CapabilityError::NotFound(format!("unknown handle {handle}")))?;
        let HandleEntry::Pty(pty) = entry else {
            return Err(CapabilityError::Invalid(format!(
                "handle {handle} is not a pty"
            )));
        };
        Ok(pty.resize(rows, cols)?)
    }

    /// Wait for the program; returns its exit code.
    pub fn pty_wait(&self, handle: u32) -> Result<i32, CapabilityError> {
        let mut table = self.handles.lock().expect("handle lock");
        let entry = table
            .entries
            .get_mut(&handle)
            .ok_or_else(|| CapabilityError::NotFound(format!("unknown handle {handle}")))?;
        let HandleEntry::Pty(pty) = entry else {
            return Err(CapabilityError::Invalid(format!(
                "handle {handle} is not a pty"
            )));
        };
        Ok(pty.wait()?)
    }

    /// End the session and release the handle.
    pub fn pty_kill(&self, handle: u32) -> Result<(), CapabilityError> {
        let mut table = self.handles.lock().expect("handle lock");
        match table.entries.remove(&handle) {
            Some(HandleEntry::Pty(mut pty)) => {
                pty.kill();
                Ok(())
            }
            Some(_) => Err(CapabilityError::Invalid(format!(
                "handle {handle} is not a pty"
            ))),
            None => Err(CapabilityError::NotFound(format!(
                "unknown handle {handle}"
            ))),
        }
    }
}
