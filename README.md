# LCA

LCA is a coding agent for your terminal that ships as **one small, static
binary** — no runtime to install, no package manager, no daemon. Everything
you add runs behind a real capability sandbox: an extension is a WebAssembly
component that gets exactly what its manifest declares, and the host links
every capability it did not get in a denied state, so a denied call is a
link-time refusal rather than a promise. The interface is a terminal UI
rebuilt from [pi](https://github.com/earendil-works/pi)'s design — full-width
user bands, tool cards tinted by their state, and one separator row that
always says what the agent is doing — over append-only sessions you can
resume, fork, and export.

One real turn, captured verbatim from a tmux pane (rows joined, gaps marked
`…`):

```
[session in /home/debian/projects/lca]
› Now run ls crates/ and tell me how many crates the workspace has.

▍

── ⠙ Working ─────────────────────────────────────────────────────────────
>
~/projects/lca (main)
↑155 ↓356 R2304 W0 94% • opencode-go/deepseek-flash • ctx ?
running...

∴ Let me run ls crates/ and count.

 > shell ls crates/ ok
   lca-cli
   lca-config
   … (11 more lines, ctrl+o to expand)

 ls crates/ lists 16 entries:

 ╭────────────────────────────────────────────────────────────╮
 │ lca-cli        lca-config     lca-core       lca-ext-abi   │
 │ lca-ext-host   lca-ext-native lca-permissions lca-protocol │
 │ lca-provider   lca-registry   lca-sdk        lca-session   │
 │ lca-testkit    lca-tools      lca-tui        lca-ui        │
 ╰────────────────────────────────────────────────────────────╯

────────────────────────────────────────────────────────────────────────────
>
~/projects/lca (main)
↑445 ↓848 R4992 W0 92% • opencode-go/deepseek-flash • ctx ?
done
```

In color: the prompt sits on a full-width band, the tool card is tinted by
its state (pending, ok, failed), code is highlighted through the theme's
`syntax*` roles, and the separator is the animated one — `── ⠴ Working
────…` while a turn runs, `Retrying (n/m) in Ns…` through a provider
backoff, plain dashes at rest. Every state says its condition in words, so
color is never the only signal. The footer reads tokens up and down,
reasoning tokens, cache reads and writes, context use, and cost — plus a
`YOLO` line while every permission prompt is being auto-approved.

---

## Install

**Linux and macOS** (POSIX sh, no sudo):

```sh
curl -fsSL https://raw.githubusercontent.com/misaalanshori/lca/main/install.sh | sh
```

**Windows** (Windows PowerShell 5.1 or pwsh):

```powershell
irm https://raw.githubusercontent.com/misaalanshori/lca/main/install.ps1 | iex
```

**The unstable line** — the latest green commit, as a rolling pre-release
(`X.Y.Z.b<sha7>`, may break; [ADR-0043](docs/adr/0043-unstable-release-line.md)):

```sh
curl -fsSL https://raw.githubusercontent.com/misaalanshori/lca/main/install.sh | sh -s -- --unstable
```

```powershell
& ([scriptblock]::Create((irm https://raw.githubusercontent.com/misaalanshori/lca/main/install.ps1))) -Unstable
```

Pin a release by passing it as an argument (both invocations below are the
ones the CI job exercises):

```sh
curl -fsSL https://raw.githubusercontent.com/misaalanshori/lca/main/install.sh | sh -s -- --version v0.5.2
```

```powershell
& ([scriptblock]::Create((irm https://raw.githubusercontent.com/misaalanshori/lca/main/install.ps1))) -Version v0.5.2
```

| | Default install directory | Overrides |
|---|---|---|
| Linux / macOS | `~/.local/bin` | `--install-dir <dir>`, env `LCA_INSTALL_DIR` |
| Windows | `%LOCALAPPDATA%\lca\bin` | `-InstallDir <dir>`, env `LCA_INSTALL_DIR` |

What the installer does: picks the release asset for your platform from the
six the project publishes, downloads it with `artifacts.sha256`, **verifies
the SHA-256 digest before anything is moved into place** (a mismatch refuses
and leaves your existing binary untouched), and only then installs it. It
adds one marked `PATH` block to your shell rc file (or the user `Path` on
Windows) and prints the `source` line to run — skip that with `--no-path`.

- **Update:** run the same one-liner again. It prints `lca <old> -> <new>`.
- **Uninstall:**
  `curl -fsSL https://raw.githubusercontent.com/misaalanshori/lca/main/install.sh | sh -s -- --uninstall`
  (Windows: `... -Uninstall`) — removes the binary and every PATH entry it
  added, nothing else.
- **Mirror / offline:** point `LCA_BASE_URL` (or `-BaseUrl`) at any host — or
  local directory — laid out like the release.
- **Install by hand instead?** Download the asset and `artifacts.sha256` from
  the release page, verify the digest yourself, and check provenance with
  `gh attestation verify ./lca --repo misaalanshori/lca`. The full manual
  path, the flags, the exit codes, and the security stance (binaries are
  unsigned in 0.5.x; checksums and attestations are the trust chain; the
  macOS quarantine strip is deliberate and explained) are in
  [`docs/installation.md`](docs/installation.md). The decision record is
  [ADR-0040](docs/adr/0040-install-and-update.md).

---

## Quickstart

Build from source (the toolchain is pinned by `rust-toolchain.toml`, so
`rustup` picks it up automatically):

```sh
git clone https://github.com/misaalanshori/lca && cd lca
cargo build --release -p lca-cli      # → target/release/lca
```

Run it:

```sh
lca                       # interactive interface in the current directory
lca -p "fix the failing test"   # one turn, reply on stdout
lca -p "..." --json       # one JSON object per line (docs/headless.md)
lca resume                # list this project's sessions, reopen one
lca ext list              # installed extensions and what they can reach
```

### First session

1. `lca` opens on an empty session with `no model` in the status line.
2. `/login` opens the endpoint picker — 19 presets ship (OpenAI,
   OpenRouter, DeepSeek, OpenCode Go, …) plus a `Custom endpoint…` row for
   any OpenAI-compatible base URL. Choose one, paste the API key into the
   masked field, and it is stored in that extension's own credential
   namespace, never in the session log.
3. If the endpoint you chose is not covered by the extension's manifest, you
   get the consent prompt naming the exact host:
   ```
   ╭─ ad hoc grant: openai-compatible ─────────────────────────────────────╮
   │ openai-compatible's endpoint is opencode.ai, which its manifest does  │
   │ not cover; add it as an ad hoc grant?                                 │
   │   connect to opencode.ai                                              │
   │ Allow [y] / Deny [n]                                                  │
   ╰───────────────────────────────────────────────────────────────────────╯
   ```
4. `/model` filters the live model list from that endpoint; pick one.
5. Type a prompt. Answers stream as they arrive; `ctrl+c` cancels the turn
   and keeps everything already written.

`/help` lists every command. The ones you reach for first are `/login`,
`/logout`, `/usage`, `/model`, `/thinking`, `/compact`, `/theme`, `/resume`,
`/fork`, `/tree`, `/trust`, `/grants`, `/session`, `/attach`, `/fullscreen`,
and `/exit`. `!command` runs a shell command inline, `!!` runs one the model
never sees.

---

## What makes it different

**A sandbox around everything you add.** The core owns the agent loop, the
built-in tools, the renderer, and the permission system; providers,
compaction, slash commands, and rendering extras are extensions. An
extension is a `.wasm` component plus an `extension.toml` manifest, or the
same source linked in natively as a first-party extension — a build flag,
not a code change. Adding a feature to the core needs a written argument for
why it cannot be one ([ADR-0013](docs/adr/0013-three-kinds-of-pluggability.md)).

**Nine named grants, deny by default.** `fs`, `net`, `net-local`, `oauth`,
`credentials`, `process`, `pty`, `ui`, `completion`, plus the always-granted
`log`, `resources`, and `state`. Anything not granted is not reachable: the
host links every capability interface in a denied state and checks the grant
on each call, filesystem grants are named scopes (`workspace`, `private`,
`home-config`, `temp`) rather than raw paths, and every denial is recorded
([ADR-0005](docs/adr/0005-filesystem-scopes.md),
[ADR-0026](docs/adr/0026-denied-state-capability-linking.md); the catalog is
[`docs/capabilities.md`](docs/capabilities.md)).

**Permissions that live outside the repository.** The model never acts
directly: shell commands, out-of-workspace reads and writes, and extension
network calls pass through a prompt — allow once, always allow this pattern,
or deny — and approvals are stored per project in *your* home directory,
never inside the tree, so cloning a hostile repository cannot grant itself
anything. A trusted folder auto-approves only commands the analyzer proves
stay inside it. Read-only tools never prompt outside the workspace (a deny
rule still refuses), and `--yolo` answers every remaining prompt "always,
for this exact pattern" while the footer says `YOLO` and explicit deny rules
still deny — and every auto-answer writes the same `permission` record a
human's would ([ADR-0006](docs/adr/0006-permission-store-split.md),
[ADR-0039](docs/adr/0039-folder-trust-and-rules.md),
[ADR-0042](docs/adr/0042-yolo-mode.md)).

