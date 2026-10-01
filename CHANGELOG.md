# Changelog

Notable changes to LCA. Versions follow semantic versioning for the product;
the `lca:ext` ABI version is independent and is printed by `lca --version`.
Dates are UTC.

## [Unreleased]

### Added

- **The transcript and chrome use color the way pi does.** User messages render as a full-width `userMessageBg` band with the content padded inside it; tool cards carry their state's background (`toolPendingBg` while the call is in flight, `toolSuccessBg` when it settled, `toolErrorBg` when it did not); `[type]` headers (`[session in …]`, `[compaction] …`) take pi's `customMessageBg` band with the label in `customMessageLabel`; a picker's selected row is `selectedBg` on accent text; the footer paints the thinking level with its own `thinking*` role and the context share by threshold (warning above 70%, error above 90%). The theme's role table was already pi's; the renderers now spend it.
- **The separator row carries pi's spinner-in-the-border**: `── ⠴ Working ─────…` while a turn runs (accent spinner, muted `Working`), `Retrying (n/m) in Ns…` counting down through a provider backoff (warning spinner), and plain border dashes at rest. The row is always exactly one terminal row, fills the width, and animates on the interface's own tick - an idle interface never repaints. The dashes carry the thinking level, the way pi colors its editor border (`thinkingOff`, the same darkGray as `borderMuted`, when no level is set).
- **Code blocks are syntax-highlighted.** A fence that names a language the theme knows (rust, javascript/typescript, python, go, c/c++, java, sh/bash, sql, json, yaml, toml - and their usual aliases) paints pi's nine `syntax*` classes: comment, keyword, function, variable, string, number, type, operator, punctuation. A fence with no language, or one the highlighter does not know, keeps pi's fallback - the whole block in `mdCodeBlock` - and language auto-detection stays off, for pi's stated reason (it colors prose). The grammars are hand-written token classes, not a new dependency.
- **The `shell` tool runs a real shell, chosen by a documented ladder.** `shell.tool` (`auto`, `bash`, `pwsh`, `powershell`, `cmd`) and `shell.path` (an exact interpreter) pick it; `auto` on Windows finds Git Bash by its install location - not by the `bash` on `PATH`, which is usually the WSL stub and silently changes what every path means - then `pwsh`, then `powershell`, then `cmd`. A configured interpreter that cannot be found fails loudly with the locations searched. The model is told which interpreter it got and how its dialect reads. On Windows every command now travels in a per-call script file (`.sh`/`.ps1`/`.cmd`), so quotes, newlines, and metacharacters reach the shell exactly as written: `echo "double"` is no longer `echo \"double\"`, and a multi-line command runs all of its lines. POSIX behavior is unchanged. ADR-0041; the ladder, its traps, and the transport table are in `docs/platform-notes.md`.
- **Yolo mode**: `--yolo` or `permissions.mode = "yolo"` answers every permission prompt "always, for this exact action's pattern" - the pattern is persisted and the session log records the same `permission` entry a human answer writes, so approve-everything never means forget-everything. The footer shows a `YOLO` line while it is on and the session opens with one explanatory line. Explicit deny rules still deny, and the mode is never persisted for you. ADR-0042.
- **Read-only tools outside the workspace no longer prompt** (`read`, `list`, `glob`, `grep`); a deny rule still refuses them, and writes/edits outside the workspace still ask.

### Changed

