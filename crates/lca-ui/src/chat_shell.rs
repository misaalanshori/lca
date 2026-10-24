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
        // Gh #407: `!i <cmd>` forces the interactive path. The marker is read
        // from the **original** line and removed here, so there is exactly one
        // spelling of it in the code.
        //
        // This is where a live pane caught a bug a unit row could not:
        // `trim_start_matches('!')` strips *every* leading `!`, so `!i vim`
        // arrived as `i vim`, the marker was gone before detection ran, and the
        // command failed with 127 `i: not found`. Strip the prefix the line
        // actually has (`!!` or `!`), and the marker after it.
        // The caller passes the line with its leading `!` already consumed, so
        // `!i vim` arrives here as `i vim`. The marker is therefore looked for
        // **without** the bang; `!i` is also accepted so a direct call (and the
        // regression rows) can use the form a user types.
        //
        // A live pane found this: the first version looked for `!i` in a string
        // that no longer had the `!`, so `forced` was always false and
        // `!i vim` ran as `i vim` - exit 127, `i: not found`.
        let body = line
            .strip_prefix("!!")
            .or_else(|| line.strip_prefix('!'))
            .unwrap_or(line)
            .trim();
        let forced_body = body
            .strip_prefix("!i")
            .or_else(|| body.strip_prefix("i"))
            .filter(|rest| rest.is_empty() || rest.starts_with([' ', '\t']));
        let forced = forced_body.is_some();
        let command = forced_body.map_or(body, str::trim_start);
        if command.is_empty() {
            return Action::Continue;
        }
        // Gh #407: an interactive command cannot run on a pipe. Hand it to the
        // loop, which stops the interface, runs it with inherited stdio, and
        // restarts - `$EDITOR`'s path, not a second one.
        //
        // The card is still opened, so the transcript records that the command
        // ran and in what mode. Its output does not come back through the card
        // (it went to the real terminal); the card finishes with the exit code
        // the loop reports.
        if forced || needs_tty(command) {
            self.transcript
                .start_manual_tool_excluding("bash", command.to_string(), excluded);
            return Action::InteractiveShell(command.to_string());
        }
        let Some(run_shell) = self.world.options.hooks.run_shell.clone() else {
            self.say("shell mode is not available in this host".to_string());
            return Action::Continue;
        };
        // The command card is visible either way; the host records the
        // command for the model's context only when it is not `!!`.
        // The editor's own run, so the card takes pi's `bashMode` title
        // instead of the model card's `toolTitle` (FR-UI-14).
        // Gh #391 gap 3: `!!` withholds its output from the model, so the
        // card draws quiet rather than in `bashMode` - the context boundary
        // is glance-visible instead of being a second `!` to notice.
        self.transcript
            .start_manual_tool_excluding("bash", command.to_string(), excluded);
        // Gh #370 flaw 1: a chunk arriving here must wake the loop. With a waker (the
        // real interface) the wake rides a forwarder thread beside the channel, since
        // the runner's signature hands out a plain `SyncSender` and the wake has to
        // happen *after* each enqueue. Without one (a test, an embedder) the channel
        // is handed over directly, so nothing is asynchronous that was not before.
        let (worker_tx, worker_rx) = std::sync::mpsc::sync_channel(256);
        let rx = match self.world.waker.clone() {
            Some(waker) => {
                let (loop_tx, loop_rx) = std::sync::mpsc::channel();
                std::thread::spawn(move || {
                    while let Ok(event) = worker_rx.recv() {
                        if loop_tx.send(event).is_err() {
                            return;
                        }
                        waker.wake();
                    }
                });
                loop_rx
            }
            None => worker_rx,
        };
        let cancel = run_shell(command, excluded, worker_tx);
        self.shell = Some(ShellRun {
            output: rx,
            cancel,
            command: command.to_string(),
            excluded,
        });
        self.say(if excluded {
            format!("running `{command}` (excluded from context)")
        } else {
            format!("running `{command}`")
        });
        Action::Continue
    }

    /// Close out an interactive `!` run once the terminal is back (gh #407).
    ///
    /// The child's output went to the real terminal, not into the card, so there
    /// is nothing to drain - only the exit code to record and the notice to show.
    /// The card is finished rather than left spinning, which is what the user
    /// would otherwise see after quitting `vim`: a `bash` card that never ends.
    pub fn finish_interactive_shell(&mut self, code: Option<i32>) {
        let status = if code == Some(0) {
            ToolStatus::Ok
        } else {
            ToolStatus::Error
        };
        self.transcript.finish_tool_status(status);
        let code = code.map_or("signalled".to_string(), |c| format!("exit {c}"));
        self.say(format!("ran an interactive command - {code}"));
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
                self.say(format!("ran `{}` - {code}{suffix}", shell.command));
            }
        }
        changed
    }
}