**Sessions are append-only logs.** Every turn appends and nothing rewrites
in place, so resume, fork-at-any-message, and export are cheap and
corruption is recoverable. Crossing the context threshold triggers a durable
summary record; a separate context-transform extension reshapes what goes on
the wire without touching the log
([ADR-0015](docs/adr/0015-compaction-and-context-transform.md)).

**A terminal UI with one job per row.** Two renderers, one key: the
fullscreen (alt-screen) renderer is the default and keeps its own scroll and
selection, while `ui.fullscreen = false` hands the terminal its native
scrollback and selection back (`/fullscreen` flips it, and your choice
persists). The transcript is ported from pi's design — bands, tinted tool
cards, syntax-highlighted code — and every state the agent can be in has a
row of its own that says so in words
([ADR-0037](docs/adr/0037-the-tui-is-ours.md),
[ADR-0036](docs/adr/0036-tui-port-dependencies.md)).

**A shell tool that survives Windows.** Commands run in a real shell,
resolved once through a documented ladder — `shell.path`, then `shell.tool`
(`auto`, `bash`, `pwsh`, `powershell`, `cmd`), then the platform's own
order, which on Windows finds Git Bash by install location instead of the
`bash` on `PATH` (that one is usually the WSL stub, and it changes what every
path means). The model is told which interpreter it got, and on Windows the
command travels in a per-call script file so quotes and newlines reach the
shell exactly as written
([ADR-0041](docs/adr/0041-shell-tool-transport-and-selection.md),
[`docs/platform-notes.md`](docs/platform-notes.md)).

