# ADR-0041: The shell tool's interpreter selection and command transport

Status: accepted.

Date: 2026-10-01.

## Context

The owner drove the 0.5.2 build on Windows and pasted the session log. Two
defects in that log, both in the `shell` tool, both reproducible:

- `echo "double"` reached `cmd.exe` as `\"double\"`. The command string
  travelled as one `CreateProcess` argv element, and the shell re-parsed a
  command line the sender never wrote.
- `echo one` on one line and `echo two` on the next ran **only the first
  line**, silently. The same text written through the `write` tool and run
  as a `.bat` file behaved perfectly, which locates the fault in the
  command *transport*, not in any shell.

A third problem is visible in the same log without being a crash: the agent
spent around ten tool calls probing which shell it was in - `echo %VAR%`,
`echo $VAR`, `dir`, `ls` - because its tool description said "the platform
shell" and nothing more. On Windows that phrase covers four different
dialects with different separators, variable syntax, and path shapes.

And there is a fourth, quieter: on Windows the tool always ran `cmd.exe`.
Most developers' bash on this platform is Git Bash, and the `bash` that a
`where bash` search finds first is usually `C:\Windows\System32\bash.exe`,
the WSL stub, which runs in a different filesystem namespace (`/mnt/c/...`)
and would silently change what every path in a command means.

## Decision

**Resolve the interpreter once, from configuration, through a documented
ladder.** `shell.path` names an exact interpreter and wins outright - it is
used even when it is the WSL stub, because naming a path is an explicit
statement of intent. Otherwise `shell.tool` selects `auto` (the default),
`bash`, `pwsh`, `powershell`, or `cmd`. On Windows, auto walks Git Bash at
its known installation locations by *path*, then `pwsh.exe`, then
`powershell.exe`, then `cmd.exe`; a `PATH` search for bash is a later rung
and skips the WSL stub. On Unix the behavior is unchanged: `sh`, with
`shell.tool` able to ask for `bash` or `pwsh`. A configured interpreter
that cannot be found is a loud error naming every location searched - never
a silent fallback to a different shell.

**Tell the model which shell it got.** The `shell` tool's description is
built at startup from the resolved interpreter: its name, its program path,
its dialect (`$VAR` versus `%VAR%`, separators, path shape), and the
fidelity guarantee below. The description is the model's contract.

**On Windows, the command travels in a file.** The command is written to a
per-call temporary script - `.sh` for bash, `.ps1` for pwsh and PowerShell
5.1, `.cmd` for cmd, run as `bash <file>`, `pwsh -NoProfile -NonInteractive
-ExecutionPolicy Bypass -File <file>`, and `cmd /D /C call <file>` with
CRLF line endings - and the file is deleted when the call ends, on every
path including timeout and cancellation. The POSIX path stays `sh -c` with
the command as one argv element: `execve` carries that string byte for byte
already, and the suite that proves it is green. Churning a working
transport to look consistent would trade a proven path for an unproven one.

The contract this buys, stated for the model and for the tests: a command
reaches the shell exactly as written - quotes, newlines, metacharacters,
trailing spaces, unicode, and escape bytes included - and every line runs.

## Alternatives considered

**Quote the command for `cmd /C` correctly.** This is the fix the symptom
invites, and it is the wrong layer. There is no single correct quoting:
`cmd.exe` parses its own command line, `CreateProcess` parses it once
before that, the right escape depends on whether the next byte is inside
quotes, and newlines have no quoting at all. Every quoting table is a list
of the cases its author thought of; a script file has no cases.

**Pipe the command to stdin** (`bash -s`, PowerShell `-Command -`), which
is what pi does for its legacy-WSL-bash case. It is faithful for bash, works
for PowerShell, and does not exist for `cmd.exe`. One transport that covers
all three beats two transports plus a special case, and the file form is
also what the owner's own experiment (`write` a `.bat`, run it) proved
works.

**Always prefer Git Bash and keep `cmd` as a manual override.** Bash is not
always installed; a Windows box without Git for Windows still needs a shell
tool, and `cmd.exe` is the one that is always there. Auto falls back to it
rather than failing.

**Resolve bash with `where bash` and take the first hit.** This is what the
owner's log shows happening, and it is how a WSL stub gets picked: it
changes the filesystem namespace under every command the model writes. The
ladder checks known install locations first for exactly this reason.

**Let the model pass the shell per call.** A per-call interpreter would
make the command's meaning depend on the model's guess, multiply the
description surface, and gain nothing the config does not already give a
human.

## Consequences

Every Windows command costs one temp-file write and one delete. That is
cheap next to spawning the shell at all, and it is the price of byte
fidelity on a platform whose command-line parsing is the reason this record
exists.

The tool description is now built rather than static, so `ToolExecutor`
carries the resolved shell and the core asks it for the description at
request-assembly time. A backend that owns no interpreter (the web target's
host-delegated one, FR-WEB-3) keeps the generic description.

A bad `shell.path` fails every shell call with the resolution message
instead of quietly running somewhere else; the interface also prints the
error at startup, because a config typo should be visible before the first
tool call.

`docs/platform-notes.md` gains the ladder, the WSL-stub trap, the transport
table, and the cmd codepage reality (its `echo` writes in the OEM codepage,
so non-ASCII output through `cmd` is not UTF-8 - a platform fact, not a
transport defect).

## Revisit conditions

Evidence that a shell rejects file-based invocation, or that a command
shape exists which the file transport cannot carry (a command longer than a
filesystem's path or file limits would be the first candidate). Evidence
that users routinely want a different shell per project than their config
says, which would argue for a project-scope `shell.*` section rather than a
per-call parameter.