- **Style functions reset only their own SGR channel** (`ESC[39m` for a foreground, `ESC[49m` for a background, the decoration's own close code for a decoration) instead of a full `ESC[0m`, which is pi's `fg()`/`bg()` semantics: a styled run inside a background band keeps its color, and a nested style cannot blank the one around it. Dialog rows still reset explicitly around the frame, so the cycle-8 bleed fix is untouched and its assertions are unchanged.
- **A theme change repaints the transcript.** The per-entry render cache is keyed by width, so `/theme`, its live preview, and a detected-scheme swap now drop it instead of serving rows painted in the previous palette.
- **`ui.fullscreen` defaults to `true` again**: the interface opens in the fullscreen (alt-screen) renderer, and `ui.fullscreen = false` opts into the terminal's own scrollback. Real Windows use showed ConPTY does not reliably restore scrollback under the main screen, which is what the previous default used. ADR-0037's second annotation.
- **Thinking runs show a short snippet by default** (`ui.thinking = snippet`): the first three non-empty reasoning lines plus a `… +N lines` marker, with the expand key overriding the latest run in place. `full` and `hidden` remain as settings.
- **The agent's data and configuration live in `~/.lca` on every platform.** Sessions, extensions, credentials, grants, state, temp, themes, `ui.json`, and `config.toml` all sit under one home dot-directory instead of the platform-specific ones (`~/.local/share/lca` and `~/.config/lca/config.toml` on Linux, `~/Library/Application Support/lca` on macOS, `%APPDATA%\lca` on Windows). **There is no migration code.** The old directories are untouched: copy the one you want across yourself, for example `cp -a ~/.local/share/lca/sessions ~/.lca/` on Linux, or `xcopy /E /I "%APPDATA%\lca" "%USERPROFILE%\.lca"` on Windows. `docs/platform-notes.md` names each old path.

### Added

- **Install with a one-liner.** Linux/macOS:
  `curl -fsSL https://raw.githubusercontent.com/misaalanshori/lca/main/install.sh | sh`;
  Windows (PowerShell 5.1 or pwsh):
  `irm https://raw.githubusercontent.com/misaalanshori/lca/main/install.ps1 | iex`.
  Each one picks the release asset for your platform, verifies it against
  `artifacts.sha256` **before** moving anything into place (a mismatch
  refuses and leaves your existing binary untouched), installs to
  `~/.local/bin` or `%LOCALAPPDATA%\lca\bin`, and puts it on PATH - one
  marked rc block on POSIX, an append-only user-`Path` entry that preserves
  the registry value kind on Windows. Running the same one-liner again is
  the update (`lca 0.5.1 -> 0.5.2`); `--uninstall` / `-Uninstall` removes
  the binary and every PATH entry it added, leaving pre-existing rc files
  byte-identical. Pin a release with `--version v0.5.2` / `-Version v0.5.2`;
  mirror or hermetic runs use `LCA_BASE_URL` / `-BaseUrl`. The full spec,
  the manual install path, and the security stance are in
  `docs/installation.md` (ADR-0040).
- **`LICENSE`** (Apache-2.0, as `Cargo.toml` has always declared) and
  **`SECURITY.md`** (the private-reporting contact that
  `docs/release-policy.md` already pointed at).

### Changed

- **The README was rewritten claim by claim against the tree**: install
  first, a quickstart carrying a transcript from a real terminal, the
  extension/capability/permission model with links to the ADRs that own
  it, the ten pipeline gates, and the six release assets with their
  verification commands. Truth-up fixes found while checking it: the ADR
  count (39 records, 0001-0040), a duplicate row in the ADR index, and
  three provider profiles (`codex`, `lmstudio`, `ollama`) that described a
  source tree which does not exist - they are now marked as
  specifications, not installable extensions.

## [0.5.2] - 2026-09-30

### Fixed

- **The Windows pty/ConPTY path works on a real console.** The
  pseudoconsole attribute was handed the address of an `HPCON` instead of
  the value itself, so every pty child was born on a fresh default console
  and its output pipe stayed empty; the child's std handles are now
  invalidated so a redirected-stdio parent cannot leak its handles into it
  either.
- **`/exit` no longer hangs on Windows.** The input reader could not be
  interrupted out of a blocking `ReadFile`, so shutdown hung joining it;
  `wait_stdin` now reports readable only for a real key event (non-key
  console records are consumed) and `CancelSynchronousIo` unblocks the
  read before the join.
- **The `\\?\` verbatim path prefix no longer appears in displayed paths**
  (the session-start notice showed it).

The five previously-quarantined Windows tests are green on a real console
and on the hosted Windows CI leg.

## [0.5.1] - 2026-09-29

A patch on the 0.5 train: **no interface changes** (`lca:ext` stays
`@0.5.0`, every manifest stays `abi = "0.5"`). It carries the commits that
landed after the `v0.5.0` tag, including a terminal-protocol fix the
published 0.5.0 binaries do not have.

### Fixed

- **A malformed terminal color reply could panic or misbehave.** The OSC 11
  background-color parser passed an unbounded channel to the hex decoder, and
  `16u64.pow(len) - 1` overflowed at 16 hex digits. The reply is untrusted
  terminal input - any process writing to the tty can send it - so the parser
  now caps a channel at xterm's 8 digits. Guard: `tests/regressions/33`.
- **Ctrl+J now inserts a newline.** Submit was checked before the newline
  binding, and a bare LF matches `enter` in a legacy terminal, so Ctrl+J
  submitted the prompt instead of newlining - and with it the documented
  fallback for a terminal that cannot report Shift+Enter was dead too. The
  newline check now runs first (pi's order), accepts pi's full spelling set
  (`\n`, `ESC CR`, `ESC [13;2~`, any ESC+CR), and a `\` typed before Enter
  inserts a newline instead of submitting.
- **The app-level keys go through the keybinding registry.** Ctrl+O/Ctrl+T/
  Ctrl+R/Ctrl+P, the escape/Ctrl+C pair, the follow-up/dequeue pair, and the
  prompt jump were raw key checks, so an embedder that rebound them was
  ignored and `/hotkeys` could not list them.
- **Markdown matches pi's shapes.** h1 underline, `- ` bullets, literal
  `[x]`/`[ ]` task markers, ordered-list renumbering, and the assistant's
  one-column left margin.
- **The editor coalesces typed runs into one undo unit**, and consecutive
  kills accumulate in the kill ring (forward appends, backward prepends) so
  one yank restores the run.
- **`/settings` and `/thinking` no longer disagree.** The `/thinking` pick
  persists to the user config file (the same comment-preserving writer the
  `/theme` pick uses), and `/settings` shows the resolved value with its
  source - session pick, then config file, then `unset (provider default)`.
  Both surfaces agree in-session and across a restart.
- **An unknown context window reads `ctx ?`** in the footer instead of a
  fabricated `ctx 0%`.
- **The footer names the login preset** (`opencode-go`) when one was stored,
  falling back to the extension name for a directly-configured provider.

### Added

- **A shared hint row on every picker overlay** (`↑↓ move · enter apply · esc
  close`, plus `type to filter` on the searchable ones). A picker owns the
  keyboard while open, so a slash command typed into one lands in its search
  box; the hint says so instead of leaving it surprising.

### Notes

- A previous cycle also verified against `mimo-v2.6-flash` alongside the
  mandated model (`deepseek-v4.1-flash`); the record says so honestly. That
  was a deviation from the model policy and is not repeated.
- Two known gaps in the interface remain, recorded for a future cycle and
  deliberately not built here: markdown **syntax highlighting** and the edit
  tool's **diff card**. Both have their theme roles already; pi renders both.

## [0.5.0] - 2026-09-29

The TUI renovation release. Cycles 2–4 rebuilt the interface on LCA's own
terminal engine (pi's design, ported) and this tag closes the chapter: the
post-review findings are fixed, the theme is the full pi vocabulary, the
grant store is visible, and the code is under the house's structure rules.
It carries **`abi 0.5`** — a **relabel** of the 0.4 line: no interface
bytes changed, exactly as the 0.4 train carried 0.2's changes forward.

### Added

- **Steering.** A prompt submitted while a turn runs queues instead of being dropped: `Steer` joins the turn's input at the next model-call boundary, `FollowUp` (Alt+Enter) auto-runs at turn end, the pending band and the footer show the queue, and an aborted turn returns it to the editor (ADR-0038, FR-CORE-11/12).
- **The interaction pack.** `!`/`!!` shell mode, an external prompt editor (Ctrl+X Ctrl+E), a runtime `/fullscreen` toggle, `/hotkeys`, Alt+Up/Down prompt jump, and Ctrl+R transcript search (FR-UI-10/11/12/14/15/21).
- **The visibility pack.** A `/theme` picker with live preview and restore-on-cancel plus `COLORFGBG` dark/light detection, a visible keyboard-interruptible permission auto-approve countdown, and a `/session` stats view (FR-UI-17/18/19).
- **`/tree` and `/fork`.** Browse session branches and fork a new one at any user message (FR-UI-16).
- **Cycle 3: the thinking level (R1).** A `thinking` config key with pi's level vocabulary, carried on every request as `reasoning-effort`; a `/thinking` picker with cost/latency descriptions and the level in the footer (FR-UI-20).
- **Cycle 3: `/resume` (R2).** A searchable session list with pi's grammar (`re:<pattern>`, `"quoted phrase"`, AND terms).
- **Cycle 3: in-place session switching (R3).** `/tree` and `/resume` switch the live session and rebuild the transcript from its log.
- **Cycle 3: async `!`/`!!` (R4).** Shell commands stream on a worker thread; Escape cancels the process tree.
- **Cycle 3: selectors (R9).** A searchable `/model` picker and a `/settings` view over the real `lca-config` keys.
- **Cycle 3: real images (R5).** An image renders through the terminal's kitty/iterm2 graphics ladder, with a legible placeholder on terminals without graphics; reading an image file returns image content through the tool result (FR-UI-13).
- **Cycle 3: verified clipboard and edge auto-scroll (R6).** Copy-on-release prefers a native clipboard command and reports honestly when only OSC 52 ran; a selection drag on a viewport edge auto-scrolls.
- **Cycle 3: OSC 11 dark/light detection (R10).** The interface queries the terminal's background color and color-scheme preference at startup.
- **Streaming-tolerant markdown.** Tables wait for an intact separator row, partial pipes render as text, unpaired inline markers stay literal, and the code-block border caps at the content width (FR-UI-7/8).
- **Cycle 4: the full theme vocabulary (S5).** The ~50 pi theme roles replaced three hard-coded palettes behind the same accessors; `ui.theme` takes `auto`, a built-in (`dark`/`light`/`plain`), or a custom theme file at `<config>/lca/themes/<name>.toml` (`<name>.light.toml` / `<name>.dark.toml` for a scheme pair). A file overlays only the roles it names, resolves a `vars` table, and accepts `#rrggbb`, a bare 256-color index, or `""`; an invalid file keeps the last-good palette and says why, and `/theme` lists custom themes with the active one marked.
- **Cycle 4: the grants view (S8).** `/grants` shows the project's grants in the store's own granularity: the install-consent group (extension enablement and the approved proposal set, whose revoke path is `lca ext disable <name>`) and the ad hoc group (shell/file patterns and `net` grants, revocable in place). `/settings` points at it.

### Changed

- **Cycle 4: the god files are gone (S1/S2/S3).** `lca-cli/src/tui.rs` (1,697 lines, a ~1,000-line `run`) is a composition root (`tui/` with `run` orchestration, options/commands, hooks, login, runner, display); `lca-core/src/lib.rs` (1,579) is `assemble`/`compact`/`turn`/`lib`. No file in the workspace is over 1,200 lines, the panic lints run workspace-wide, and every lock helper is poison-tolerant.
- **Cycle 4: custom themes are TOML**, the project's one configuration format (ADR-0036 addendum).

### Changed

- **The interface runs on the ported pi widget stack.** `lca-ui`'s `Chat` owns the engine's `Editor`, transcript, footer, and theme; the completion menu is live (commands, arguments, file paths); modals composite over the viewport. `crossterm` and the interim key-event bridge are gone from `lca-ui`.
- **`lca-tui` files are split under the 1,200-line ceiling** (`engine/text/{ansi,osc8}.rs`, `engine/keys/{legacy,kitty}.rs`); the keybindings global singleton is deleted and byte-slice indexing can no longer panic.
- **Cycle 3: the transcript collapses by default.** Tool cards are one line with per-tool arguments (Ctrl+O expands); thinking runs hide behind one dim line (Ctrl+T expands) (R8).
- **Cycle 3: the editor gained sticky-column motion and jump mode** (Ctrl+]/Ctrl+Alt+]) (R7).
- **Cycle 3: dead code out.** The unused `RenderScheduler` is deleted; `SteerQueue` carries an in-code justification; `lca-tui`/`lca-ui` warn on non-test `unwrap`/`expect`/`panic` (R12/R13/R17).

