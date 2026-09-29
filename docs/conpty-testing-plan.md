# Windows console testing — open TODO and plan

Status: **RESOLVED** (2026-09-29). The five tests are green on Windows, on
a real console and on the hosted `windows-latest` CI leg. The fault was in
our ConPTY call, not the runner. See the update log and
`docs/platform-notes.md`.

Tracked debt: the quarantine ledger in `docs/platform-notes.md` (Windows
section), opened 2026-09-25 under the testing plan's §13 quarantine rule.
Related: `docs/testing-plan.md` §14 (real-terminal tests) and the
platform-gating contract.

## What is quarantined, and why

Five tests are `#[ignore]`d on Windows only. All five compile on the
`windows-latest` leg on every push (so they cannot drift), and all five are
green on Linux and macOS:

| Test | Covers |
|---|---|
| `pty_spawn_delivers_terminal_output` | the `pty` capability's output path |
| `pty_spawn_delivers_program_output_and_exit_code` | pty spawn + exit |
| `typing_into_the_panel_reaches_the_program_and_comes_back_as_data` | ui-example panel + pty keystroke round trip |
| `the_tui_renders_a_turn_in_a_windows_console` | §14 checklist, ConPTY harness |
| `the_logins_secret_prompt_masks_input_in_a_windows_console` | §14 masking, ConPTY harness |

## What is already ruled out (do not redo this work)

The pty path went through eight CI rounds before the remainder was bounded
(`docs/platform-notes.md`, "The pty capability on hosted runners"). Fixed
for real along the way, and worth knowing before touching the code again:

- `UpdateProcThreadAttribute` was given the pseudoconsole handle's *value*
  cast to a pointer and the failure was ignored — every pty child was
  silently born on the parent's console and its output landed on the
  runner's stdout. Fixed.
- Verbatim `\\?\` working directories: `cmd.exe` reads them as UNC and
  refuses, defaulting to `C:\Windows`. Fixed (plain paths for the child).
- Closing the console was a reader deadlock; it is now the documented
  end-of-stream flush. Fixed.
- The ConPTY pump reader busy-spun; it now yields. Fixed.
- `PtyChild::spawn` accepts an environment block (the deterministic-sandbox
  requirement from the test-gaps plan).

**The bounded remainder**, reproduced on hosted runners with all fixes in
place:

1. A `cmd /C` child sits **several seconds without producing a byte** on
   the output pipe (known ConPTY quirk, documented).
2. An interactive child's **banner arrives but its keystroke echo does not**.
3. The ConPTY real-terminal pair gets **zero bytes in sixty seconds** — no
   output at all.

## Working hypothesis

The hosted runner executes as a service without an attached console
session. ConPTY (the pseudo-console API) is present and accepts handles —
children are created and exit — but the I/O pump behaves as if no terminal
is attached to feed it. The evidence shape (no output at all, versus the
echo-only failures in the pty trio) points at the runner's session
environment rather than at our code, but that is a hypothesis until a real
console machine confirms or kills it. The three candidate faults named in
the ledger: the console, the environment block `PtyChild::spawn` passes,
or the runner — in that order of suspicion.

**Killed 2026-09-29.** A real console reproduced the same zero-byte shape,
so the runner was exonerated: the fault was ours. `mode con` inside the
pty child reported a fresh 120x30 console instead of the requested 24x80,
which pointed straight at the pseudoconsole attribute. See the update log.

## The plan (once a console machine exists)

1. **Run the pty trio first** (`cargo nextest run -p lca-tools -p
   lca-ext-host -p ui-example -E 'test(/pty_spawn_delivers|typing_into_the_panel/)')`).
   They need a console but not the full TUI. Their failure shape on a real
   console will isolate console-vs-runner within minutes:
   - green → the hosted runner is the fault; keep the `#[ignore]`s with a
     "hosted-runner limitation" note and add a self-hosted leg (below);
   - red with the `cmd /C` delay shape → the ConPTY quirk is ours to
     handle (the known multi-second no-output needs a readiness wait, not
     a fixed timeout);
   - red differently → re-open the ledger with the new evidence.
2. **Then the ConPTY TUI pair** against §14's checklist: startup renders,
   a scripted turn streams and renders, the permission modal works, the
   secret prompt masks input (no key bytes in the visible frame), resize
   re-renders, clean quit writes `session-end` and is resumable.
3. **Diagnostics on failure**: the existing instrumentation (per-call pipe
   state, child exit code, spawn command line) already lands in run
   artifacts — keep that habit; it is what bounded the first eight rounds.
4. **Exit criteria:** the five tests green on a console-attached machine
   and either (a) green on CI via a self-hosted runner, or (b) a named,
   documented platform constraint in `platform-notes.md` in the same style
   as the NFR-4/NFR-29 Windows numbers — i.e. bounded honestly, not
   deleted. Only then does the ledger entry close.

## Options for getting a console (owner decision)

| Option | Cost | Notes |
|---|---|---|
| **A. Self-hosted runner** on any always-on Windows box you control | hardware only | the durable fix: CI gets a real console permanently; the five tests move from `#[ignore]` to the self-hosted leg |
| **B. One-shot interactive Windows VM** (cloud desktop / Azure VM with console, or a local Hyper-V/VirtualBox session) | ~an hour of time | enough to run the plan above and *bound* the failure; if it's a hosted-runner limitation, A becomes optional and the debt closes as a documented constraint |
| **C. Hold the quarantine** | zero | the ledger's age clock keeps running (opened 2026-09-25); per testing-plan §13 a stale quarantine eventually blocks new quarantines from the same crates, so this is a loan, not a resting place |

**Recommendation:** B first — one interactive session answers the
console-vs-runner question definitively and may close the whole debt
without new infrastructure. Do A only if B shows the tests deserve a
permanent console leg.

## Update log

- 2026-09-25: quarantine opened (ledger); 8 pty rounds' fixes landed.
- 2026-09-26 (cycle 5 re-check): whole set still compiles on the Windows
  leg; none retried on a console; blocker unchanged — the machine.
- 2026-09-27: this plan written; awaiting the owner's console route.
- 2026-09-29: **resolved.** On a console-attached Windows machine the pty
  trio failed with zero bytes, and the isolation ladder found the fault in
  our ConPTY call: `PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE` was handed the
  address of an `HPCON` instead of the value itself, and the child's std
  handles were not invalidated, so every child was born on a fresh default
  console and the output pipe stayed empty. Fixed. Rendering on ConPTY then
  exposed a second, independent defect: `/exit` hung joining a reader
  blocked in `ReadFile` (`wait_stdin` never timed out); fixed with
  `WaitForSingleObject` plus `CancelSynchronousIo` before the join. All
  five tests green on the real console *and* on the hosted `windows-latest`
  leg, so exit criterion (a) is met with no self-hosted runner, and the
  ledger is closed (`docs/platform-notes.md`). Regressions 35-37.
