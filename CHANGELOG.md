# Changelog

Notable changes to LCA. Versions follow semantic versioning for the product;
the `lca:ext` ABI version is independent and is printed by `lca --version`.
Dates are UTC.

## [Unreleased]

### Fixed
- **Capacity failures retry (gh #202).** `Selected model is at capacity`
  and HTTP 529 (plus 503/504 siblings) are retryable: the shared
  `is_capacity_error` predicate classifies mid-stream error payloads
  at the wire kits and overrides a non-retryable flag in the turn
  loop, which backs off up to `provider.retry_limit` as before.

## [0.6.0] - 2026-10-08

The 0.6 train release: extension decoupling, shared wire kits, the
unified `/model` catalog, the `ui@0.6.0` world with host dialogs, and
the tool world with the additive hooks expansion. Extension ABI `0.6`
(window `0.5..=0.6`); `wit/` + `schemas/` freeze for the 0.6 line
after this release — no interface change rides a 0.6 patch.

### Upgrade notes (extension authors)
- **Single-tool and six-hook guests: rebuild, no source change.**
  The `tool` world's exports and the six-point `hooks` world are
  byte-identical; a `0.5` manifest keeps loading on `0.6` hosts.
  Declare `abi = "0.6"` on new builds.
- **`ui` world: breaking.** Rebuild against `lca:ext@0.6.0` with
  source changes: `boxed` carries border role + background tint, the
  `interaction` input gains `click-widget`/`click`/`scroll`, and the
  new widgets (`styled-text`, `markdown`, `button`, `table`,
  `scroll-container`) replace hand-rolled chrome. Ask questions
  through `lca:host/ui-dialogs`, not a custom modal.
- **New surface, opt in by declaring it:** the `tool-catalog` world
  for multi-tool suites (exposure + namespaces; only `direct` tools
  declare), the `tools` capability import for orchestrators
  (declare `[capabilities.tools]` with a reason), and the eight
  `hooks-*` worlds. Nothing new runs unless the manifest names it.

### Added
- **Tool suites, exposure, nested calls + the additive hooks expansion (gh #77, #45 — train cycle 4, final).**
  The `tool-catalog` world (multi-tool suites with `exposure` and
  `namespace`; only `direct` tools declare, `tool_search` discovers
  the rest) and the `tools` capability import (nested calls with
  `<parent>/<n>` ids and bounded records, active-set control with
  transcript entries). Eight opt-in hooks worlds (`message_end`
  replace, composable `tool_call`/`tool_result`, stream observation,
  actionable settle, compact veto, cache and trust votes); the `tool`
  world and the six-point `hooks` world are byte-identical.
  `wit/` + `schemas/` freeze for the 0.6 line after this cycle.
- **Rich extension UI: `ui@0.6.0` world + host dialogs (gh #172, #124).**
  The ABI train's first real interface break (`lca:ext@0.6.0` / `lca:host@0.6.0`,
  window `0.5..=0.6`): dual-channel styled text (roles or `#RRGGBB`, independent
  `39`/`49` resets, no-color and 16-color degradation), the `markdown` / `button` /
  `table` / `scroll-container` widgets, `boxed` with border role and background
  tint, mouse (`click-widget`, relative `click`, `scroll` over modal and panel),
  and the `lca:host/ui-dialogs` import (`confirm`/`select`/`input`/`notify` on
  native chrome, denied values headless). `ui-example` demonstrates the button;
  the conformance extension probes every new shape in both modes.
- **Two subscription providers + the parity table (gh #63, #180, #181, #182).**
  `extensions/codex` (ChatGPT subscription OAuth + Codex responses
  gateway) and `extensions/grok` (SuperGrok OAuth + Grok responses
  proxy), both thin spec tables over the new `lca-subscription` kit
  (one PKCE login/refresh flow, one Responses-protocol core) with mock
  conformance journeys and credential-gated live smokes. Vercel AI
  Gateway lands as an `openai-compatible` preset, not an extension.
  `docs/providers/README.md` gains the pi-provider parity table the
  next demand votes get counted against.

### Added
- **Extension-owned custom endpoints (gh #188).** The host no longer synthesizes `__custom__`: `openai-compatible` declares its own `custom` preset (base-url/api-key/model, preset-less, default-profile), and `/login` with no options names the way out.
- **Unified `/model` catalog with provider switching (gh #177).** `/model` lists every ready provider's models (ready = the `auth check` probe; unready contributes nothing, silently), each tagged with its provider; a cross-provider pick swaps the whole generation with a `ModelChange` record carrying both, and `/login` to another provider asks a switch confirmation instead of stranding. No WIT change.
- **Credential-free endpoints count as ready (gh #211).** The `openai-compatible` login attempt succeeds keyless when the active base URL matches a preset declaring `auth = "none"` (Ollama, LM Studio), so `/model` and `auth check` agree; bearer endpoints without a key stay `NotReady`. No WIT change.
- **Shared wire kits (gh #189).** `lca-wire-openai` (Chat Completions decoder + request mappers, Responses builder + event mapper + driver, one `StreamFailure` vocabulary) and `lca-wire-anthropic` (Messages builder + decoder with thinking-signature preservation and cache breakpoints). `openai-compatible`, `codex`, `grok` build on the kits; `lca-subscription` is OAuth/refresh only; `antigravity` adopts the shared failure vocabulary. No WIT change.
- **Manifest-declared provider needs (gh #157).** Default endpoint hosts, the credential namespace, and the login env override come from each provider's own manifest: `lca-cli` embeds no provider literals, and `--no-default-features` still boots a working host.
- **Compaction file tracking, checkpoint, recovery, retain-none, phase 3 (gh #36, epic complete).** Records carry cumulative capped file lists; the system prompt is checkpointed with change detection; a capped generation compacts and retries once; manual `/compact` anchors its own id.
- **Compaction split spans + iterative summaries, phase 2 (gh #36).**
  A straddling span splits at record granularity (prefix summarizes,
  tail stays); the latest summary rides in-band (capped, tagged) so
  the strategy refines instead of restarting.
- **Compaction trigger + budget, phase 1 (gh #36).** The trigger is
  `context_tokens > context_window − reserve` with
  `compaction.enabled/reserve_tokens/keep_recent_tokens` (reserve 0
  derives the old fraction behavior); the cut keeps the recent window
  verbatim, anchors `first_kept_id` on the record, and never splits a
  tool pair; the summary budget derives `0.8 × reserve`. Later phases
  (split spans, recovery ordering, checkpoints) are recorded in
  `docs/compaction.md`, not built.
- **System-prompt files and flags (gh #68).** `~/.lca/SYSTEM.md`
  replaces the built-in preamble and `APPEND_SYSTEM.md` appends after
  project context (before the skills catalog); trusted-project files
  win by replacement, untrusted projects are ignored; and
  `--system-prompt` / `--append-system-prompt` win for one run.
- **User key bindings (gh #66).** `~/.lca/keybindings.toml` maps action
  names to keys (string, list, or empty-to-disable) through the
  existing registry; `/hotkeys` prints the effective bindings. Unknown
action names, double-claimed keys, and parse failures report loud on
the startup notice while defaults hold.

### Fixed
- **Ranked autocomplete (gh #176).** Slash-command and `@file` offers
  sort by pis fuzzy score (word-start and consecutive bonuses, gap and
  position penalties, exact-match reward), so `/st` selects `/stats`
  and `/ant` selects `/antigravity.*`; an empty query keeps
  registration order.
- **Bounded, highlighted, clickable popup (gh #175).** The suggestion
  popup shows at most five rows in a rolling window around the
  selection, paints the selected row in `SelectedBg`, appends a dim
  scroll hint on overflow, and answers clicks (apply) and wheel
  (scroll) over its rows. The editor and footer stay on screen.
- **Wrapped modal URLs stay clickable (gh #178).** The OAuth waiting
  label carries an explicit OSC 8 sequence and `overlay_box`
  linkifies raw URLs before wrapping, so every wrapped segment
  re-opens the full link and any row clicks open the whole URL.
- **Antigravity 0.9.0 wire alignment (gh #179).** A login with stored
  tokens re-runs the OAuth flow instead of short-circuiting to `Ok`, so
  expired or revoked tokens repair rather than deadlock; a 401 purges
  the stored tokens so the next call re-authenticates instead of
  looping. Later steps carry pi's deterministic `last_execution_id`
  label; tool schemas split by model class (Gemini takes
  `parametersJsonSchema`, Claude/GPT-OSS take the allowlisted legacy
  `parameters`); and a 404 on a preview id falls back to the mapped
  backend model.

## [0.5.4] - 2026-10-05

### Added
- **Model switching that follows the provider (gh #8, gh #31).**
  `Ctrl+P` cycles models forward (`Ctrl+Shift+P` / `Alt+P` back) and
  `Ctrl+S` saves the picker choice as the session default; `--model`,
  `--provider` (which scopes the lookup), and `--thinking <level>` work
  from the command line with per-model thinking clamps, and every
  switch lands everywhere at once - runner, status line, compaction
  backend, `meta.json`, and a `model-change` record. Deliberate
  divergence, recorded where the flags are declared: pi's `--api-key
  <key>` will not be added - argv is world-readable, and secrets never
  touch argv, scrollback, or history (`OPENAI_API_KEY` is the
  documented path); model scoping stays in `models.enabled` patterns
  and `--provider`, not a new surface.
- **Provider profiles with honest labels (gh #31).** One
  `openai-compatible` login per service preset, each with its own
  endpoint, key, and models, so a second login stops overwriting the
  first; picker rows name the service that will bill the call
  (`model (provider)`) while selection, logs, and requests keep the raw
  id. Model discovery runs through host consent on the request path
  (with `--allow-host` for scripts), per-model context windows ship in
  the catalog (gh #34), and a preset login now offers its endpoint's ad
  hoc grant at sign-in - previously a local preset signed in and then
  failed every turn with no recourse (gh #21).
- **The interactive `/settings` selector (gh #30).** `/settings` edits
  live values (theme, thinking, fullscreen, permissions mode) through
  the picker chrome with the winning source on every row; `lca config`
  stays the print-only, script-readable dump.
- **Fullscreen stops scrolling away (gh #35).** The alt-screen viewport
  is a pinned dock (separator, notice, queue, editor, footer) over a
  transcript window with pi's scrollbar geometry and a `↓ Jump to
  latest message` indicator (`End` returns); main-screen scrollback is
  untouched.
- **The edit result renders as pi's diff card (gh #9).** The tool's
  structured diff rides `extras` into the card, and `Ctrl+X` copies
  with pi 1.0.0's context-aware target (selection, sign-in URL, last
  reply); `markdown.codeblock_border` (`full`/`horizontal`/`none`, gh
  #32) controls the fence frame.
- **Composer polish: cost, pickers, mid-turn commands.** The footer
  shows the cost once usage is measured - a measured `$0.0000` is a
  free model, silence before that is noise (gh #17). Pickers anchor
  directly above the composer, overlaying tall notices (gh #16). A
  slash command typed mid-turn dispatches as a command: safe UI
  commands run at once, `/compact` queues as a command and runs at
  turn end, never as model text.
- **Click-to-toggle thinking runs (gh #11).** The transcript maps
  reasoning rows, and a single alt-screen click toggles that run like
  `Ctrl+T` (multi-click stays with selection, the scrollbar is not
  content, the jump indicator is clickable too); main-screen never
  captures the mouse, by design. The markdown pipeline takes ordered
  pre-parse transforms (gh #12, pi's `registerMarkdownTransformer`)
  with panic-is-identity, exposed to native extensions through an
  optional Rust dispatch method - no WIT growth.
- **A startup key-hint line (gh #23).** One dim line on the first frame
  (`escape interrupt · ctrl+c clear/exit · / commands`), key names from
  the default table; headless never prints it.
- **The SDK approval surface (gh #14).**
  `Session::with_permission_prompt` takes the host's callback (the
  action verbatim in, once/always/deny out) wherever the UI would open
  the modal; without it the session declines and records
  (deny-by-default). Yolo equivalence is out of scope.
- **`ui.theme = "system"` (gh #10).** Foreground and background roles
  derive from the detected terminal palette (OSC 11 / DSR / COLORFGBG
  parsers the engine already carried) and repaint on scheme change;
  unanswered terminals fall back to dark/light, `plain` still emits
  zero SGR, and `#rgb` expands in theme files. `oklch()`/`okhsl()`
  and `theme.style()`/`colors()`/`appearance` are mapped for
  pi-parity, not this cycle; the default stays `auto`.
- **Compaction asks the model with a budget (gh #169).** The
  summarization round-trip carries an explicit generation cap over a
  bounded prompt and degrades loudly, not silently. This is the
  stopgap, said plainly: the architecture (budgets, keep-recent,
  overflow recovery) is gh #36's work.
- **Local presets are the supported local-model route (gh #21).**
  Codex, LM Studio, and Ollama stay spec-only by explicit policy
  (dedicated extensions are pi-parity-phase work); the
  `ollama`/`lmstudio` presets (`auth = "none"`) sign in on selection
  through `openai-compatible`, and Codex - which needs its OAuth
  extension - waits for the phase.

### Changed
- **One GrantStore everywhere (gh #29).** A second handle opened
  beside the session's clobbered state; approval, consent, and
  `--allow-host` now write through the shared handle, `once` covers
  the session, and the startup note is honest about what happens
  next. The capability consent requires Enter (gh #24) and survives a
  console left in raw mode.
- **Shutdown and resumption details.** Exiting parks the cursor below
  the transcript on every renderer (gh #33); `meta.json` carries the
  last model and provider (gh #20); a closing stdout (`| head`) ends
  by signal instead of panicking (gh #19).
- **Prompt rows keep their marker pad** and spaces survive wraps;
  Up/Down move by visual row (gh #27, gh #28). A modal close that
  shrinks the frame clears vacated rows first, so no stale notice
  survives it (gh #18).
- **`lca config` is print-only by design** (gh #30); `/settings` is the
  editor.

### Fixed
- **`install.ps1` no longer exits the caller's host under `iex`**
  (gh #26); the exit code lands in `$LASTEXITCODE`.
- **A namespaced `<provider>.login` reaches the identity flow**
  instead of opening an empty preset picker (gh #25).
- **Model discovery runs in its own blocking region** (Windows CI)
  and per-model windows reach the footer (gh #34, gh #31 review).
- **The live-provider smoke tolerates quota answers** and writes its
  grant where the store reads it.
- **Dependency bump:** Wasmtime 49.0.2 (RUSTSEC-2026-0321..0324).
- HTML export stays with gh #57 (pi-parity backlog), not this release.

## [0.5.3] - 2026-10-02

### Added
- **The composer stops fighting its user (UI/UX Phase 1).** The caret is
  now painted by the interface: the grapheme under the cursor in reverse
  video, an end-of-line caret as a reverse-video space, `CURSOR_MARKER`
  kept for IME - spaces are visible as you move over them, Home/End can
  no longer desync the caret, and a ZWJ emoji is one step and one delete.
  The hardware cursor is positioned but hidden while a caret is painted
  (pi's default), and the plain theme - which renders no escapes at all -
  keeps the real cursor. Up/Down now match pi's boundary: inside the
  buffer first, into history only at the first/last line under pi's
  conditions, and Down at the bottom jumps to the end of the line.
  `/settings` during a running turn no longer freezes the interface (the
  input thread used to queue behind the turn's `tools` lock - Ctrl+C went
  with it). And `tool.max_iterations` defaults to **0 = unlimited**: the
  runaway guard is opt-in, the notice still names the configured value,
  and FR-CORE-9 carries the dated amendment.
- **The unstable release line.** Every green commit on `main` now publishes
  six binaries, `artifacts.sha256`, and provenance attestations to a rolling
  prerelease tagged `unstable`, and `install.sh --unstable` /
  `install.ps1 -Unstable` install and update that line through the same
  one-liners — same verification, same assets, versioned `X.Y.Z.b<sha7>`
  (`lca 0.5.2.b6573049`). Without the flag both installers resolve the
  stable line exactly as before, and the flag is latest-only: combined with
  `--version` it is a usage error (ADR-0043, FR-INSTALL-10).
- **LaTeX math and mermaid diagrams (TUI-10 M3/M4).** Inline `$…$`/`\(…\)`
  and display `$$…$$`/`\[…\]` typeset through a port of pi's `latex.ts`:
  224 symbol commands, a recursive-descent parser, and baseline-joined 2D
  layout for fractions, limits, scripts, matrices and cases - with pi's
  fail-soft contract, so unsupported or malformed input prints as the raw
  source, and streamed, still-open math stays raw until its closer
  arrives. A ` ```mermaid ` fence renders as Unicode art (flowcharts and
  sequence diagrams, with pi's width guard falling back to the framed
  source and his warning note showing only once streaming settles); the
  theme spends six of pi's own roles, none of them new.
- **Markdown parity with pi, pass one (TUI-10 M1/M2).** Hyperlinks use pi's
  full capability ladder - the tmux client's `client_termfeatures` probe,
  the known-terminal table, and a `LCA_HYPERLINKS=1|0` override in
  `PI_HYPERLINKS`'s shape - with pi's ST-terminated OSC 8 and
  `link(underline(text))`. Autolink literals (bare URLs, `www.`, emails)
  linkify; strikethrough is pi's strict `~~…~~`; backslash escapes carry
  both of pi's modes; authored list markers (`1)`, `+`) survive under the
  preserve options; a blockquote renders its children as blocks and `>>`
  nests; a too-narrow table falls back to the raw source and wrapped cells
  reset the narrow styles between fragments; an image prints its alt text;
  and the user's own message renders as markdown inside the band.
- **The transcript and chrome use color the way pi does.** User messages render as a full-width `userMessageBg` band with the content padded inside it; tool cards carry their state's background (`toolPendingBg` while the call is in flight, `toolSuccessBg` when it settled, `toolErrorBg` when it did not); `[type]` headers (`[session in …]`, `[compaction] …`) take pi's `customMessageBg` band with the label in `customMessageLabel`; a picker's selected row is `selectedBg` on accent text; the footer paints the thinking level with its own `thinking*` role and the context share by threshold (warning above 70%, error above 90%). The theme's role table was already pi's; the renderers now spend it.
- **The separator row carries pi's spinner-in-the-border**: `── ⠴ Working ─────…` while a turn runs - the whole row one color, spinner, label and dashes alike, exactly how pi paints its embedded indicator - , `Retrying (n/m) in Ns…` counting down through a provider backoff (warning spinner), and plain border dashes at rest. The row is always exactly one terminal row, fills the width, and animates on the interface's own tick - an idle interface never repaints. The dashes carry the thinking level, the way pi colors its editor border (`thinkingOff`, the same darkGray as `borderMuted`, when no level is set).
- **Code blocks are syntax-highlighted.** A fence that names a language the theme knows (rust, javascript/typescript, python, go, c/c++, java, sh/bash, sql, json, yaml, toml - and their usual aliases) paints pi's nine `syntax*` classes: comment, keyword, function, variable, string, number, type, operator, punctuation. A fence with no language, or one the highlighter does not know, keeps pi's fallback - the whole block in `mdCodeBlock` - and language auto-detection stays off, for pi's stated reason (it colors prose). The grammars are hand-written token classes, not a new dependency.
- **The `shell` tool runs a real shell, chosen by a documented ladder.** `shell.tool` (`auto`, `bash`, `pwsh`, `powershell`, `cmd`) and `shell.path` (an exact interpreter) pick it; `auto` on Windows finds Git Bash by its install location - not by the `bash` on `PATH`, which is usually the WSL stub and silently changes what every path means - then `pwsh`, then `powershell`, then `cmd`. A configured interpreter that cannot be found fails loudly with the locations searched. The model is told which interpreter it got and how its dialect reads. On Windows every command now travels in a per-call script file (`.sh`/`.ps1`/`.cmd`), so quotes, newlines, and metacharacters reach the shell exactly as written: `echo "double"` is no longer `echo \"double\"`, and a multi-line command runs all of its lines. POSIX behavior is unchanged. ADR-0041; the ladder, its traps, and the transport table are in `docs/platform-notes.md`.
- **Yolo mode**: `--yolo` or `permissions.mode = "yolo"` answers every permission prompt "always, for this exact action's pattern" - the pattern is persisted and the session log records the same `permission` entry a human answer writes, so approve-everything never means forget-everything. The footer shows a `YOLO` line while it is on and the session opens with one explanatory line. Explicit deny rules still deny, and the mode is never persisted for you. ADR-0042.
- **Read-only tools outside the workspace no longer prompt** (`read`, `list`, `glob`, `grep`); a deny rule still refuses them, and writes/edits outside the workspace still ask.
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
  the update (`lca 0.5.2 -> 0.5.3`); `--uninstall` / `-Uninstall` removes
  the binary and every PATH entry it added, leaving pre-existing rc files
  byte-identical. Pin a release with `--version v0.5.3` / `-Version v0.5.3`;
  mirror or hermetic runs use `LCA_BASE_URL` / `-BaseUrl`. The full spec,
  the manual install path, and the security stance are in
  `docs/installation.md` (ADR-0040).
- **`LICENSE`** (Apache-2.0, as `Cargo.toml` has always declared) and
  **`SECURITY.md`** (the private-reporting contact that
  `docs/release-policy.md` already pointed at).
- **Pi CLI parity and provider polish (UI/UX Phase 4).** Added `-c` / `--continue`
  to auto-resume the latest session in the working directory, `-r` / `--resume <ID>`
  alias, and `--model <ID>` override flag. Model picker labels display as
  `model (provider)` (pi style) while passing the raw un-decorated model ID
  structurally to provider requests and logs.
- **Live generation throughput (`tok/s`) and metrics refinement.** Footer
  reports generation rate (`tok/s`) measuring provider output tokens over
  active streaming duration (excluding tool execution), cache hit percentage
  at one-decimal precision (`CH60.9%`), compact count formatting (`1.0k`,
  `1.3M`), and position indicator in the `/model` picker.
- **Antigravity wire parity with `pi-antigravity`.** Wire envelope, headers,
  User-Agent (`antigravity/cli/1.2.4`), metadata, and fallback catalog
  aligned byte-for-byte with upstream `pi-antigravity` release 0.9.0
  (`a3d8caba1b10263420060406de57112ce16490d0`).

### Changed
- **Style functions reset only their own SGR channel** (`ESC[39m` for a foreground, `ESC[49m` for a background, the decoration's own close code for a decoration) instead of a full `ESC[0m`, which is pi's `fg()`/`bg()` semantics: a styled run inside a background band keeps its color, and a nested style cannot blank the one around it. Dialog rows still reset explicitly around the frame, so the cycle-8 bleed fix is untouched and its assertions are unchanged.
- **Markdown paints the way pi's does**, from a row-by-row diff against a live pi: headings take `mdHeading` (they used to come out in the body color, because the markdown `bold` carried a color of its own), headings render their inline markup instead of printing `**`, a blockquote colors its text `mdQuote` + italic and its border glyph `mdQuoteBorder`, and a table header is bold. All four were found by driving LCA beside pi, not by a test - the tests that now hold each are in `docs/testing-plan.md` §14.
- **A denied tool call shows a card.** The start event used to fire only after the permission check, so refusing a command left the transcript silent (no card, no `denied` label) and `--json` printed a `tool-result` with no `tool-call` before it. The card now exists from the moment the model asks, which is also where the session log records the request.
- **`ctrl+t` expands the run it is pressed on** even when the newest assistant message is a tool report: the key used to target only the newest message, so it was dead whenever a `ctrl+t to expand` marker was on screen. An expanded run also renders its blank lines blank instead of as bare `∴` rows.
- **A queued message keeps its `steer`/`follow-up` marker** when it flushes as the next turn: the record now says how it was submitted, as `docs/session-log-format.md` requires, instead of calling it an ordinary prompt.
- **A theme change repaints the transcript.** The per-entry render cache is keyed by width, so `/theme`, its live preview, and a detected-scheme swap now drop it instead of serving rows painted in the previous palette.
- **`ui.fullscreen` defaults to `false` (main screen / scrollback mode)**: the transcript appends directly to terminal scrollback so native selection and scrollbars work out of the box, with `/fullscreen` available for alt-screen mode.
- **Thinking runs show a short snippet by default** (`ui.thinking = snippet`): the first three non-empty reasoning lines plus a `… +N lines` marker, with the expand key overriding the latest run in place. `full` and `hidden` remain as settings.
- **The agent's data and configuration live in `~/.lca` on every platform.** Sessions, extensions, credentials, grants, state, temp, themes, `ui.json`, and `config.toml` all sit under one home dot-directory instead of the platform-specific ones (`~/.local/share/lca` and `~/.config/lca/config.toml` on Linux, `~/Library/Application Support/lca` on macOS, `%APPDATA%\lca` on Windows). **There is no migration code.** The old directories are untouched: copy the one you want across yourself, for example `cp -a ~/.local/share/lca/sessions ~/.lca/` on Linux, or `xcopy /E /I "%APPDATA%\lca" "%USERPROFILE%\.lca"` on Windows. `docs/platform-notes.md` names each old path.
- **The README was rewritten claim by claim against the tree**: install
  first, a quickstart carrying a transcript from a real terminal, the
  extension/capability/permission model with links to the ADRs that own
  it, the ten pipeline gates, and the six release assets with their
  verification commands. Truth-up fixes found while checking it: the ADR
  count (39 records, 0001-0040), a duplicate row in the ADR index, and
  three provider profiles (`codex`, `lmstudio`, `ollama`) that described a
  source tree which does not exist - they are now marked as
  specifications, not installable extensions.

### Fixed
- **A cancel that raced call setup no longer loses the interrupt.**
  Wasmtime measures a store's epoch deadline from the epoch at
  `set_epoch_deadline` time, so an `interrupt` landing while the store
  was still being built was absorbed and the guest spun until its fuel
  budget - which is what fired the 30-second hang guard in
  `cancelling_a_turn_interrupts_a_running_extension_call` on a loaded
  Windows runner (CI 36864779928). The host flags the interrupt before
  bumping the epoch, re-arms it after the deadline is set
  (`build_store`), and clears it - with the capability engine's own
  cancel flag - at the turn boundary through the new
  `Dispatch::turn_started`, so a fresh turn cannot start pre-cancelled.
  FR-CONC-1.
- **Native extensions clear cancel flag at turn boundary.** Previously,
  an `interrupt()` call latched the cancellation state in native extensions,
  causing all subsequent turns to fail with "request cancelled by the user"
  until process restart.
- **Live model catalog refresh after login.** Dynamic model discovery
  immediately exposes newly authorized models upon successful login without
  requiring an agent restart.
- **The live-provider smoke writes its grant where the store reads
  it.** `real_provider.rs` still placed `grants.json` under
  `$XDG_DATA_HOME/lca`, the pre-move layout, so since the data dir
  became `~/.lca` (dae9470) the smoke's `opencode.ai` pattern never
  landed and the daily run failed with "permission denied:
  opencode.ai:443 matches no granted pattern".
- **Session metadata tracks last used model and provider.** `meta.json`
  now accurately records `model` and `provider` upon turn completion.

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
