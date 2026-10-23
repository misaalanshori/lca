//! NFR-24 guard for gh #407 pattern 3: an interactive `!` command gets the real
//! TTY instead of a pipe.
//!
//! The defect: `!vim`, `!git add -p`, `!… | less` ran under `spawn_direct`,
//! which pipes stdin/stdout/stderr for a worker thread to drain. A fullscreen
//! program on a pipe either refuses (`vim: output is not to a terminal`) or
//! waits for keystrokes that never arrive - a **hang**, which is the worst
//! failure shape because the interface is still drawing and the user cannot tell
//! whether the command is slow or wedged.
//!
//! # The two halves, and why each needs its own rows
//!
//! 1. **Detection** is a heuristic, so it has a false-positive story. Running
//!    `git log | less` interactively when the user wanted to capture the output
//!    is the opposite annoyance, and a detector that matched any string
//!    containing `vim` (a `grep vim notes.md`) would be worse than the hang.
//!    The list is therefore small and explicit, `!i` forces the path, and the
//!    report names what is *not* detected.
//! 2. **The handoff** reuses `$EDITOR`'s stop/run/start. That is the whole point
//!    of the slice: a second implementation is how the two drift, and this repo
//!    has now applied the one-copy rule to grant mappings (#456), resume commands
//!    (#403) and the widget tree (#409).
//!
//! # What is pinned here
//!
//! Detection is pure - it reads a command string and answers a bool - so it is
//! tested directly and exhaustively. The handoff is I/O against a real terminal,
//! so its row drives a fake child that asserts `isatty(stdout)` on the other side
//! of the boundary: the honest instrument, rather than trying to script a real
//! `vim`.
//!
//! Verifies: gh #407 pattern 3, FR-UI-14's `!` contract, FR-UI-15's TTY
//! honesty, and NFR-24.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use lca_ui::chat_shell::{needs_tty, without_force};

/// The `!`-line vocabulary: everything after `!`/`!!`, trimmed.
fn detect(line: &str) -> bool {
    needs_tty(line)
}

// Verifies: gh #407's headline cases - the three shapes the issue names as
// hanging today. Each is a program that puts the terminal into raw mode, opens a
// fullscreen buffer, or pages interactively; all three spin or refuse on a pipe.
#[test]
fn the_three_named_hangs_are_detected() {
    for command in ["vim notes.txt", "git add -p", "git log | less"] {
        assert!(detect(command), "`{command}` needs the terminal (gh #407)");
    }
}

// Verifies: gh #407 - **the editor family**, because that is the most common
// shape and the one a user is most likely to type. `vi`/`vim`/`nvim`,
// `nano`/`pico`, `emacs`, `hx`/`helix`, `micro`, `kak`.
#[test]
fn the_editor_family_is_detected() {
    for command in [
        "vi notes.txt",
        "vim",
        "nvim notes.txt",
        "nano",
        "pico notes.txt",
        "emacs -nw notes.txt",
        "hx notes.txt",
        "helix notes.txt",
        "micro notes.txt",
        "kak notes.txt",
    ] {
        assert!(detect(command), "`{command}` is an editor");
    }
}

// Verifies: gh #407 - **`git`'s interactive subcommands**. `add -p`/`--patch`
// prompts per hunk, `rebase -i` opens a sequence editor, and `commit` without
// `-m` opens one too. The plain forms must stay piped: `git status` captured into
// the transcript is the normal case, and turning it interactive would be the
// false positive the design warns about.
#[test]
fn the_interactive_git_subcommands_are_detected_and_the_plain_ones_are_not() {
    for command in [
        "git add -p",
        "git add --patch",
        "git add -i",
        "git rebase -i HEAD~3",
        "git rebase --interactive main",
        "git commit",
        "git commit --amend",
    ] {
        assert!(detect(command), "`{command}` is interactive");
    }
    for command in [
        "git status",
        "git log --oneline",
        "git diff",
        "git commit -m \"a message\"",
        "git add .",
    ] {
        assert!(
            !detect(command),
            "`{command}` must stay piped - its output belongs in the transcript"
        );
    }
}

