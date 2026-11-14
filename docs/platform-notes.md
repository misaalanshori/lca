# Platform notes

Version 0.1, 2026-09-20.

This is the collection of per-platform behavior a contributor needs and would otherwise learn by hitting it in CI. The requirements and design document states what must work on each platform; this document states the specific reason it doesn't work by default and what makes it work. Read this before touching path handling, process spawning, or the loopback listener on any platform other than the one in front of you.

**Where the agent's data lives: `~/.lca`, on every platform (R7, 2026-10-01).** Sessions, extensions, credentials, `grants.json`, state, temp, themes, `ui.json`, and `config.toml` all sit under one home dot-directory, pi's `~/.pi` shape. `HOME` decides it; `USERPROFILE` is the Windows fallback when `HOME` is unset. **This moved** from the platform conventions this document used to describe - `~/.local/share/lca` and `~/.config/lca/config.toml` on Linux, `~/Library/Application Support/lca` on macOS, `%APPDATA%\lca` on Windows - because "where is my data" should not need a platform table to answer, and the owner's own agent could not find its state on Windows. **There is no migration code:** the old directories are left exactly as they are, and a user who wants the old sessions copies them across by hand (`cp -a ~/.local/share/lca/sessions ~/.lca/` and so on; on Windows, `xcopy /E /I "%APPDATA%\lca" "%USERPROFILE%\.lca"`). The old paths are also named in `CHANGELOG.md`. `docs/configuration.md` carries the path reference for each key.

## Linux

The release targets are `x86_64-unknown-linux-musl` and `aarch64-unknown-linux-musl`, not the `-gnu` targets, per NFR-8. musl gives a genuinely static binary with no dynamic dependency at all; glibc's static linking has real, known gaps, most notably around NSS-based name resolution, that make it the wrong default for a binary meant to run unmodified across arbitrary distributions.

Session and extension state, the user configuration, and every other agent-owned file live under **`~/.lca`** (R7). The `fs` capability's `home-config` scope still resolves to `$XDG_CONFIG_HOME` or `~/.config`, the directory an extension reads *another tool's* login from; the agent's own tree is not under it on Linux, and the state-directory exclusion in the capability catalog refuses every resolution that would enter `~/.lca`, with one bounded exception: the active session's `temp` scope resolves to `~/.lca/tmp/<session>` and nowhere else in the state directory, confined by the same root check that contains every other scope.

The loopback OAuth listener binds `127.0.0.1` specifically, not `0.0.0.0` or `::`, and this is worth stating explicitly because it's an easy default to get wrong when a framework's "bind a local server" helper defaults to all interfaces. Binding all interfaces on a machine with no firewall would expose the callback endpoint, briefly, to the local network, which is exactly the class of mistake `net-local`'s own design in ADR-0011 is trying to keep separate from ordinary network access.