// ---------------------------------------------------------------------------
// Interactive detection (gh #407 pattern 3)
// ---------------------------------------------------------------------------

/// Whether a `!`/`!!` command needs a real terminal.
///
/// The defect this answers: `!vim`, `!git add -p` and `!… | less` ran through
/// `spawn_direct`, which pipes all three stdio for a worker thread to drain. A
/// fullscreen program on a pipe either refuses outright (`vim: output is not to a
/// terminal`) or blocks waiting for keystrokes that never arrive - a **hang**,
/// and the interface is still drawing throughout, so the user cannot tell
/// whether the command is slow or wedged.
///
/// # Why a list, and why it is small
///
/// There is no reliable way to ask a command whether it will be interactive
/// without running it. pi's shape is the one taken here (its
/// `isInteractiveCommand`): a short explicit list, an opt-in, and a pipe-tail
/// check. Each is a heuristic, so the design rule is **detect conservatively**:
///
/// - A false positive runs `git log` fullscreen when the user wanted its output
///   in the transcript - the feature `!` exists for, broken.
/// - A false negative hangs, but [`without_force`]'s `!i` marker is a
///   documented escape, and the notice names it.
///
/// So the list holds programs whose *primary mode is the terminal*, not anything
/// that might use one. `grep vim notes.md` is a search; `vim notes.txt` is an
/// editor. The difference is which word is the program, which is why this parses
/// the head rather than searching the string.
///
/// # What is deliberately not detected
///
/// Named in the report and here, because a heuristic's gaps are part of its
/// contract: a program not on the list that happens to want a TTY (an obscure
/// REPL, a bespoke curses tool), a wrapper this does not unwrap, and anything
/// where the interactive mode is chosen by a flag this does not know. `!i` covers
/// all three.
#[must_use]
pub fn needs_tty(command: &str) -> bool {
    let command = command.trim();
    if command.is_empty() {
        return false;
    }
    // The explicit opt-in wins over everything, including the false-positive
    // cases: a user who typed `!i` has told us what they want.
    if command == "!i" || command.starts_with("!i ") || command.starts_with("!i\t") {
        return true;
    }
    // **With a pipeline, only the tail decides.** `git log | less` ends in a
    // program that owns the screen; `less notes.md | head` ends in `head`, so its
    // output is captured like any other. Checking the head first would get the
    // second case wrong, and that is the false positive the design warns about.
    if command.contains('|') {
        return pipe_tail_needs_tty(command);
    }
    let head = program_head(command);
    let Some(program) = head.first() else {
        return false;
    };
    program_needs_tty(program, &head[1..])
}

/// The command with a leading `!i` marker removed, so the program is what runs.
#[must_use]
pub fn without_force(command: &str) -> &str {
    // Both spellings, because the two callers differ: `needs_tty` is handed the
    // line a user typed (`!i vim`) while `run_shell_mode` receives it with the
    // leading `!` already consumed (`i vim`). Keeping them in one function means
    // they cannot disagree about where the marker ends.
    for marker in ["!i", "i"] {
        if let Some(rest) = command.trim().strip_prefix(marker)
            && (rest.is_empty() || rest.starts_with([' ', '\t']))
        {
            return rest.trim_start();
        }
    }
    command.trim()
}