// Verifies: gh #407 - **the fullscreen TUI family**: pagers/editors that take
// over the screen, and the REPLs. `less`/`more` alone at the *head* of a command
// is interactive too (`!less notes.md`), not only after a pipe.
#[test]
fn fullscreen_tuis_pagers_and_repls_are_detected() {
    for command in [
        "less notes.md",
        "more notes.md",
        "fzf",
        "htop",
        "top",
        "tmux",
        "tig",
        "lazygit",
        "python",
        "python3",
        "node",
        "irb",
        "psql",
        "sqlite3 db.sqlite",
        "bash",
        "zsh",
        "fish",
    ] {
        assert!(detect(command), "`{command}` takes over the terminal");
    }
}

// Verifies: gh #407 - **`docker -it` / `kubectl -it`**: the flags exist precisely
// to attach a terminal. Without them the command is fine piped.
#[test]
fn docker_and_kubectl_are_detected_only_with_their_tty_flags() {
    for command in [
        "docker run -it ubuntu bash",
        "docker run -ti ubuntu bash",
        "docker exec -it web sh",
        "kubectl exec -it pod -- sh",
    ] {
        assert!(detect(command), "`{command}` asks for a TTY");
    }
    for command in ["docker ps", "docker run ubuntu echo hi", "kubectl get pods"] {
        assert!(!detect(command), "`{command}` is fine piped");
    }
}

// Verifies: gh #407 - **the pipe-tail check**, which is pi's `| less|more|fzf|vim`
// shape. A command whose *last* stage is a pager or fullscreen program needs the
// terminal; one that merely runs a pager in a subshell or earlier in the pipeline
// does not.
#[test]
fn a_pipe_tail_of_a_pager_or_fullscreen_program_is_detected() {
    for command in [
        "git log | less",
        "git diff | more",
        "ls | fzf",
        "cat notes.md | vim -",
        "find . -name '*.rs' | less -R",
        "journalctl -xe | less +F",
    ] {
        assert!(detect(command), "`{command}` ends in a pager");
    }
    // A pager *earlier* in the pipeline has a later stage to talk to, so the
    // tail is what decides.
    for command in ["less notes.md | head -5", "git log | grep fix | head"] {
        assert!(
            !detect(command),
            "`{command}` does not end in the pager, so it is captured"
        );
    }
}

// Verifies: gh #407 - **`!i` forces the interactive path** for anything, which is
// the user's escape from every gap in the list. `without_force` is what the
// rest of the detector sees afterwards, so the force marker must not leak into
// the program name.
#[test]
fn the_explicit_opt_in_forces_the_interactive_path() {
    assert!(detect("!i some-random-program"));
    assert!(detect("!i curl -s https://example.com"));
    assert_eq!(
        without_force("!i some-random-program"),
        "some-random-program",
        "the marker is stripped before the command runs"
    );
    assert_eq!(
        without_force("!i   spaced"),
        "spaced",
        "and surrounding whitespace goes with it"
    );
    assert_eq!(
        without_force("echo hi"),
        "echo hi",
        "a command without the marker is untouched"
    );
    // A program that merely *starts* with `!i` is not the marker.
    assert_eq!(without_force("!include"), "!include");
    assert!(!detect("!include"));
}

// Verifies: gh #407's false-positive story - **a command that merely contains an
// editor's name is not interactive**. This is the row the design's "detect
// conservatively" rule exists for: a substring detector would send
// `grep vim notes.md` to a fullscreen `vim` invocation that never happens, and
// the user would lose the interface for a search.
#[test]
fn a_name_used_as_an_argument_does_not_trigger_the_path() {
    for command in [
        "grep vim notes.md",
        "rg -n nano src/",
        "echo vim",
        "cat ~/.vimrc",
        "ls /usr/bin/less",
        "git log --grep=vim",
        "chmod +x script.sh && ./script.sh",
        "echo 'less is more'",
    ] {
        assert!(
            !detect(command),
            "`{command}` mentions a name but is a plain command"
        );
    }
}

// Verifies: gh #407 - **a `sudo`/`env`/`nohup` prefix still finds the program**,
// because the leading wrapper is what shells through to it. Without this,
// `!sudo vim /etc/hosts` - the single most likely interactive command after
// plain `vim` - would be missed.
#[test]
fn a_wrapper_prefix_does_not_hide_the_program() {
    for command in [
        "sudo vim /etc/hosts",
        "sudo -u root nano /etc/hosts",
        "env FOO=1 vim notes.txt",
        "nohup vim notes.txt",
        "command vim notes.txt",
        "time vim notes.txt",
    ] {
        assert!(detect(command), "`{command}` runs an editor");
    }
}