---

## Architecture

One process, sixteen crates: the binary drives a Tokio agent loop that owns
one turn at a time, hands the model's tool calls to built-in tools or to
extensions instantiated in Wasmtime (or linked in natively), and renders
every record it writes through a terminal engine that was ported from pi's
widget tree. Everything below is one workspace member:

| Crate | Role |
|---|---|
| `lca-cli` | the `lca` binary: argument dispatch, headless mode, the UI runner, config and data directories |
| `lca-config` | reading and merging configuration |
| `lca-core` | the agent loop — one Tokio-driven turn at a time |
| `lca-protocol` | shared data types: messages, tool calls, stream events |
| `lca-provider` | the provider trait, streaming types, host-side tool-call plumbing |
| `lca-session` | the append-only session log, fork, resume, export |
| `lca-permissions` | the grant store (ADR-0006), rules, and the filesystem scopes |
| `lca-tools` | the built-in tools: `read`, `write`, `edit`, `list`, `glob`, `grep`, `shell` |
| `lca-ext-abi` | the `lca:ext` contract: the normative WIT package under `wit/` |
| `lca-ext-host` | the extension host: Wasmtime, instantiation, capability enforcement |
| `lca-ext-native` | native-linked extensions, registered exactly like WASM ones |
| `lca-registry` | extension reference resolution and installation state |
| `lca-sdk` | the embedding API for host applications |
| `lca-tui` | the terminal engine and widget library — the pi TUI port (ADR-0036) |
| `lca-ui` | the agent interface built on that engine: transcript, theme, footer, overlays |
| `lca-testkit` | the scripted fake provider (pi's `providers/faux.ts` idea) and fixtures |

The extension contract itself lives in [`wit/`](wit/), a WIT package
versioned as the ABI line (currently `0.5`); the crates are internal
structure, not a public API.

---

## Where your data lives

Everything the agent owns sits under **`~/.lca`** on every platform:
`sessions/`, `extensions/`, `credentials/`, `grants.json`, `private/`,
`state/`, `tmp/`, `themes/`, `ui.json`, and `config.toml`. One home
dot-directory, pi's `~/.pi` shape. A project can add its own
`.lca/config.toml` at its root, which merges beneath the user file.

(This moved in 0.6.0-dev from the platform-conventional directories;
`CHANGELOG.md` and [`docs/platform-notes.md`](docs/platform-notes.md) name
each old path and say how to copy a directory across. There is no migration
code.)

---

## Development

Toolchain: stable Rust pinned at `rust-toolchain.toml` (1.98.1), edition
2024. Formatting is `rustfmt` with the default profile; lints are `clippy`
with warnings denied.

```sh
cargo nextest run --workspace        # the suite CI runs (cargo test works too)
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --check
bash scripts/traceability.sh         # every FR/NFR has a verifying test
bash scripts/install-check.sh        # gate 10: the installers
bash scripts/ceiling-check.sh        # gate 11: 1,200-line file ceiling
```

Eleven gates run on the pipeline; the canonical list, with what each one runs
and where, is the gate table in
[`docs/release-policy.md`](docs/release-policy.md#gate-list):

1. Format and lint (`cargo fmt`, `clippy -D warnings`, `cargo doc -D warnings`)
2. Test suite, on Linux, macOS, and Windows
3. NFR timing (NFR-4, NFR-5, NFR-29, release build, serial)
4. Dependency audit and license check (`cargo-deny`)
5. Requirements traceability (`scripts/traceability.sh`, NFR-30)
6. Fuzz targets build (`scripts/fuzz-check.sh`)
7. Extension components build for `wasm32-wasip2` (`scripts/wasm-check.sh`)
8. Release targets build, all six (`scripts/release-targets-check.sh`)
9. Size, startup, and the cache-hit-ratio benchmark (`scripts/perf-gate.sh`)
10. Installers (`scripts/install-check.sh` + the PowerShell suite on Windows)
11. File-size ceiling (`scripts/ceiling-check.sh`, 1,200 lines per tracked `.rs`)

Real-terminal tests run inside tmux (Unix) and ConPTY (Windows) and assert
what is actually on the screen, including the `load-buffer`/`paste-buffer`
paste rows; the harness rules are in
[`docs/testing-plan.md`](docs/testing-plan.md) section 14.

---

## Releases and verification

Six native assets per release, verified with `artifacts.sha256` published
beside them and provenance-attested by the pipeline:

| Platform | Asset |
|---|---|
| Linux x86-64 | `lca-x86_64-unknown-linux-musl` |
| Linux aarch64 | `lca-aarch64-unknown-linux-musl` |
| macOS x86-64 | `lca-x86_64-apple-darwin` |
| macOS arm64 | `lca-aarch64-apple-darwin` |
| Windows x86-64 | `lca-x86_64-pc-windows-msvc.exe` |
| Windows arm64 | `lca-aarch64-pc-windows-msvc.exe` |

Verify what you downloaded:

```sh
grep ' lca-x86_64-unknown-linux-musl$' artifacts.sha256 | sha256sum -c -
gh attestation verify ./lca --repo misaalanshori/lca
```

Extensions publish to `ghcr.io` under an immutable version tag and a moving
ABI-line tag (`abi-0.5`), and load by digest afterwards:

```sh
lca ext install ghcr.io/misaalanshori/lca/antigravity:abi-0.5
```

Versioning, the artifact matrix, the gate list, and the manual release gate
are in [`docs/release-policy.md`](docs/release-policy.md).

---

## Status and scope

Implemented, gated, and released — currently **0.5.2**, extension ABI **0.5**
— with phase-by-phase receipts in [`docs/phase-log.md`](docs/phase-log.md):

| Area | Status |
|---|---|
| Interactive TUI, tested on real terminals (tmux / ConPTY) | shipped |
| Sessions: resume, fork, export, attach, compaction | shipped |
| Permissions: pattern store, folder trust, read-only tools, `--yolo` | shipped |
| Extensions: WIT ABI `0.5`, installed from OCI/HTTPS/local or linked in natively | shipped |
| Providers: 19 presets + any OpenAI-compatible endpoint, login/usage | shipped |
| Built-in tools: `read` `write` `edit` `list` `glob` `grep` `shell` | shipped |
| Windows: first-class, real CI runners and dedicated tests | shipped |
| Web/browser target | designed, deferred (`FR-WEB-1/2/3`, `NFR-11`) |
| Codex, LM Studio, Ollama profiles | specification only — nothing under `extensions/` builds them |
| Telemetry, hosted service, package-manager channel | none (`FR-CFG-3`) |

What ships and publishes today is `openai-compatible` (bundled, enabled by
default), `antigravity`, `skills`, and `compaction-default`. Binaries are
unsigned in 0.5.x: checksums and attestations carry trust until signing
lands, and [`SECURITY.md`](SECURITY.md) describes private reporting.

---

## Documentation

[`docs/lca-srdd.md`](docs/lca-srdd.md) is the requirements and architecture
document and the index for everything else. From there:

| Document | What it answers |
|---|---|
| [`docs/adr/`](docs/adr/README.md) | 42 decision records, 0001–0043 (0020 unused): why, what was rejected, when to revisit |
| [`docs/capabilities.md`](docs/capabilities.md) | every capability an extension can hold, normatively |
| [`docs/extension-authoring.md`](docs/extension-authoring.md) | writing and publishing an extension |
| [`docs/installation.md`](docs/installation.md) | the installers, manual install, security stance |
| [`docs/testing-plan.md`](docs/testing-plan.md) | how this is built test-first, and how CI runs it |
| [`docs/platform-notes.md`](docs/platform-notes.md) | the easy-to-get-wrong behavior per OS |
| [`docs/configuration.md`](docs/configuration.md), [`docs/headless.md`](docs/headless.md) | configuration keys; the scripting contract |
| [`docs/glossary.md`](docs/glossary.md) | the overloaded terms, disambiguated |
| [`docs/providers/`](docs/providers/README.md) | one profile per first-party provider |
| [`docs/inspiration.md`](docs/inspiration.md) | what comes from Pi and fx, and what is novel |
| [`docs/release-policy.md`](docs/release-policy.md), [`CHANGELOG.md`](CHANGELOG.md) | versioning, artifacts, gates, what changed |

---

## License and attribution

Apache-2.0 — see [`LICENSE`](LICENSE).

LCA's design takes a great deal from [Pi](https://github.com/earendil-works/pi)
(its minimal core, prompt compaction, testing discipline, and prompt-cache
measurement method) and takes its product form — one small, single static
native binary — from [fx](https://github.com/vercel-labs/fx). The
native-plus-WASM build shape, the hook point names, and the permission model
are this project's own design; what comes from where, with the source that
supports each claim, is recorded in
[`docs/inspiration.md`](docs/inspiration.md). What LCA adds is the
sandboxed, capability-gated extension boundary — Pi's own extensions run
unsandboxed and require full trust.