Over SSH there is no browser to open and often no URL to click (OSC 8 does not survive every multiplexer), so the OAuth waiting screen names its way out: `ctrl+x` copies the sign-in URL through the copy ladder - an in-process write through `arboard` (gh #406: the same backend the clipboard *image* read uses, which handles Win32 contention, the macOS pasteboard and X11 `INCR` itself; the helper-per-copy ladder that shelled out to `wl-copy`/`xclip`/`xsel`/`pbcopy`/`clip` is gone), else an OSC 52 write that works over remote SSH and tmux but is reported unverified, because the terminal may silently block it (testing plan §14). A headless box - SSH, a container, `DISPLAY` unset - is the case the ladder was built for: the in-process write fails, OSC 52 takes over, and the notice says which step ran rather than claiming a copy that did not happen. Both halves of this ladder have been driven in a real tmux pane with `DISPLAY` unset; the OSC 52 payload in the raw stream decoded to the copied text. Paste the callback URL back when the quiet period offers the manual field, and the flow completes without the loopback listener ever seeing the browser.

Process cancellation, per ADR-0014, kills a shell command's process group, not just the immediate child, since a shell command frequently forks its own children and killing only the parent leaves orphans running. This needs `setsid` or an equivalent at spawn time so the spawned process has its own process group to kill. The kill itself must pass the group as `kill -9 -- -PGID` with the `--` separator: procps `/bin/kill` before 4.0.5 (Ubuntu 24.04 ships 4.0.2) truncates `-<pgid>` to its first digit (upstream issue #354, fixed in 4.0.5), so a pgid starting with `1` becomes `kill(-1, SIGKILL)` - every process of our uid, which on CI includes the test runner and the agent and reads as a silent 30-minute hang (2026-10-15). All group kills go through one helper (`lca-tools` `kill_process_group`) with a `pgid > 1` guard so `-0`/`-1` stay unreachable.

### Termux (Android): the soft keyboard resizes the terminal (gh #372 bug 3)

Termux is Linux, but its soft keyboard changes the terminal height by 10-15 rows every
time it shows or hides. The scrollback renderer used to run a full re-render on every
height change (`\x1b[2J\x1b[H\x1b[3J`) to re-align the visible viewport, so on Termux
that wiped the screen **and the scrollback** repeatedly - the transcript the reader was
looking at disappeared every time they touched the prompt.

So a **height-only** change is exempt on Termux: a height change does not invalidate
wrapping, so the visible viewport can re-anchor without a rebuild. The exemption is
narrow by construction - `width_changed` is decided first and still rebuilds, because a
rotation or a genuine resize really does change wrapping, and there is a row that fails
if a width change on Termux stops rendering.

The detection is `TERMUX_VERSION`, which is Termux's own variable. Deliberately **not** a
`TERM=screen` heuristic: tmux and GNU screen also report `screen`, and neither has a soft
keyboard resizing the pane under the application.

### A resize during a tty handoff (gh #522)

The three tty handoffs - `$EDITOR` (Ctrl+X Ctrl+E), an interactive `!` command, and
`Ctrl+Z` suspend - are the same sequence: leave the screen, `terminal.stop()`, run the
child with the tty, `terminal.start()`, enter the screen. While the child owns the
terminal nobody reads, and on **Unix** the resize signal (`SIGWINCH`) is not delivered to
a process that has no reader waiting for it - so a resize in that window is not merely
late, it is unobserved.

`stop()` joins the reader thread and `start()` spawns a fresh one, and that reader seeds
its own `last_size` baseline from `sys::terminal_size()` on its first pass. So the size it
finds is not a *change* from anything it remembers, no resize fires, and the interface
keeps composing frames at the width it recorded before the handoff. Measured in a real
140-column pane with an `$EDITOR` that sleeps 2 s, resized to 60 columns while the child
ran: three trials of three left the resumed frame with a 140-cell editor cap wrapped by
tmux into `╭───` plus a continuation row. The same resize with **no** handoff was clean
5 of 5, so the window is the cause, not the resize path.

**The rule that follows: a resume re-derives the frame's size from the tty.** `start`
calls `ProcessTerminal::adopt_current_size`, which re-reads `TIOCGWINSZ` and fires the
resize handler only when the size actually moved - after installing the handlers, so the
notification reaches the loop rather than the previous frame's no-op. The size is one
fact with one owner (`shared.cols`/`rows`); the reader mirrors it, which is what makes
re-seeding it at resume correct instead of a second source. A resume that finds the same
size stays quiet, so exiting `$EDITOR` does not repaint for nothing.

Windows is unaffected: a console reports the change as a `WINDOW_BUFFER_SIZE_EVENT`
record, which the reader sees on its next wait after `start` (`WaitOutcome::Resize`).

**Two measurement lessons, both paid for here.** (1) Count display cells, never bytes: a
`─` is 3 bytes in UTF-8, and counting bytes once made a correct 60-column dock read as
180. (2) A max-width check cannot see this defect at all: tmux wraps an over-wide frame, so
every physical row *fits* the pane while the layout is 140. The signature that cannot be
fooled is a box cap whose corner went missing - `╭` with no `╮`, or `╰` with no `╯` - which
is what `wait_for_layout_width`'s `logical_width` now joins and reports.

`shortcut:` the exemption covers the height change itself, not the deleted-lines arm
further down - if a soft-keyboard toggle coincides with content shrinking below the new
viewport top, that arm still calls the full render. pi has the identical hole, so closing
it means changing the scroll arithmetic and the exemption together; upgrade when a Termux
report shows it rather than widening the exemption blind.

## macOS

Release targets are `x86_64-apple-darwin` and `aarch64-apple-darwin`. Apple does not support a fully static binary for anyone, on any platform; the agent, like every other native macOS binary, dynamically links `libSystem`. This is not a gap relative to the Linux musl build, it is a platform constraint every macOS binary shares, and it costs nothing in practice since `libSystem` is present on every Mac the binary will ever run on.

Distribution requires code signing and notarization, per the release policy, and this needs an entitlements file granting outbound network access and, if the binary is sandboxed under App Sandbox rather than only signed, file access entitlements matching what the `fs` capability's scopes actually need. A binary downloaded from outside the App Store that isn't notarized triggers Gatekeeper's quarantine warning on first launch; notarization is what removes that friction, not signing alone.

The agent's own data lives under **`~/.lca`** (R7), like every other platform. The `home-config` scope still resolves to `~/Library/Application Support/`, which is a broader directory than the Linux equivalent and contains the agent's tree as a child on macOS; the host's state-directory exclusion keeps credential isolation true anyway, refusing every `fs` resolution that would enter `~/.lca` through `home-config` and through ad hoc grants alike, with one bounded exception: the active session's `temp` scope resolves to `~/.lca/tmp/<session>` and nowhere else in the state directory, confined by the same root check that contains every other scope. On Linux the same exclusion is structural (`~/.lca` is not under `~/.config`), and the rule is identical on both platforms with one shared test.

The loopback listener binds `127.0.0.1` for the same reason as Linux. Process group cancellation uses the same `setsid`-equivalent approach as Linux, since macOS shares the POSIX process model there.

**`/proc` does not exist here, and a harness that assumed it did was silently idle (gh #589).** `crates/lca-cli/tests/common/mod.rs`'s wait budget is scaled by a load factor, and that factor was read from `/proc/loadavg` with `.unwrap_or(0.0)` — so on macOS (and Windows) every real-terminal wait got **1x**, the same as an idle box, while the Linux dev machine got the scaling the rule was written for. The family of consequences is in `docs/testing-plan.md` §14; the platform lesson is this file's: **a Linux-only file read is a silent default on the other two platforms, and "silent default" is what turns a designed allowance into a placebo.** macOS publishes no load average at all (no `/proc`, and `getloadavg(3)` exists but returns a decaying 1/5/15-minute figure rather than instantaneous pressure). The harness now measures **scheduling overshoot** instead — three 20 ms sleeps, minimum overrun — because that is the quantity a slow runner actually inflicts on a thread waiting to repaint, and it needs no platform API. Measured here, four cores: idle median 0.2 ms / p90 3-4 ms, and the **minimum** of three samples stayed within a fraction of a millisecond (factor 1). Under 16 and then 32 spinners the median rose to 4 ms and p90 to 12 ms, but the minimum-of-three **still classified as 1x at load 37** - Linux's CFS hands a sleeping task its slice as soon as one is free, so systematic lateness is not what this box exhibits (gh #589's own correction of an earlier claim in this paragraph). That weakness is harmless because Linux never takes this branch `/proc/loadavg` answers first; the branch exists for platforms where the delay is systematic, which is what a hosted macOS runner exhibits with one 60-80 ms quantum. Windows is in the same position for the same reason: no `/proc`, and the same sleep-based probe answers.

**Screenshot paths resolve by shape, not by one template** (gh #405). macOS has named screenshot files at least four ways, and a user pastes whichever one Finder shows them:

| shape | notes |
|---|---|
| `Screen Shot 2024-01-01 at 10.00.00.png` | classic: a space in the stem, ` at ` before the time |
| `Screenshot 2024-01-01 at 10.00.00.png` | newer macOS drops the space |
| `Screenshot 2024-01-01 10.00.00.png` | and sometimes the ` at ` too |
| `Screen Recording …` / `ScreenRecording …` | same four shapes with a `.mov` extension |
| `Capture d'écran …` | the French stem, accents in NFC or NFD |

The folder is localised while the *display* name is not: `~/Desktop` is on disk `Bureau`, `Schreibtisch`, or `Escritorio`. All spellings are searched, and `~/Pictures` is a **fallback** after every Desktop spelling — so turning the fallback on never changes which of two real files a user gets. A path whose name matches no screenshot shape is never reinterpreted (an existing file resolves to itself; a missing one stays not-found rather than becoming a guess). A miss reports every location tried rather than suggesting a file that is not there.

These rows are unit tests over a scratch home (`crates/lca-tools/src/paths/screenshot_tests.rs`); the resolver is driven directly rather than through a real macOS Desktop, so what is proven is the variant set and the precedence, not that a specific macOS release writes one of these names.

## Windows

Release targets are `x86_64-pc-windows-msvc` and `aarch64-pc-windows-msvc`, the MSVC ABI rather than the GNU one, since MSVC is the target the platform's own toolchain and most native dependencies assume, and is what `cargo-xwin` is built to cross-compile against from the release pipeline's Linux runners per the release policy.

This is the platform where fx has no support at all, and it's worth stating why that gap is real rather than incidental: terminal handling, process termination, and path semantics all differ from the POSIX model the rest of this document assumes, each in a way that's easy to get subtly wrong rather than obviously wrong.

**Terminal handling.** A legacy Windows console does not interpret ANSI escape sequences by default; Virtual Terminal Processing has to be explicitly enabled via `SetConsoleMode` before the renderer can rely on the same escape sequences it uses everywhere else. `crossterm`, the dependency originally chosen for terminal handling, does this automatically, but it's worth knowing why the call exists rather than treating it as boilerplate, since a future change to the rendering layer that bypasses `crossterm`'s setup would silently break Windows only.

*That change has since happened, and the warning applies to its replacement.* The interactive TUI's terminal layer is now the project's own (`crates/lca-tui/src/engine/`, ADR-0037): it enables Virtual Terminal Processing itself in `engine/sys.rs` under the module's documented `unsafe` exemption, and it enables VT input handling for the Windows console path as well. Any future change to that module inherits this paragraph's warning. `crossterm` has since left the dependency graph entirely (gh #24 made the consent prompt read a line instead of a raw keypress, its last use), so nothing outside that module touches console modes.

**A local Mac's trackpad is already accelerated, so the wheel curve must not run there** (gh #375 gap 1). macOS terminals receive wheel and trackpad deltas the OS has already accelerated and emit roughly one event per line; every other platform, and any SSH session (where the client's platform is unknown), sends one event per notch. pi's rule is the conjunction - `darwin && !SSH_CONNECTION && !SSH_CLIENT && !SSH_TTY` - and LCA adopts it by construction: `engine::keys::wheel_accelerates(on_macos, local)` is a pure function of the two facts, `is_local_session()` was already there for the Shift+Enter question (gh #367) and is now shared rather than re-read, and the alt-screen renderer carries the answer as a field set once at construction. `auto` on a local Mac is therefore one line per event; the **fixed** setting is untouched everywhere, because a user who asked for five lines gets five on every platform - a guard that also clamped the number would be a second bug. `LCA_WHEEL_ACCEL=1|0` forces the answer, in `LCA_SHIFT_ENTER`'s shape, which is also how the local-Mac branch is exercised on a machine that is not a Mac.

**Terminal protocol.** The engine's input layer absorbs the terminal-quirk surface so the rest of the UI does not have to: Kitty keyboard protocol negotiation (flags pushed before the capability query, with a device-attributes sentinel), xterm `modifyOtherKeys` as the fallback dialect, legacy escape sequences, bracketed paste, and sequence reassembly against partial reads (including the `\x1b` split rule that WezTerm's input reader imposes). It also speaks the output protocols the interface uses: OSC 52 for clipboard copy (a native clipboard command is preferred and verified; OSC 52 is the honest `CopyOutcome::Unverified` fallback), OSC 8 hyperlinks, and the OSC 11/DSR color-scheme parsers. Dark/light autodetection reads `COLORFGBG` at startup and also queries OSC 11/DSR; either reply sets the auto theme. Images render through the kitty/iterm2 graphics ladder (detected from the environment; tmux and screen fall back to the placeholder, as pi's ladder does). **The cell size every reservation is computed from is an estimate until the terminal is asked.** **The cell size is probed, and 9×18 is the stated fallback.** LCA sends `CSI 16 t` in the startup query, ahead of the `DA1` sentinel, and consumes the `CSI 6;h;w t` reply; the answer installs the real cell in pixels and the transcript re-renders, because an image's kitty id hashes its cell scale — a new scale is a new upload rather than a silent re-stretch. The query has to precede the sentinel: terminals answer in order, so `DA1` is what proves the cell reply is in, and a query sent after it would land in the same window that exists to reject strays. Before the reply arrives, `DEFAULT_CELL_PIXELS` (9×18, pi's default) is used and is documented as an **estimate**: a retina panel or a 10×22 font reflows the picture slightly taller or shorter than the rows it claims. A terminal that never answers keeps the estimate and still renders — the probe improves the picture, it does not gate it.

**A differential repaint is widened so it never cuts through an image block**, and a texture nothing points at any more is deleted. The renderer repaints an interval of document rows, and the terminal cannot redraw part of a kitty texture — so an interval starting or ending mid-image leaves the untouched half showing the old drawing (a ghost) and the repainted half showing the new frame. The interval is therefore expanded, in both the previous and the new frame, to a union of whole blocks, and each block's span is the run of *blank* rows it actually owns (`r=` capped by the next row holding text or the end of the document) rather than the claimed height. Ordering inside the frame matters: the orphaned ids are deleted **inside** the synchronized block, before the repaint, so the old image never vanishes for a frame. That ordering is **generic, not a WezTerm special case** (a byte-order row drives a terminal with no WezTerm identity and finds the delete first), and it applies on the **full-clear** path too: a width change or a shrink takes `full_render`, which emits a delete for **every id the previous frame held** immediately before `2J/H/3J`, because the clear orphans all of them. That is what bounds the terminal's texture count: an id hashes the cell scale, so **every distinct rendered width is a distinct texture** - one 800×600 image across a 30-column resize drag mints 30 of them, and without the full-clear sweep nothing would ever free them.

Deletion is an **id diff**, not a range scan. A replaced image's old texture needs `a=d,d=I,i=<old>` or the terminal accumulates one per replacement for the session; a *cropped* image (gh #324 gap 3) keeps its id and changes only its placement, so deleting it would remove the texture about to be re-placed. The accepted cost: an image spanning most of the viewport makes almost any range a near-full repaint. That is the right trade — a full repaint is correct, a sliced texture is not — and it is bounded to one pass per frame, so it cannot loop or double-emit.

**The kitty encoder refuses a format it cannot send** (gh #598). The protocol's `f=` selects the payload format and the spec names exactly three - `f=24` RGB, `f=32` RGBA (default), `f=100` PNG - so **there is no JPEG code**, and a client holding a JPEG must decode it or refuse. The encoder used to send `f=100` for whatever it was handed, which meant a JPEG (the shape `lca-tools`' resize produces for a vision model) was labelled PNG and the terminal drew nothing. It now maps `image/png` to `f=100` and **takes the placeholder for anything else**, naming the media type it could not send; webp and gif are refused for the same reason. The asymmetry that makes this a kitty-only bug: `encode_iterm2` carries no format field at all, so the terminal sniffs the payload and a JPEG has always displayed there.

**A partly visible image is placed as the band the window shows.** Fullscreen slices the transcript by whole rows, so a block that straddles the viewport edge arrives as a fragment — its header may be scrolled off the top, or its tail cut off the bottom — and neither is recoverable downstream. The window therefore crops it where the whole block is still addressable, emitting a **placement** with a pixel sub-rectangle (`y`/`h`) rather than re-encoding: the image id is unchanged (it hashes payload and cell scale, deliberately not the crop) and no base64 is re-sent, which is what makes scrolling past an image cheap. The band is `floor` on the near edge and `ceil` on the far one, so consecutive bands tile instead of leaving gaps; a top-clipped block's sequence moves onto its first *visible* row, because leaving it on the scrolled-away header makes the image vanish instead of showing its band. iTerm2 has no `y`/`h`/`r`, so it cannot be cropped at all — a straddling iTerm2 image is the case this does not cover.

The grid is chosen by **least distortion**, not by rounding up: `ceil` can make a picture 9% taller than it asks to be, so the count is compared against `ceil - 1` by `max(count/ideal, ideal/count)`, which weighs a grid twice too tall the same as one twice too wide. **The ladder has exactly three answers**: kitty, iTerm2, and none - so a terminal is either given a protocol this build can encode or given the placeholder, never a protocol it detects and cannot draw (gh #326). Sixel is deliberately absent, not forgotten: `foot`/`mlterm`/`yaft` report it and LCA does not speak it, so detecting it would promise an image and deliver a sentence, which is worse than not detecting it. pi is the same shape (`ImageProtocol = "kitty" | "iterm2" | null`), and neither project carries a Sixel encoder; adding one means palette quantisation and band encoding for terminals almost nobody runs. The placeholder is text on purpose and says so (it names the media type, dimensions and size, and adds `terminal graphics unavailable` when the header could not be read) - it does not pretend to be an image. `tests/regressions/326-*` fails if a fourth variant appears without an encoder, so a future Sixel arm has to bring one. Prompt navigation uses the transcript's own user-message offsets, not OSC 133 zones. The quirks and their test anchors live next to the code in `crates/lca-tui/src/engine/`; the port reasoning is preserved there in comments.

**Process termination.** Killing a spawned process on Windows by its handle alone does not kill children it spawned, the way a process group kill does on POSIX; a shell command that forks its own children, exactly the case ADR-0014's cancellation flow needs to handle cleanly, leaves orphaned processes behind unless the spawned process is placed in a Job Object at creation time and the whole job is terminated on cancellation. This is precisely the class of bug pi's own `taskkill-enoent.test.ts` and `bash-close-hang-windows.test.ts` exist to catch, and it recurs because the failure is silent: the visible process exits, the orphan keeps running, and nothing in an ordinary functional test notices.

**Path semantics.** Windows paths are case-insensitive by default and use backslash as the primary separator, though most APIs accept forward slash too. The `fs` capability's scope-escape check, which refuses a path resolution that leaves its granted scope, has to canonicalize before comparing on Windows specifically, because a case-differing path or a mixed-separator path that would obviously fail a naive string-prefix check on Linux can still resolve to a location inside, or outside, the intended scope on Windows. This is exactly the kind of platform difference a security-relevant check needs a dedicated test for, not an assumption that the same code path behaves identically everywhere; see the fuzz and property-based test coverage in `docs/testing-plan.md`.

**Shell.** The `process` and `shell` tool behavior on Windows targets the platform's own shell rather than assuming `bash` exists, per FR-TOOL-6. A command string that's valid `bash` syntax is not generally valid for `cmd.exe` or PowerShell, so an extension or a built-in tool constructing a shell command string, rather than an argument list, needs to be aware it's building a platform-specific string, not a portable one; preferring an argument list over a constructed command string avoids this class of problem entirely and should be the default choice wherever the underlying operation allows it.

**The `shell` tool's ladder and transport (ADR-0041, 2026-10-01).** The owner's v0.5.2 session log produced two defects and one piece of friction that together define this platform's shell contract, so all three are recorded here.

*Which interpreter.* `shell.path` names an exact interpreter and wins outright. Otherwise `shell.tool` picks `auto`, `bash`, `pwsh`, `powershell`, or `cmd`. Auto on Windows walks, in order: Git Bash at `%ProgramFiles%\Git\bin\bash.exe`, `%ProgramFiles(x86)%\Git\bin\bash.exe`, `%LOCALAPPDATA%\Programs\Git\bin\bash.exe`; then `pwsh.exe` (PATH, then `%ProgramFiles%\PowerShell\7\pwsh.exe`); then `%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe`; then `%ComSpec%` (or `%SystemRoot%\System32\cmd.exe`). On Unix, unchanged: `sh`, with `bash` or `pwsh` available through `shell.tool`. A configured interpreter that cannot be found fails every call with the list of locations searched. Nothing falls back silently: the agent's own log shows what silent resolution costs.

*Why not `where bash`.* On a stock Windows box `where bash` resolves to `C:\Windows\System32\bash.exe`, the WSL launcher stub. It is a real bash, and it runs inside a different filesystem namespace where `C:\Users\me` is `/mnt/c/Users/me`. Every path the model writes would mean something else. Auto mode skips that exact path; `shell.path` may still select it deliberately.

*Transport.* `cmd.exe` is not the only shell that reparses a command line, but it is the one where it bites: a command sent as one argv element arrives with `"` backslash-escaped and a second line gone, because `CreateProcess` quotes the string and the shell parses the result a second time. On Windows the command therefore travels in a **file**: `.sh` for bash (`bash <file>`), `.ps1` for pwsh and PowerShell 5.1 (`pwsh -NoProfile -NonInteractive -ExecutionPolicy Bypass -File <file>`), `.cmd` for cmd (`cmd /D /C call <file>`, CRLF line endings). The file lives in the system temp directory and is deleted when the call ends, including timeout and cancellation. POSIX keeps `sh -c` with the command as one argv element, which `execve` carries byte for byte; that path is proven and unchanged.

*What the model is told.* The `shell` tool's description is built at startup from the resolved interpreter: its name, path, dialect (`%VAR%` versus `$VAR`, `&` versus `;` separators, path shape), and the guarantee that the command reaches the shell exactly as written. The owner's log shows an agent spending about ten calls working out which shell it was in; that is a description defect, and the description now answers it.

*One platform fact that is not a defect.* `cmd.exe`'s `echo` writes non-ASCII text in the console's OEM codepage (437 or 850), not UTF-8. A command that must emit exact Unicode bytes should run under bash or PowerShell on this platform; the fidelity corpus asserts exact bytes for those two and asserts cmd's own behavior (which is byte-exact for ASCII, including quotes and CRLF) rather than pretending the codepage does not exist.

**Edited files.** The `edit` tool matches against LF-normalized text, so a CRLF file matches LF `oldText` and keeps CRLF on write, and a UTF-8 BOM is stripped for matching and restored on write (gh #114/#115, pi's pipeline order). A file that mixes endings keeps the style of its first newline; the tool never converts a file wholesale.

**Credentials.** The credential store's owner-only mode (`0o600`, NFR-14) is set on Unix. On Windows the file is created under `~/.lca/credentials` (the user profile's dot-directory, R7) and an explicit owner-only DACL is applied before the rename publishes it: `write_credentials` calls `SetEntriesInAclW` for the current user's SID and then `SetNamedSecurityInfoW` with `PROTECTED_DACL_SECURITY_INFORMATION`, so the file does not inherit the directory's ACEs. The call is one `#[cfg(windows)]` module with a documented `unsafe` exemption (the pty module is the other), and a Windows-only test asserts the resulting DACL is protected. This closes the documented 1.0 position (ADR-0021); before it, the file relied on the directory's default ACL.

**Launching a URL must never go through a shell (cycle 7, R3).** This is the "subtly wrong, not obviously wrong" class the section above opens with, so it is recorded in full. Opening a URL used to be `Command::new("cmd").args(["/C", "start", "", url])`. `std::process::Command` quotes only arguments containing spaces, so the argument arrives unquoted and `cmd.exe`'s *own* parser handles it — and `cmd` splits a command line at the first unquoted `&`. An OAuth authorize URL is exactly a `&`-separated query string (`…auth?client_id=…&response_type=code&…`), so the browser opened with only the part before the first `&`, and Google answered `Required parameter is missing: response_type`. The URL itself was correct; the launcher ate it. The fix is structural rather than a quoting table: `lca_tools::open_url` is the one implementation both callers share (the `oauth.open` host import and the interface's OSC-8 link opener), and no candidate on any platform invokes a shell — Windows goes through `rundll32 url.dll,FileProtocolHandler <url>`, whose command line is parsed by `CommandLineToArgvW`, where `&` is not special; macOS `open` and Linux `xdg-open` were already direct execs. The unit test asserts the Windows plan on *every* host, precisely because it is not otherwise executable where it matters. **It is not Windows-only in principle:** any launcher that puts a shell between the caller and the browser has this bug whenever the URL carries a shell metacharacter (`&`, `;`, `|`, backticks in a query, an unbalanced quote), and a new platform that "helpfully" wraps URLs in `sh -c` reintroduces it. The rule is the one above: prefer an argument list over a constructed string, always.

**The external editor follows the same rule (#121).** `external_editor` used to run `sh -c "{editor} {path}"`: no `sh` on a stock Windows box, and any space in the buffer path broke the command line. It now splits the editor command on shell words and spawns it directly with the buffer path as its own argv element (a uniquely-named `tempfile`, closed before spawning so the editor can open it on Windows too); only an unsplittable editor string falls back to the platform shell. One quoting rule survives, and it belongs to the user, not the code: quote a path with spaces inside `EDITOR`/`VISUAL` itself (`EDITOR="code --wait"`), the way a shell would.

The loopback OAuth listener works the same as on POSIX platforms; Windows has no unusual restriction on binding `127.0.0.1` locally.

**Scrollback, and why the main-screen renderer is the default (S1, 2026-10-02).** S1 ported pi's bottom-anchored incremental append contract (`tui-main-screen.ts`), so the main-screen renderer appends to the console's real scrollback rather than truncating history. The terminal's native selection, scrollbar, right-click paste, and Ctrl+V work everywhere by construction. The fullscreen (alt-screen) renderer remains the opt-in (`ui.fullscreen = true` or `/fullscreen`) for users who want app-owned selection and mouse capture. The per-mode mouse rules from TUI cycle 7 still hold (main-screen never enables mouse tracking; alt-screen emits exactly pi's set), and since 2026-10-04 (gh #35) the fullscreen renderer's viewport is a pinned dock over a scrolling transcript window with a virtual scrollbar and a jump-to-bottom indicator - the main-screen contract above is untouched by it. ADR-0037 carries the dated annotation.

**The pty capability on Windows (resolved 2026-09-29).** Windows pseudo-terminal support went through eight CI rounds, and those rounds were not wasted: verbatim `\\?\` working directories (cmd.exe reads them as UNC and refuses them), the console-close reader deadlock (now the documented end-of-stream flush), the pump busy-spin (now yields), and the `PtyChild::spawn` environment block all landed there. But the round that set out to fix the pseudoconsole attribute went the wrong way: it passed `UpdateProcThreadAttribute` the *address* of an `HPCON` instead of the HPCON value itself. `UpdateProcThreadAttribute` returns success either way, so the child was silently born on a fresh default console - `mode con` inside it reported 120x30 while the pty had been created 24x80 - and the output pipe received zero bytes. Microsoft's own sample and wezterm's `procthreadattr::set_pty` both pass the value, with `cbSize = sizeof(HPCON)`; that is what this code now does. The child's std handles are also explicitly invalidated (`STARTF_USESTDHANDLES` with `INVALID_HANDLE_VALUE`), because a parent with redirected stdio - a test harness, a daemon, an editor - otherwise leaks its own handles into the child and the output never reaches the pseudoconsole. A second, independent defect surfaced the moment the TUI rendered on ConPTY: `/exit` hung the process, because `sys::wait_stdin` reported readable unconditionally and the reader's blocking `ReadFile` could not be interrupted, so `ProcessTerminal::stop` hung joining the reader thread; `wait_stdin` now reports readable only when a key event is actually pending (it peeks the console records and consumes non-key ones, so `ReadFile` returns promptly), and `stop` retries `CancelSynchronousIo` before joining as a net. **A consumed non-key record is no longer discarded** (gh #323): the wait reports what it found, so a `WINDOW_BUFFER_SIZE_EVENT` - the console saying the user resized - reaches the reader loop's size check in the same iteration instead of waiting for its next tick. Only buffer-size records count; mouse and focus records are still ignored, because treating every non-key record as a resize would repaint on every mouse move. The unix side has no equivalent channel (a resize arrives as `SIGWINCH`), so `WaitOutcome::Resize` is constructed only by this backend. Relatedly, the two `SetConsoleMode` calls in `enable_raw_mode` now have their returns checked: a refusal used to vanish, leaving the app believing it was in raw mode while the console stayed cooked, and it now surfaces through `Terminal::raw_mode_error` with the handle name and the OS code rather than being swallowed by `.ok()`. All five previously-quarantined tests are green on a real console *and* on the hosted `windows-latest` runner, so the "hosted runner has no console" hypothesis was wrong - the fault was in this code all along, and no self-hosted runner is needed. The three pty tests keep their macOS `ENOTTY` ignore (tracked below); nothing else is ignored on Windows.

**Quarantine ledger (open 2026-10-02).** `cancelling_a_turn_interrupts_a_running_extension_call` (`lca-core/tests/hooks_dispatch.rs`, gh #548 phase 4's split of `hooks.rs`) is `#[ignore]`d on Windows under testing-plan section 13's rule: its *second* turn - a fresh cancel flag, the scripted provider, one completed conformance call - exceeded the 30-second hang guard twice on hosted runners (CI `36881146884` on 2026-10-01 and CI `36943299928` on 2026-10-02), while the first turn's cancellation assertions passed both times, the local suite stayed green on every run, and Linux/macOS never reproduced it. What is known so far: the fake provider releases its script lock at statement scope, the epoch trap does end the spinning call (turn one is prompt, which is why no spinner leaks), the blocking pool is not exhausted by one call, and the elapsed diagnostic the guard prints is the number the next observation will carry. No mechanism is named yet, so the fix is open - runner contention is the shape of the evidence, not a proven cause. The assertions themselves are untouched; un-quarantining is removing the `cfg_attr` on the test, and section 13's stale-quarantine clock starts at this date.

*Update (2026-10-06, #89):* the test is split in two. Turn one's cancellation assertions now run un-quarantined on every platform (`cancelling_a_turn_interrupts_a_running_extension_call` — they passed on Windows both times the hang fired, so the FR-CONC-1 contract is covered where the full test cannot run).
*Update (2026-10-07, gh #190):* turn one joined the quarantine (full `#[ignore]`, all platforms). The 2026-10-06 split assumed turn one was solid; a local full-suite run then showed the same late-trap shape in turn one itself (27s, non-cancelled return — the assertion, not the timeout). Four hits total, mechanism still open (epoch-bump absorption race under load is the hypothesis; the eliminated list is on #190). FR-CONC-1 has no running pin until #190 lands — that is what the tracking issue is for. The second turn keeps the quarantine (`a_fresh_turn_starts_clean_after_cancellation`) and its guard now prints an elapsed breakdown - provider calls served, sink events by shape, records durable - so the next firing names the stalled phase instead of just the total. No timing thresholds were added; the existing ones are unchanged.
*Update (2026-10-17, gh #276):* the second turn fired again on the hosted **Linux** runner for the first time (CI `39044027340`), and this firing is diagnostic where the earlier ones were not. The printed breakdown read `turn2_elapsed=30.000617101s provider_calls=1 sink_text=0 sink_tool_started=1 sink_tool_finished=0 sink_errors=0 sink_other=6 store_records=5`: turn two's provider answered once, the extension call *started*, and it never finished the `conformance` `ok` call. The healthy state for the same test is `provider_calls=3 sink_text=1 sink_tool_finished=1 store_records=11`, and it was reproduced green at both ends of the load scale — 16–20 s solo (`turn2_elapsed` 24–132 ms) and 72–83 s with three suites sharing four cores, SLOW but green. The absorb change that cycle shipped (#276) is not a candidate: `Usage::absorb` is `&mut` arithmetic on a per-call local (`usage` in `turn/stream.rs`, `turn_usage` in `turn/mod.rs`), it performs no I/O, takes no lock, and cannot await, and both accumulators are initialized fresh per turn and per call (`Usage::default()` at `turn/mod.rs:125` and `turn/stream.rs:164`), with the cancel path returning before `accumulate_usage` ever runs. The guards are therefore hang guards, and they were the second half of the problem: at 30 s they fired *after* nextest's 120 s `terminate-after` in CI (the job logged `SLOW [> 60s]`, then `TERMINATING [> 120s]`, then `TIMEOUT`), so the breakdown never got printed at the moment it mattered, and the panic could not end the process either — the spinning guest's thread, spawned by `ExtHost::blocking`, is never joined, so nextest's kill is what stored the failure. Both guards now sit at 110 s, just under the terminate, which buys a named failure and the breakdown line; nothing asserts on elapsed time, so no assertion was touched, and turn one's 3-second promptness bound (the real NFR-29-shaped claim) is unchanged. The test remains `#[ignore]`d on Windows; Linux and macOS run it. See gh #190 for the still-unnamed epoch-absorption race in the quarantined sibling.

**Non-quarantined load-only observation (2026-11-15, gh #598).** `the_picker_moves_to_a_local_preset_that_needs_no_key` (`lca-cli::e2e_terminal`, real tmux pane) failed **once** on `test (macos-latest, shell)` (CI `43587423453`, head `0d3bd053`): its assertion read `the picker closed:` and the captured pane was still the startup screen, so the `Down`-walk to the local preset had not landed when `Enter` went in. It is a **load-only** shape - the row drives a real terminal, its walk polls a fixed 4 x 100 ms per step, and the failure is a redraw the capture missed rather than a wrong screen. Receipts: **green three times on the same machine** immediately afterwards (solo 9.8 s, solo 9.4 s, and the whole 18-row `e2e_terminal` target 18/18 in 15.1 s), and the firing commit touched only `crates/lca-tui/src/widgets/image*`, which cannot reach a login picker. **Not quarantined**: one observation is not a pattern, and the house rule is that quarantining needs a tracking issue and a named mechanism. Nothing was loosened - the assertion, the settle and the poll windows are as the gh #447 row left them. If it fires a second time, the next step is the `wait_for_input_ready`/`wait_settled_for` primitives (a captured frame that has *settled* on the preset, not a fixed sleep per step), which is what `docs/testing-plan.md` section 14 already prescribes for this class.

**Quarantine ledger (closed 2026-09-29).** The Windows quarantine set opened on 2026-09-25 - the three pty tests plus the ConPTY real-terminal pair - is **closed**. All five are green on Windows, both on a console-attached machine and on the hosted `windows-latest` CI leg, after the pseudoconsole-attribute and reader-exit fixes above. No test is `#[ignore]`d on Windows any more, and no new quarantine was needed to get there. The stale-quarantine clock that started on 2026-09-25 stops here; the remaining ignores in the tree are macOS-only (`ENOTTY` on the pty allocation path) and are recorded in the macOS section below.

Two performance numbers do not meet their requirements on Windows, and the measurements are recorded here rather than hidden in a relaxed assertion on the other platforms. NFR-4 bounds instantiation of a precompiled extension at20 ms; the Windows CI measurements are a44.7 ms median on the first recorded run and a101.4 ms median after warmups, the spread itself pointing at endpoint scanning of freshly-created executable pages rather than at instantiation work. The test therefore asserts20 ms on Linux and macOS, where both pass continuously, and150 ms on Windows, with these measurements as its justification. NFR-29's cancellation bound of50 ms does hold on Windows now that the test's clock starts on a genuinely running call rather than during instantiation (the host's in-flight counter exists for exactly that), and it is allowed three fresh-engine attempts because the first observation on a loaded runner lands at56-60 ms when the spinning thread is unrunnable at the moment the epoch moves. On macOS it does not hold on hosted runners: three consecutive readings of74-84 ms, after earlier ones of56-60, the shape of a machine timesharing a compute loop on a sixty-to-eighty millisecond quantum - so the macOS bound in the test is120 ms against those recordings, and the requirement's50 stands unchanged on Linux and Windows. None of these relaxations leak into any other platform.

*Update (2026-11-07, gh #547):* one contributor to that 56-60 ms variance is gone, and
the history above is left as written. The turn used to spawn an OS thread that slept
1 ms and polled the cancel flag, then called `interrupt()` on its own tick - so the
observation waited for a poll period, and on Windows `sleep(1ms)` rounds **up** to the
~15.6 ms system timer quantum, i.e. a third of the 50 ms budget was spent before
Wasmtime saw the epoch move. Cancellation is now a callback registered on the
`CancelFlag` and invoked **inside `cancel()`, synchronously, on the cancelling
thread, before it returns** (`crates/lca-tools/src/cancel.rs`): 0 us, no poller, and
nothing left for the timer to quantise. The per-turn 1,000 Hz wakeup stream and the
`join()` that blocked an async runtime thread at every turn end are both gone; the
turn holds one RAII guard, which unregisters on drop so a hundred-turn session
accumulates nothing. The recorded thresholds above are unchanged - they were
measurements, and the point of this note is that they now measure less. Two honest
limits of the callback rule: a panic in a listener is caught so the remaining
listeners still run, which cannot act in this workspace's `panic = "abort"` release
profile, and the panic hook still prints before the catch fires.

What that is worth in the product, measured through a real tmux pane on this box
(headless cannot see a wakeup, so the pane is the only honest instrument): a turn
streaming deltas, counted by the process's own voluntary context switches over an
eight-second window. The 1 ms poller cost **4,579-6,525 switches on one thread**
across three runs; the callback costs **159-161**, and the extra thread the poller
needed is gone (`threads=8` while a turn runs, `9` before; zero at rest once the
turn ends). The remaining ~160 are the mock provider's half-second ticks and the
renderer's own waits, not cancellation. Escape-to-`cancelled` on the pane measures
159-205 ms over eight trials, against a 12-79 ms floor for a keystroke reaching the
screen through the same capture loop - so the observable latency is the harness's
sampling and repaint, not the cancel, and it is not comparable across machines.

**The second family of macOS elapsed-time bounds (2026-11-10, gh #588).** Three paragraphs up, the NFR-29 recording is a *requirement* bound — NFR-29's 50 ms, relaxed to 120 ms on macOS because the platform cannot reach it — and its per-platform shape lives in `crates/lca-ext-host/tests/nfr.rs`. This is a different family: three **ordinary test rows** whose wall-clock assertion was finer than the platform's scheduler quantum, so they were red on every hosted macOS run while the code they check was correct. Measured on CI run `42701900918` (the first run since the billing lock lifted), and identical on the same cycle's first commit, so the code was not the cause:

| row | macOS observation | bound before | bound now |
|---|---:|---:|---:|
| `lca-subscription::kit::a_cancelled_device_flow_returns_inside_the_cancellation_budget` (fake host) | **280.301 ms** | 150 ms | **500 ms** |
| `regressions …::a_native_cancel_lands_inside_the_device_code_wait` (real `Capabilities` engine) | **298.199 / 366.4 ms** | 250 ms | **500 ms** |
| `lca-ui::run_wake::tests::a_producer_wake_returns_immediately` | **55.227 ms** | `WAIT_TICK` 50 ms | **250 ms against a 1 s deadline** |

The rule, and it is the durable half: **a bound finer than the platform's scheduling quantum is not a bound on that platform.** The first two rows slice their wait 25 ms at a time and then observe the cancel, so 50–100 ms of real work plus one 60–80 ms quantum *is* the number; no bound between 150 and 400 ms can separate a working cancel from a broken one there, because the defect costs 5 s (5.02 s measured with the flag read removed) and one quantum is a sixth of the current slack. Both bounds therefore come from the **defect interval** rather than from the observation: 500 ms proves "did not wait out a 5 s interval" ten to thirty times over, on any scheduler, and it leaves room for a loaded runner to insert three quanta before drifting toward the interval it rules out. The Linux bounds (150 ms measured at 50.7/50.7/54.0, and 250 ms measured at 100.9/101.5/106.0) are untouched.

The third row is the one that needed a different fix, because widening the bound could not work: the fix's reading and the defect's overlap once a quantum is the size of the tick (the fix is 5 ms of sleep plus a quantum, the defect is 50 ms plus a quantum). So the row stops asking "did this beat the tick" and starts asking "did this beat the deadline it was given" — a **1 s** deadline, so a wake-less wait sits out 1000 ms while a producer's wake lands at 5 ms plus at most one quantum, and a 250 ms bound separates the two on every platform. Its own comment records the two mutants: dropping `waker.wake()` reddens the ordering assertion (`Tick` instead of `Producer`) rather than the timing one, and a 300 ms producer sleep reddens the timing assertion at 300.8 ms, so the two assertions keep the two halves of the proof. The general lesson, beside gh #489's: a fixture's own phases are the test's to choose, and a row that compares itself to a *shared* production constant inherits that constant's platform-specific jitter.

## The installers: per-platform behavior

`install.sh` and `install.ps1` (repository root; `docs/installation.md`) have three platform-specific behaviors worth knowing before editing either.

**Linux.** The asset is the musl target, so the binary has no dynamic dependency and runs on any distribution without a compatibility problem; the installer's own job is smaller here than elsewhere because `sha256sum` ships in coreutils on every target distribution — but a container without it is a hard failure by design (FR-INSTALL-2 refuses rather than skipping), not a skip. `$HOME/.local/bin` is *not* on `PATH` by default on many distributions (it is on most modern ones via `~/.profile`), which is exactly why the installer writes the marked block rather than assuming the directory works. The XDG split documented above is unrelated to this: the installer only ever touches the install directory and the user's own rc files.

**macOS.** Gatekeeper quarantines anything downloaded by a browser or by `curl`-based installers that inherit the quarantine attribute, so the installer strips it (`xattr -d com.apple.quarantine`, best effort, after checksum verification) — the honest position is in `docs/installation.md`'s security stance: binaries are unsigned in 0.6.x, the digest is checked first, and the attestation is the provenance path. `shasum -a 256` is the platform's hash tool (`sha256sum` does not exist by default on macOS), and the installer picks by capability rather than by `uname`, so a macOS box with coreutils installed still works. The rc target is `~/.zshrc` because zsh is the default shell since Catalina, but the installer still dispatches on `$SHELL`, not on the OS, since a Linux user running zsh deserves the same behavior.

**Windows.** Three separate things, each of which has bitten an installer before:

- *TLS.* Windows PowerShell 5.1 runs on .NET Framework, whose `ServicePointManager` default protocol list predates TLS 1.2 and is set by the machine's Schannel policy; the installer sets `[Net.ServicePointManager]::SecurityProtocol` to include `Tls12` **before** the first download, because afterwards the failure is an opaque `Unable to connect to the remote server` on a machine whose browser works fine.
- *Execution policy.* `irm ... | iex` needs **no** policy change and no `RemoteSigned` exemption: the execution policy governs *files on disk*, and nothing here is executed from a file — the text arrives in memory. The documentation says so explicitly because the reflexive advice (`Set-ExecutionPolicy RemoteSigned -Scope CurrentUser`) is a real, unnecessary change to a machine's security configuration, and telling a user to make one in order to install a tool is worse than the tool.
- *PATH.* The user `Path` is a registry value under `HKCU:\Environment`, not a file, and it is frequently `REG_EXPAND_SZ` containing `%SystemRoot%` entries contributed by installers that ran years apart. The installer reads it with `[RegistryValueOptions]::DoNotExpandEnvironmentNames`, remembers the value kind, appends its own directory only if neither the user nor the machine `Path` already contains it, writes back with the original kind, and broadcasts `WM_SETTINGCHANGE` — so a `REG_EXPAND_SZ` value is not silently downgraded to `REG_SZ`, an existing entry is not duplicated, and a terminal opened *after* the install actually sees the new directory. The rc-file discipline on POSIX and the registry discipline here are the same rule wearing different clothes: append one owned thing, never rebuild what you did not write.

## The web target

The `wasm32-wasip2` build, per FR-WEB-1, has no real filesystem, no real process table, and no raw sockets, because it's running inside whatever sandbox the browser or Node's WASM engine provides, not on an operating system of its own. Three capabilities behave differently here as a direct consequence, not as an oversight:

`fs` and `net` are entirely host-delegated, per FR-WEB-3: the embedding JavaScript application supplies the actual filesystem and network implementations, and the agent's own I/O backend trait, per ADR-0013, calls through to whatever the embedder wired up rather than making a real syscall. What an extension sees as `fs.read` or `net.request` in the web build is only as capable as the embedding application chose to make it.

`process` and `pty` have no meaningful implementation at all in this build; there is no process to spawn. A provider or tool extension declaring either capability in its manifest can still be installed, since the manifest and consent model don't need to know which build they're running under, but an attempt to actually call `process.spawn` or `pty.spawn` in the web build returns a permission error identical in shape to a denied grant, not a crash, and an extension author who wants their component to work in both native and web deployments should treat these two capabilities as potentially unavailable rather than always present.

`net-local`'s premise, reaching another device on the user's own network, runs into the browser's own Private Network Access restrictions: a web page generally cannot reach a private IP address from JavaScript without the browser's own separate permission flow, independent of anything this project's capability model grants. A local-provider extension such as LM Studio or Ollama's, which assumes `net-local` reaches a real local server, is not expected to function inside a browser-embedded build for this reason, and this is a browser platform constraint, not a gap in this project's own design.

The extension hosting mechanism itself, sibling instantiation through `jco` rather than a nested WASM runtime, is specified in ADR-0018 and is the reason none of the above needed a second, WASM-specific capability-enforcement implementation: the same capability grants and the same host imports apply, only the transport connecting an extension's calls to the host's implementation of them differs between the native and web builds.

## Suspend: `Ctrl+Z` exists on unix and says so on Windows (gh #403 gap 4)

`SIGTSTP` is a unix job-control signal with no Windows equivalent, so `Ctrl+Z` cannot suspend LCA there. pi's answer is a one-line status ("Suspend to background is not supported on Windows"); LCA does the same rather than leaving the key dead, because silence reads as a broken key. Nothing is torn down on the refusal: the alternate screen is only left once a suspend is certain, so a platform that cannot suspend keeps its frame.

On unix the sequence is `leave` (restore the tty) → `stop` (drop the reader) → `SIGTSTP` to the **process group** (so a shell child stops with us) → `start` → `enter`. `TSTP` rather than `STOP` because it is catchable and a parent `strace`/job-control layer can see it; and the group rather than the pid because a suspend that leaves a child in the foreground is worse than no suspend.

Two guards pi carries are **not** ported, and the reason is structural rather than an omission:

- pi keeps a Node timer alive across the stop (`setInterval(…, 2**30)`) so the event loop has a ref'ed handle while suspended. Rust threads are not tied to an event loop, so the suspend is a process stop and holds by construction.
- pi shields `SIGINT` while suspended so `Ctrl+C` in the shell does not kill the backgrounded process. LCA's reader thread owns input under raw mode, and the signal disposition is the shell's while we are stopped - if a live `Ctrl+C` in the background turns out to kill a suspended LCA, that is a real defect and the shield is the fix; it is listed in the #403 gap 4 report as still owed rather than assumed fixed.

## A dead terminal

A closed pty (`POLLHUP`, or a read that returns 0) and a `SIGHUP` are the **same event from two directions**, and both exit 129 after flushing. `poll` reports `POLLHUP|POLLERR|POLLNVAL`, which outranks `POLLIN` - a hung-up pty is also "readable", so a classification ordered the other way sees a permanent key and the reader spins. Windows has no `SIGHUP`: a closed console surfaces as a failed `ReadFile`, which the reader's end-of-file arm classifies the same way.

## Input reassembly: what times out into a keystroke, and what drops (#368)

`StdinBuffer` accumulates until a sequence is provably complete, and the 50 ms timeout is what decides the ambiguous cases. Three rules, and they are deliberately different:

**An introducer with no terminator is not a sequence.** A legacy terminal sends `Alt+]` as `ESC ]`, `Alt+P` as `ESC P`, `Alt+_` as `ESC _` - the OSC/DCS/APC introducers with nothing after them. Waiting for a terminator that is never coming means the key and **every keystroke after it** glue into one unreadable event, and the session looks frozen. So on expiry the two introducer bytes are delivered as the meta pair and the remainder is **reparsed** as input (so a `Ctrl+M` in what was typed is still Enter, not two literal characters). `ESC [` is deliberately **not** in this set: a bare `ESC [` is `ctrl+alt+[` *and* the first two bytes of a half-arrived arrow key, so splitting it would trade one ambiguity for another.

**A reply head we asked for is dropped, with its tail.** `OSC 11`/`OSC 7501`/`DA1`/Kitty-flags heads that have *committed* (`ESC ] 11 ;`, `ESC [ ?`) are discarded rather than typed, and a bounded content window keeps the tail from being typed either (#366). `DCS > |` (XTVersion) and `APC G i=1` (Kitty graphics) join that table here - they were the gap.

**The terminator table.** `BEL` and `ST` (`ESC \`) end an `OSC`; **`ST` only** ends `DCS` and `APC`, because `BEL` is an ordinary body byte inside those strings. A `CSI ?` reply ends on its own final byte - and since a head that has not reached one could still become any of `u`/`c`/`n`, any of the three closes the tail window.

**Two ambiguous heads are the accepted cost**, pinned as rows rather than implied: a reply split at the *bare* `OSC`/`DCS` introducer is indistinguishable from the corresponding Alt key plus typing, and is therefore recovered as keystrokes. pi draws the line in the same place. The reported splits - mid-body, or one byte past the introducer - are closed.

## Streamed text: a read boundary is a byte boundary, not a character one (gh #405 gap 4)

A pipe read ends wherever the kernel's buffer ends, so a UTF-8 codepoint can straddle
two reads. Every `String::from_utf8_lossy(&chunk)` therefore invents a `U+FFFD` for a
character that is not invalid, merely late — and once the bytes are decoded that way the
information needed to fix it is gone, because the leading bytes were replaced rather than
kept.

Measured on this box, before the fix, on the two paths that read bytes:

| path | input | before |
|---|---|---|
| tool output (`lca-core`) | `"日本語の日本語の日本語の日本語"` in 8-byte reads | **10** × `U+FFFD`, painted in the bash card |
| keyboard (`stdin_buffer`) | `"日本語"` split after byte 2 | `"��本語"` |

Both now decode through `lca_protocol::Utf8Stream`, which holds a partial codepoint until
the next read completes it. Three states, deliberately distinguishable:

- **incomplete** — a valid prefix of a codepoint. Held, silent, and `has_pending()` says so
  (a UI that shows a spinner cares). Never replaced: this is the case the bug got wrong.
- **invalid** — a byte that cannot begin any UTF-8 sequence, or a continuation that
  contradicts its lead. Replaced **once, visibly**, and decoding continues after it, so
  one bad byte cannot swallow the rest. Silently dropping it would hide a real fault.
- **incomplete at end of stream** — a transfer cut mid-codepoint. Reported **once** by
  `finish()`, because nothing can ever complete it and losing it invisibly is worse.

`str::from_utf8`'s own `valid_up_to` / `error_len` are the authority for which state a
buffer is in, so there is no hand-rolled byte-class table to drift from the standard.

**The text accumulator was never the problem.** `lca-ui`'s transcript appends `&str`
deltas to a `String` and renders the whole accumulated text, so a document with a
true-colour SGR, an OSC 8 link, CJK and an emoji renders byte-identically in 1 chunk and
in 200. A guard aimed at the accumulator would have passed while the card painted
replacement characters, which is why `tests/regressions/405-stream-chunk-boundaries.rs`
pins the decoder and the equality rather than the string building.

**And an incomplete *escape* is a different case from a split one.** A sequence split
across reads reassembles in the accumulator; what needed handling is a sequence that is
incomplete **in the final text** (a truncated `\x1b[38;5` at end of stream), which is why
`sanitize_incomplete_escapes` drops a trailing tail rather than the renderer tolerating a
broken one — a truncated escape can never be repaired once its bytes are gone, so the
buffer is the only place the information still exists. Two defects in that sanitizer
were found by measuring it, not by reading it: a **backward** scan mistook an OSC's `ST`
terminator (`ESC \`) for an unterminated opener and dropped every character after it
(`"a\x1b]0;title\x1b\\b"` lost the `b`; a bare `ST` ate the rest of the row), and past
its 4 KiB bound it returned the text **unchanged**, letting a raw `ESC` reach the terminal
— the control-byte injection the function exists to prevent. It is a forward scan now,
and past the bound it drops the one control byte and keeps the payload.

## A pty's end-of-stream is a platform-specific claim (gh #535)

`pty.read` answering `None` means "the session ended" — and the hard part is that a pty's
master reports its end on terms of its own. The child exiting does **not** mean the bytes it
wrote have been drained, and the platform that keeps proving it is macOS: there a
*drained* master stays **permanently readable** (a read answers zero bytes), so `poll`
reporting `POLLIN` cannot distinguish a late tail from the end of the stream.

That combination produced the same failure twice, in `pty_spawn_delivers_terminal_output` on
macOS CI, as `exit 0` with an empty result — the host having declared the session drained
while `echo pty-ok`'s output was still in flight. gh #477 met the shape first and answered it
with **one** 4 ms grace window after a confirmed child exit; that is a guess, and 4 ms is
what the guess was. The phase is now:

| aspect | value | why |
|---|---|---|
| attempts | `DRAIN_ATTEMPTS = 20` | a bound you can multiply, unlike a single sample |
| step | `DRAIN_STEP = 5 ms` | a real window per attempt |
| ceiling | 100 ms | spent **once** per session, at shutdown, never in steady state |
| who pays it | only a platform whose drained master still claims readability | macOS |
| who does not | a platform whose kernel answers "nothing more is coming" | Linux sets `POLLHUP` and never `POLLIN`, so the zero-timeout probe answers `false` and the loop returns at once |

The sleep is **unconditional in that branch on purpose**, which is the exact opposite of
gh #477's bug: that one re-read with no wait at all and ran at a measured 614,104 calls per
second. A branch that only slept when a read returned no data would spin on macOS, because
"readable and empty" is precisely the state there; a branch that sleeps whenever the master
claims readability costs at most 100 ms where the kernel cannot be trusted and nothing where
it can. `tests/regressions/535-pty-drain-window.rs` pins all three properties structurally —
attempts, the wait, and the immediate exit — so a future simplification cannot quietly restore
either the one-shot window or the spin.

## Cross-cutting: what "works everywhere" actually means here

A requirement stated as applying to all platforms, in the requirements and design document, means the behavior is identical from the perspective of the agent's own logic and the extension ABI; it does not mean the underlying mechanism is identical. The `fs` scope-escape check is one function with one contract everywhere and two different, platform-specific canonicalization implementations underneath it. The cancellation flow is one behavior, kill everything a turn started, with a process-group kill on POSIX and a Job Object kill on Windows underneath it. Extension authors and first-party contributors alike should write to the contract, not to a mechanism, and platform-specific tests, not just platform-specific code, are what keeps the two from drifting apart silently; the Windows CI job being non-optional, per the testing plan, is the concrete enforcement of that principle rather than a formality.

## Real-terminal tests: platform gating

The real-terminal tests (testing plan section 14) drive the TUI in an actual terminal, so a leaked secret, a stale resize row, or a hostile escape sequence is visible where a virtual-buffer test cannot see it. The harness is platform-native: Unix runs under `tmux` and asserts through `capture-pane`; Windows will run under a pseudo-console (ConPTY), the same mechanism the `pty` capability uses, and assert through the console buffer.

The gating is part of the contract, not an afterthought. `tmux` has no Windows build, so the Unix tests carry `#[cfg(unix)]` and probe `tmux -V` at runtime: a machine without `tmux` skips them with a named reason and never reports a failure for the tooling's absence. Windows builds simply do not contain the Unix tests, and the ConPTY tests, when they land, are `#[cfg(windows)]` in the same way. The Linux CI job carries `tmux` preinstalled and runs the Unix set; the macOS job installs it via Homebrew and every CI leg exports `LCA_REQUIRE_TMUX=1`, which fails a test outright instead of skipping it when the harness is absent (#104). Pane driving may still be quarantined if it proves flaky (testing-plan section 13), so an absent run is always a documented state rather than a silent gap. One macOS-only trap for tests that read the grant store: scratch directories hang under symlinked `/var` while the store keys the canonical `/private/var` path, so assertions must canonicalize exactly like `canonical_key` does (#104).

## Known macOS failure (tracked)

Five pty tests fail on macOS CI with `ENOTTY` ("Inappropriate ioctl
for device") or the panel session never coming up behind that same
allocation path: `pty_spawn_delivers_terminal_output`,
`pty_spawn_delivers_program_output_and_exit_code`,
`pty_forwards_keystrokes_both_ways`, and the two ui-example panel
tests (`all_four_regions_render_and_the_panel_session_starts_on_demand`,
`typing_into_the_panel_reaches_the_program_and_comes_back_as_data`).
They are `#[ignore]`d on macOS alone - green on Linux and Windows,
and the rest of the macOS suite runs green: the panel tests joined the
list only after the e2e data-directory fix left them as the sole
macOS failures. Under the project's stated platform priority (Linux,
then Windows, then macOS) the likely shape is a macOS-specific ioctl
on the allocation path, which needs a machine with a terminal attached
to diagnose; until then the ignore is visible at each test site rather
than the tests being deleted, and this entry is their tracking record.

*Attempt (2026-10-06, #88):* the ignores are lifted and the allocation
order changes - `TIOCSWINSZ` now runs after the slave is opened, not
before. No reference implementation sizes a master with no open slave,
and a not-yet-attached master is the one shape macOS may refuse with
`ENOTTY`; the reorder is harmless on Linux and Windows either way.
macOS CI judges: green closes this entry, red reverts the un-ignore
(never a red main) and the failure log lands here as new evidence.

*Closed (2026-10-06, #88):* all five tests green on the macOS leg
with the ignores lifted, four consecutive CI runs (`37350432151`
through `37360085042`). The differing order is the only change on
that path, so it stands as the mechanism: size an attached pair.
The stale-quarantine clock that started on 2026-09-25 stops here.