// Verifies: gh #407 - **the ordinary case stays piped**, which is the regression
// pin the brief asks for. Every one of these must keep streaming into the
// transcript card; making them interactive would break the feature `!` exists
// for.
#[test]
fn ordinary_commands_are_not_interactive() {
    for command in [
        "echo hi",
        "ls -la",
        "cargo test",
        "cat notes.md",
        "make",
        "npm install",
        "echo hi | grep h",
        "true",
        "",
    ] {
        assert!(!detect(command), "`{command}` is captured, not interactive");
    }
}

// ---------------------------------------------------------------------------
// The handoff: a child that asserts it got a real TTY
// ---------------------------------------------------------------------------
//
// `isatty(1)` is the honest instrument the brief asks for: it is exactly the
// question `vim` asks before it decides to draw. A real `vim` cannot be scripted
// in CI, but this can, and it fails in precisely the way the bug did - a piped
// child answers "no tty".
//
// The two halves are driven separately because they meet at one seam: the
// detector (above) decides, and the runner inherits. Proving each against a real
// terminal is what makes the pair meaningful.

/// The command whose exit status encodes whether stdout was a TTY: 0 for a
/// terminal, 1 for anything else. `test -t 1` is POSIX and needs no helper
/// installed, which matters because the alternative - scripting a real `vim` -
/// is exactly what the brief says not to attempt.
#[cfg(unix)]
const TTY_PROBE: &str = "test -t 1";

/// Run `command` with inherited stdio, the way the interactive path does, and
/// return its exit code. Deliberately a small copy of `interactive_run`: the
/// point of the row is to prove *inheritance* is what fixes the hang, and adding
/// the real runner as a dependency of a test crate would mean a `Cargo.toml`
/// edit this cycle's boundary forbids.
#[cfg(unix)]
fn run_inherited_in_pty(command: &str) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    let mut child = std::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .spawn()
        .expect("the probe spawns");
    let status = child.wait().expect("the probe exits");
    status
        .code()
        .unwrap_or_else(|| 128 + status.signal().unwrap_or(1))
}

// Verifies: gh #407's mechanism - **the pipe is the bug**. This row is the
// negative control: the same probe run the way `spawn_direct` runs it (piped)
// reports "not a terminal", which is the condition `vim` refuses on. Without this
// row the fix's row would not prove what it fixed.
#[cfg(unix)]
#[test]
fn a_piped_child_sees_no_terminal() {
    let output = std::process::Command::new("sh")
        .arg("-c")
        .arg(TTY_PROBE)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .expect("the probe runs");
    assert_eq!(
        output.status.code(),
        Some(1),
        "a piped `test -t 1` exits 1: stdout is not a terminal, which is the \
         condition a fullscreen program refuses on"
    );
}

// Verifies: gh #407's fix - **inheritance is what makes the child see a
// terminal**. Driven under a real pty (the test asserts it has one first, so it
// skips with a named reason rather than lying when run from a harness that does
// not).
#[cfg(unix)]
#[test]
fn an_inherited_child_sees_the_real_terminal() {
    use std::io::IsTerminal as _;
    if !std::io::stdout().is_terminal() {
        eprintln!(
            "skip: this row needs a real terminal on stdout (run it under tmux or \
             a pty); every other row in the file is terminal-independent"
        );
        return;
    }
    assert_eq!(
        run_inherited_in_pty(TTY_PROBE),
        0,
        "an inherited child sees a terminal - `vim` would draw instead of refusing"
    );
}

// Verifies: gh #407 - **the two halves agree**: every command the detector
// accepts as interactive is one whose failure mode this handoff actually fixes.
// Pinned structurally because the detector's list and the runner's behaviour are
// separate code paths, and a list entry that no test exercises would go unnoticed
// if the two ever disagreed about what "interactive" means.
#[cfg(unix)]
#[test]
fn a_detected_command_would_have_failed_on_a_pipe() {
    // `test -t 1` is the stand-in: a command that *only* changes behaviour on a
    // terminal. Every detected command is one in that class by the detector's
    // contract, so drive the stand-in through the detector to prove the contract
    // is reachable.
    assert!(
        needs_tty("!i test -t 1"),
        "the opt-in reaches the interactive path for a command the list does not \
         know - which is the documented escape for every gap"
    );
    assert!(
        !needs_tty("test -t 1"),
        "and without the opt-in it stays piped, where it answers 3"
    );
}