### Fixed

- **Cycle 3: cancelling the permission auto-approve countdown** kept the modal but dropped its responder, so a later Allow/Deny was a no-op (R11).
- **Cycle 4: the overlay and code-block frames** drew a left-only border (no top-right or bottom-right corner); both are full frames now.
- **Cycle 4: link URLs vanished under tmux** because OSC 8 was always emitted; the interface now follows the terminal's hyperlink capability and falls back to `text (url)`.
- **Cycle 4: the context-use meter** never showed because its cells were never populated.
- **Cycle 4: three silent truncation casts** (the scroll offset past `u16::MAX`, image rows at `u32`, and the wait timeout's `u64 as i32`) now saturate explicitly.
- **Cycle 4: an empty-id model** from a provider that answered a model probe unparseably left the session with no model; it now falls back like an empty list does.

## [0.4.0] - 2026-09-27

The backlog release: the last four undriven journeys, a build gate for every
release target, and the code-quality cleanup. It carries **`abi 0.4`** - the
ABI label now tracks the product minor (ADR-0028's second annotation), and
`0.3.0 / abi 0.2` is the documented transition artifact.

### Added

- **A release-target build gate.** `scripts/release-targets-check.sh`, run
  by a new CI job, checks all six release targets compile on every push -
  the last surface (after the fuzz workspace and the wasm components) that
  nothing built until release time.

### Changed

- **The ABI label moves to 0.4.** The WIT packages, the first-party
  manifests, and `lca-ext-abi`'s `ABI_VERSION` all read 0.4. A component
  still declaring `abi = "0.2"` is refused at the manifest check with a
  message naming the accepted `0.3..=0.4` window, not at a link error.
- **The two god-files are modular.** `lca-ext-host/src/lib.rs` (was 2,706
  lines) and `lca-tools/src/capabilities.rs` (was 2,329) are split by
  concern; no file exceeds ~1,200 lines.
- **One mutex-lock helper per crate.** The 56 scattered `lock().expect()`
  sites call a single `lock<T: ?Sized>(&Mutex<T>)`; panic semantics are
  unchanged.
- **Typed errors at the remaining stringly seams:** `lca-provider`'s schema
  validator, `lca-cli`'s manifest and secret paths, and the `completion`
  capability's backend.

### Fixed

- **A data-only extension could never be updated.** Its version identity
  was the digest of an empty component, so a new skill pack compared equal
  to the old one and `ext update` answered "up to date" forever.
- **`ext disable` did not stop a skill pack.** The host-side skills merge
  read the install tree without consulting per-project enablement.

## [0.3.0] - 2026-09-26

The finish-it release: no new surface. Every feature the last two cycles
added got its face, and what driving found in the way got fixed.

### Added

- **The `/login` picker.** `/login` with no argument lists every enabled
  provider extension's login options - the presets, and always the host's
  own "Custom endpoint..." - in a scrolling list. Choosing a row runs that
  option's field list, one prompt at a time. A key is masked and never
  renders; a base URL or a model id shows as typed (ADR-0033).
- **`/login <provider>`** scopes the picker; **`/login <option-id>`** skips
  it, so the same journey is drivable from a script.
- **Named custom endpoints.** `<config>/provider-presets.toml` merges the
  user's own presets with the extension's (D1's override layer).
- **`GET /models` discovery.** After a login the provider extension queries
  the endpoint's model list; `/model` offers what comes back, and the
  preset's curated short list is the fallback when it cannot be reached
  (D2).

### Fixed

- **The picker had no presets at all.** The bundled native extension's
  `resources` source was never set, so `resource_read` found nothing and
  the 19 presets cycle 4 shipped were dead weight.
- **A picker with many choices truncated its last rows**, making
  "Custom endpoint..." unreachable. It now scrolls with the cursor in view
  and shows `[n/total]`.
- **A bare release build produced no `lca` binary** again: the workspace
  `default-members` fix did not reach every build line. Regression 18.
- **A forked session listed as `0 messages`**; a fork now counts its
  resolved history. Regression 16.
- **A compaction summary read as an untrusted user note.** It now carries
  the agent's own self-describing framing, so a resumed model reads its own
  memory. Regression 17.
- **The nightly fuzz workspace did not build.** Two fuzz targets had
  drifted from the API they fuzz - one would have false-crashed on any
  valid data-only package. All four build again, and a `cargo check` of the
  fuzz workspace now runs in CI so a parser change cannot break the nightly
  silently. Regression 19.
- The unknown-provider message names `lca ext enable` alongside
  `lca ext install`, so the state where the only provider is disabled is
  escapable. Regression 20.
- Exhausted retries say so; a burst of pasted input redraws once.

### Changed

- `/login`'s secret prompt is per-field: only a key is masked.

## [0.2.0] - 2026-09-26

The dogfood release: LCA was driven like a real developer for a full cycle,
and everything found in the way was fixed. Cycle 2's features, which had not
been released, are in here too.

### Added

- **Typed image content.** A message's content is a list of blocks
  (`text` or `image`); `/attach <path>` in the interface and `--attach <path>`
  headless stage an image, stored content-addressed and owner-only, and a
  vision-capable provider receives the bytes (ADR-0029).
- **`lca session gc <id>`**: delete attachments in a session's fork tree that
  no resolved record references.
- **`lca ext enable <name>` / `lca ext disable <name>`**: per-project
  enablement, which the loader already read but no command reached
  (FR-PROV-9).
- **`lca:ext` ABI 0.2.** The window is a single in-place development line;
  the re-freeze is a snapshot-and-relabel to 1.0 (ADR-0028 and its
  annotation).
- **A scheduled live-provider smoke** workflow, the only test that exercises
  an extension's HTTPS path against a real endpoint.

### Changed

- Cancellation reaches every blocking host wait: the OAuth callback and,
  new in this release, `net` requests and streaming body reads. A hung
  request returns within the NFR-21 window instead of waiting out its
  timeout.
- The interface renders a reasoning model's reasoning marked `∴` and set off
  from the answer, and names the tool and its argument in a finished tool
  call instead of the provider's opaque call id.
- A large paste arrives as one bracketed-paste event instead of one key event
  per character.
- The interface says it needs a terminal when run without one, instead of the
  opaque `os error 6`.

### Fixed

- **Extension `https` requests worked again.** The pinned-DNS connector left
  `enforce_http` set, so every `https` `net` request died before TLS with
  "invalid URL, scheme is not http".
- **Capability grants are keyed by the project, not the data directory.**
  A grant attached mid-session was invisible to the engine that had to honor
  it, and shell "allow always" patterns applied to every project.
- **Re-compaction no longer drops the previous summary**, so facts distilled
  early in a long session survive.
- **A corrupt session says so**: the interface kept the reader's truncation
  warning instead of showing a short transcript silently.
- **`lca resume` lists current message counts**, not a snapshot from session
  creation.
- **A non-SSE provider response is an error**, not a silent empty answer.
- A plain conversation no longer logs a false cache-boundary divergence every
  turn.
- `grep` accepts a file path (it failed with "Not a directory").
- The Windows credential file gets an explicit owner-only DACL.

## [0.1.3] - 2026-09-25

Runtime and UX fixes from hands-on testing.

### Fixed

- Extension manifests that declared the `command` world without exporting it
  made the host refuse the installed artifact; both providers ship no slash
  commands, so the world is gone and the components load.
- The interactive TUI could leave the terminal in raw mode on a panic; a
  restore guard plus panic hook always restore, and `ext install` answers
  with a single key.

### Added

- A real input cursor (Left/Right/Home/End/Delete), `/help`, `/exit`, and Tab
  completion that lists multiple matches.
- A Pi-inspired layout: no boxes, a flowing transcript, a single separator,
  a dim status line.

## [0.1.2] - 2026-09-25

- Fixes `lca --version` and the `session-start` record, which reported ABI
  0.1 from a stale constant; the ABI is sourced from the contract crate so it
  cannot drift.
- Publish assets are named from the manifest ABI line.

## [0.1.1] - 2026-09-25

- Post-audit patch: fixes the critical, high, medium, and low findings from a
  full code and test review, extends conformance coverage, and hardens
  traceability.

## [0.1.0] - 2026-09-25

- Phase 5 exit: reference extensions published as OCI artifacts
  (`ghcr.io/misaalanshori/lca/*`) and as a plain HTTPS zip, both installable
  with `lca ext install` (ADR-0010).
