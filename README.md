# LCA

LCA is a lightweight, cross-platform coding agent for the terminal: one static
binary, no language runtime to install, and a plugin system where every
third-party extension runs inside a WebAssembly sandbox under permissions you
grant by name. It reads a prompt, calls a language model, runs tools against
your files and shell, and streams the answer into a terminal interface with a
durable, forkable session log behind it.

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

A real turn, captured from a real terminal:

```
[session in /home/debian/projects/lca]
› Say hello in exactly five words, then stop.
∴ Thinking… (ctrl+t to expand)
 Hello there, nice to meet you!
────────────────────────────────────────────────────────────────────────────
>
~/projects/lca (main)
↑978 ↓21 R0 W0 0% • opencode-go/mimo-v2.6-flash • ctx ?
done
```

The status line reads tokens up/down, reasoning tokens, cache reads/writes,
context use, and cost. `/help` lists the commands; the ones you will reach
for first are `/login`, `/logout`, `/usage`, `/model`, `/thinking`,
`/compact`, `/theme`, `/resume`, `/fork`, `/tree`, `/trust`, `/grants`,
`/session`, `/attach`, `/fullscreen`, and `/exit`. `!command` runs a shell
command inline, `!!` runs one the model never sees.

---

## What it is

**Core.** A small trusted core owns the agent loop, session storage, the
built-in file and shell tools, the terminal renderer, and the permission
system. Everything else — model providers, compaction, slash commands,
lifecycle hooks, custom rendering — is an extension. The core is deliberately
small: adding a feature to it needs a written argument for why it cannot be
an extension ([ADR-0013](docs/adr/0013-three-kinds-of-pluggability.md)).

**Extensions.** An extension is a WebAssembly component (`.wasm` plus an
`extension.toml` manifest), installed from an OCI registry, a plain HTTPS
archive, or a local path — no npm, no package manager, no second toolchain
on your machine. The same source can also be compiled into the binary as a
native-linked first-party extension, which is how the default provider ships;
the choice is a build flag, not a code change. An extension declares what it
needs in its manifest, and the manifest is the consent screen you see at
install time ([ADR-0030](docs/adr/0030-three-bags-resources-state-credentials.md),
[ADR-0009](docs/adr/0009-extension-update-path.md)).

**Capabilities.** Nine named grants — `fs`, `net`, `net-local`, `oauth`,
`credentials`, `process`, `pty`, `ui`, `completion` — plus an always-granted
log. Anything not granted is not reachable: the host links every capability
interface in a denied state and checks the grant on each call, so an
extension that was never given the network cannot reach the network whatever
its code says. Filesystem grants are named scopes (`workspace`, `private`,
`home-config`, `temp`), never raw paths, and a host that resolves outside the
scope refuses the call
([ADR-0005](docs/adr/0005-filesystem-scopes.md),
[ADR-0026](docs/adr/0026-denied-state-capability-linking.md); the catalog is
[`docs/capabilities.md`](docs/capabilities.md)).

**Permissions.** The model never acts directly. Shell commands, reads and
writes outside the workspace, and extension network calls pass through a
prompt — allow once, always allow this pattern, or deny — and approvals are
stored per project in *your* home directory, never inside the repository, so
cloning a hostile tree cannot grant itself anything. A trusted folder
auto-approves only commands the analyzer can prove stay inside the workspace
([ADR-0006](docs/adr/0006-permission-store-split.md),
[ADR-0039](docs/adr/0039-folder-trust-and-rules.md)).

**Sessions.** Every turn appends to an on-disk log (never rewritten in
place), so resume, fork-at-any-message, and export are cheap and corruption
is recoverable. When context use crosses the threshold, a compaction
extension writes a durable summary record; a separate context-transform
extension reshapes what goes on the wire without touching the log
([ADR-0015](docs/adr/0015-compaction-and-context-transform.md)).

---

## Documentation

[`docs/lca-srdd.md`](docs/lca-srdd.md) is the requirements and architecture
document and the index for everything else. From there:

| Document | What it answers |
|---|---|
| [`docs/adr/`](docs/adr/README.md) | 39 decision records, 0001–0040 (0020 unused): why, what was rejected, when to revisit |
| [`docs/capabilities.md`](docs/capabilities.md) | every capability an extension can hold, normatively |
| [`docs/extension-authoring.md`](docs/extension-authoring.md) | writing and publishing an extension |
| [`docs/installation.md`](docs/installation.md) | the installers, manual install, security stance |
| [`docs/testing-plan.md`](docs/testing-plan.md) | how this is built test-first, and how CI runs it |
| [`docs/platform-notes.md`](docs/platform-notes.md) | the easy-to-get-wrong behavior per OS |
| [`docs/configuration.md`](docs/configuration.md), [`docs/headless.md`](docs/headless.md) | configuration keys; the scripting contract |
| [`docs/glossary.md`](docs/glossary.md) | the overloaded terms, disambiguated |
| [`docs/providers/`](docs/providers/README.md) | one profile per first-party provider |
| [`docs/release-policy.md`](docs/release-policy.md), [`CHANGELOG.md`](CHANGELOG.md) | versioning, artifacts, gates, what changed |

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
```

Ten gates run on the pipeline; the canonical list, with what each one runs
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

Real-terminal tests run inside tmux (Unix) and ConPTY (Windows) and assert
what is actually on the screen, including the `load-buffer`/`paste-buffer`
paste rows; the harness rules are in
[`docs/testing-plan.md`](docs/testing-plan.md) section 14.

---

## Releases

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

Implemented, gated, and released (currently **0.5.2**, extension ABI
`0.5`); phase-by-phase receipts are in
[`docs/phase-log.md`](docs/phase-log.md). What is *not* here, named rather
than implied:

- The **web/browser target** is designed and deferred
  (`scripts/deferred-requirements.txt`: NFR-11, FR-WEB-1/2/3).
- **Codex, LM Studio, and Ollama** provider profiles are specifications —
  nothing under `extensions/` builds them yet. What ships and publishes
  today is `openai-compatible` (bundled, enabled by default), `antigravity`,
  `skills`, and `compaction-default`.
- No telemetry (FR-CFG-3), no hosted service, no package-manager channel
  yet.
- `SECURITY.md` describes private reporting; binaries are unsigned in 0.5.x
  (checksums + attestations carry trust until signing lands).

---

## License and attribution

Apache-2.0 — see [`LICENSE`](LICENSE).

LCA's design takes a great deal from [Pi](https://github.com/earendil-works/pi)
(its minimal core, prompt compaction, testing discipline, and prompt-cache
measurement method) and, for the native-plus-WASM build shape, from
[fx](https://github.com/vercel-labs/fx); what comes from where is recorded
with attribution in [`docs/inspiration.md`](docs/inspiration.md). What LCA
adds is the sandboxed, capability-gated extension boundary — Pi's own
extensions run unsandboxed and require full trust.