/// The words before the first unquoted shell operator, with `sudo`-class wrappers
/// and their own arguments skipped.
///
/// Only the head is inspected because that is the only place a program name
/// lives: everything after `|`, `&&`, `;`, `>` or a redirection belongs to
/// another command. Quoting is tracked so `echo "a | b"` does not split.
fn program_head(command: &str) -> Vec<String> {
    let mut words: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut chars = command.chars().peekable();
    while let Some(ch) = chars.next() {
        match quote {
            Some(q) => {
                if ch == q {
                    quote = None;
                } else if ch == '\\' && q == '"' {
                    if let Some(next) = chars.next() {
                        current.push(next);
                    }
                } else {
                    current.push(ch);
                }
            }
            None => match ch {
                '\'' | '"' => quote = Some(ch),
                '\\' => {
                    if let Some(next) = chars.next() {
                        current.push(next);
                    }
                }
                c if c.is_whitespace() => {
                    if !current.is_empty() {
                        words.push(std::mem::take(&mut current));
                    }
                }
                '|' | '&' | ';' | '>' | '<' => break,
                c => current.push(c),
            },
        }
    }
    if !current.is_empty() {
        words.push(current);
    }
    skip_wrappers(words)
}

/// Drop leading `sudo`/`env`/`nohup`-class words and their arguments, so
/// `sudo -u root vim f` still finds `vim`.
///
/// The set is small on purpose. A wrapper with an argument grammar this does not
/// model (a subshell, `xargs`, `find -exec`) is left alone: guessing wrong there
/// is how a detector starts running searches fullscreen.
fn skip_wrappers(mut words: Vec<String>) -> Vec<String> {
    loop {
        let Some(first) = words.first() else {
            return words;
        };
        let program = std::path::Path::new(first.as_str())
            .file_name()
            .map_or_else(|| first.clone(), |name| name.to_string_lossy().into_owned());
        let skip = match program.as_str() {
            // `sudo [-u user] [-g group] ...`: flags, then their values for the
            // two that take one, then the command.
            "sudo" | "doas" => {
                let mut index = 1;
                while index < words.len() {
                    let word = words[index].as_str();
                    if word == "-u" || word == "-g" || word == "-h" || word == "-p" {
                        index += 2; // the flag and its value
                    } else if word.starts_with('-') {
                        index += 1;
                    } else {
                        break;
                    }
                }
                index
            }
            // `env [-i] [NAME=value ...] command`
            "env" => {
                let mut index = 1;
                while index < words.len()
                    && (words[index].starts_with('-') || words[index].contains('='))
                {
                    index += 1;
                }
                index
            }
            "nohup" | "command" | "exec" | "time" | "nice" => 1,
            _ => return words,
        };
        if skip >= words.len() {
            return words;
        }
        words.drain(..skip);
    }
}

/// Whether the `program` word (with its args) is one whose primary mode is the
/// terminal.
fn program_needs_tty(program: &str, args: &[String]) -> bool {
    // `./vim-wrapper` and `/usr/bin/vim` are the same program for this purpose.
    let name = std::path::Path::new(program)
        .file_name()
        .map_or_else(|| program.to_string(), |n| n.to_string_lossy().into_owned());
    let name = name.as_str();

    // Editors: the family a user reaches for as `!vim file`.
    const EDITORS: &[&str] = &[
        "vi", "vim", "nvim", "neovim", "nano", "pico", "emacs", "hx", "helix", "micro", "kak",
        "kakoune", "joe", "jed", "mcedit", "ne", "mg", "ed", "ex", "view", "vimdiff", "nvimdiff",
    ];
    if EDITORS.contains(&name) {
        return true;
    }

    // Fullscreen terminal programs and pagers: they own the screen.
    const FULLSCREEN: &[&str] = &[
        "less",
        "more",
        "most",
        "fzf",
        "htop",
        "btop",
        "atop",
        "top",
        "tmux",
        "screen",
        "tig",
        "lazygit",
        "lazydocker",
        "ncdu",
        "mc",
        "ranger",
        "nnn",
        "vifm",
        "w3m",
        "links",
        "lynx",
        "elinks",
        "watch",
        "dialog",
        "whiptail",
    ];
    if FULLSCREEN.contains(&name) {
        return true;
    }

    // REPLs and database shells: a prompt, not a stream.
    const REPLS: &[&str] = &[
        "python",
        "python2",
        "python3",
        "ipython",
        "node",
        "deno",
        "bun",
        "irb",
        "pry",
        "ghci",
        "lua",
        "luajit",
        "ruby",
        "perl",
        "php",
        "sqlite3",
        "psql",
        "mysql",
        "mongo",
        "mongosh",
        "redis-cli",
        "cqlsh",
        "sqlplus",
        "bash",
        "zsh",
        "fish",
        "sh",
        "dash",
        "ksh",
        "csh",
        "tcsh",
        "nu",
        "elvish",
        "xonsh",
        "pwsh",
        "powershell",
    ];
    if REPLS.contains(&name) {
        return true;
    }

    // `git` subcommands that prompt. Checked by *argument* rather than by string
    // search, so `git log --grep=vim` stays piped.
    if name == "git" {
        return git_needs_tty(args);
    }

    // `docker`/`kubectl`/`podman` only with an explicit TTY flag: the flag exists
    // precisely to attach a terminal, and without it they are ordinary output.
    if matches!(name, "docker" | "podman" | "kubectl" | "oc" | "nerdctl") {
        return args.iter().any(|arg| is_tty_flag(arg));
    }

    false
}

