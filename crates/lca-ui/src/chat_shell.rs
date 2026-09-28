//! The `!`/`!!` shell run, split from `chat.rs` (the 1,200-line ceiling).

use super::chat::Chat;
use crate::chat_pickers::ShellRun;
use crate::state::Action;
use crate::transcript::ToolStatus;

impl Chat {
    /// Run a `!`/`!!` shell command and show it as a bash card (FR-UI-14).
    /// The command runs on a worker thread (R4); output streams into the
    /// card and Escape cancels it.
    pub(crate) fn run_shell_mode(&mut self, line: &str) -> Action {
        let excluded = line.starts_with("!!");
        let command = line.trim_start_matches('!').trim();
        if command.is_empty() {
            return Action::Continue;
        }
        let Some(run_shell) = self.world.options.hooks.run_shell.clone() else {
            self.world.notice = Some("shell mode is not available in this host".to_string());
            return Action::Continue;
        };
        // The command card is visible either way; the host records the
        // command for the model's context only when it is not `!!`.
        self.transcript.start_tool("bash", command.to_string());
        let (tx, rx) = std::sync::mpsc::sync_channel(256);
        let cancel = run_shell(command, excluded, tx);
        self.shell = Some(ShellRun {
            output: rx,
            cancel,
            command: command.to_string(),
            excluded,
        });
        self.world.notice = Some(if excluded {
            format!("running `{command}` (excluded from context)")
        } else {
            format!("running `{command}`")
        });
        Action::Continue
    }

    /// Whether a `!`/`!!` command is running (R4).
    pub fn shell_running(&self) -> bool {
        self.shell.is_some()
    }

    /// Drain the running `!`/`!!` command's output into its card (R4).
    /// Returns whether the transcript changed.
    pub fn poll_shell(&mut self) -> bool {
        let Some(shell) = self.shell.as_ref() else {
            return false;
        };
        let mut changed = false;
        let mut done: Option<Option<i32>> = None;
        while let Ok(event) = shell.output.try_recv() {
            match event {
                crate::state::ShellEvent::Chunk(chunk) => {
                    self.transcript.append_tool_output(&chunk);
                    changed = true;
                }
                crate::state::ShellEvent::Done(code) => {
                    done = Some(code);
                    changed = true;
                }
            }
        }
        if let Some(code) = done {
            let status = if code == Some(0) {
                ToolStatus::Ok
            } else {
                ToolStatus::Error
            };
            self.transcript.finish_tool_status(status);
            if let Some(shell) = self.shell.take() {
                let suffix = if shell.excluded {
                    " (excluded from context)"
                } else {
                    ""
                };
                let code = code.map_or("signalled".to_string(), |c| format!("exit {c}"));
                self.world.notice = Some(format!("ran `{}` - {code}{suffix}", shell.command));
            }
        }
        changed
    }
}