/// `git`'s prompting subcommands.
fn git_needs_tty(args: &[String]) -> bool {
    let Some(subcommand) = args.first().map(String::as_str) else {
        return false;
    };
    let rest = || args.iter().skip(1).map(String::as_str);
    match subcommand {
        // `add -p`/`--patch`/`-i` prompt per hunk or open a menu.
        "add" => rest().any(|arg| {
            arg == "-p"
                || arg == "--patch"
                || arg == "-i"
                || arg == "--interactive"
                // Combined short flags: `-ap`, `-pi`.
                || (arg.starts_with('-') && !arg.starts_with("--") && arg[1..].contains('p'))
                || (arg.starts_with('-') && !arg.starts_with("--") && arg[1..].contains('i'))
        }),
        // `rebase -i`/`--interactive` opens a sequence editor.
        "rebase" => rest().any(|arg| arg == "-i" || arg == "--interactive"),
        // `commit` without `-m`/`-F` opens one.
        "commit" => !args.iter().any(|arg| {
            arg == "-m"
                || arg == "--message"
                || arg == "-F"
                || arg == "--file"
                || arg == "-C"
                || arg == "--reuse-message"
                || arg == "-c"
                || arg == "--no-edit"
                || arg == "--amend" && false
        }),
        // `git mergetool`/`difftool`/`bisect`/`rebase --edit-todo` all prompt.
        "mergetool" | "difftool" => true,
        _ => false,
    }
}

/// A `--tty`-class flag, including combined short forms (`-it`, `-ti`, `-dit`).
fn is_tty_flag(arg: &str) -> bool {
    if arg == "--tty" || arg == "--interactive" {
        return true;
    }
    // Combined short flags: any `i` or `t` in a `-xyz` cluster. Only applied to
    // the docker-family commands, where those letters mean exactly that.
    arg.starts_with('-')
        && !arg.starts_with("--")
        && arg.len() > 1
        && (arg[1..].contains('t') || arg[1..].contains('i'))
}

/// Whether the pipeline's **last** stage is a fullscreen program (pi's
/// `| less|more|fzf|vim` check).
///
/// Only the tail decides, because earlier stages have a later one to write to.
/// `less notes.md | head` is captured output; `git log | less` is a pager.
fn pipe_tail_needs_tty(command: &str) -> bool {
    if !command.contains('|') {
        return false;
    }
    let Some(tail) = command.rsplit('|').next() else {
        return false;
    };
    let words = skip_wrappers(program_head(tail.trim()));
    let Some(program) = words.first() else {
        return false;
    };
    program_needs_tty(program, &words[1..])
}

#[cfg(test)]
mod detection_tests {
    use super::*;

    // Verifies: the `!i` marker survives the prefix strip. A live pane found the
    // bug this pins: `trim_start_matches('!')` ate the `i` too, so the command
    // ran as `i test -t 1` and failed with 127.
    #[test]
    fn the_force_marker_is_recognised_after_the_prefix_is_stripped() {
        assert!(needs_tty("!i vim"));
        assert!(needs_tty("!i test -t 1"));
        assert!(needs_tty("vim"), "and the list still fires on its own");
        assert_eq!(without_force("!i vim"), "vim");
        assert_eq!(without_force("vim"), "vim");
    }
}
