# Phase log

One entry per phase exit attempt: what ran, what passed, what blocked.

## Phase 0 — measurement and spikes: PASS

Exit test: "the size, startup, and streaming numbers exist and the team has
picked the default backend. NFR-1 and NFR-2 get their final values here."

Evidence: `docs/phase0-report.md`. Cranelift default backend confirmed
(ADR-0001 accepted), NFR-1 fixed at25 MB, NFR-2 at12 MB, streaming spike
measured (0.43 us/event), wasmi rejected with a receipt, and the
six-target cross-compilation matrix completed (all six build).

## Phase 1 — the core agent with no extensions: exit test INCOMPLETE

Exit test: "a user can hold a working coding session on Linux, macOS, and
Windows, resume it the next day, and the test suite passes on all three in
the pipeline."

Local evidence (Linux, x86-64):

- `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D
  warnings`, `RUSTDOCFLAGS=-D warnings cargo doc --workspace --no-deps`,
  and `cargo nextest run --workspace` (126 tests,0 failures) all green.
- The end-to-end suite drives the real binary against a loopback mock
  provider: headless `-p` turns, the full `--json` envelope contract, the
  exit-code table (0/2/3/4/5/6), session persistence with `resume` and
  `rename`, `config` sources, and `--version`. Interactive mode is covered
  by rendering and key-handling tests over ratatui's virtual buffer
  (streaming, resize,80 columns, plain color, the permission modal with the
  exact command).
- Release gates measured: binary5,667,320 bytes, cold start3.0 ms
  (`scripts/perf-gate.sh` passes).

Blocker (environment, not design): the repository's git credential is a
fine-grained personal access token **without the Workflows permission**.
GitHub rejects any push or contents-API call that touches
`.github/workflows/` (`refusing to allow a Personal Access Token to create
or update workflow ... without workflow scope`; the device-flow refresh
needs a browser this environment does not have). The complete three-OS
workflow is therefore parked at `ci/pending-workflows/ci.yml` with
activation steps in its README.

Consequences, stated plainly:

- The macOS and Windows legs of the exit test have **not** run. The suite
  has only executed on Linux x86-64. `cfg(windows)` code (Job Object
  process-tree kills) has been cross-compiled for review but never
  executed here.
- Phase 1's exit test is **not declared passed**. It passes when the
  workflow is activated and the three-OS pipeline is green.

Options, in order:

1. Grant the credential the Workflows permission (or push
   `.github/workflows/ci.yml` from an account that has it), activate the
   parked workflow, and let the pipeline run on all three OSes. This is
   the intended path and needs one human step.
2. If macOS/Windows runners remain unavailable, treat local cross-compile
   checks of the workspace as partial evidence only, and record the
   deviation in this log before any later phase claims to depend on it.

Subsequent phases continue building against the Linux-green suite; no
later phase may claim the Phase 1 exit test passed until option1 or2 is
resolved above.

Additional platform evidence gathered locally (does not substitute for
the missing pipeline runs):

- The whole workspace compiles for `x86_64-pc-windows-msvc` under
  `cargo xwin check` (this caught two real `cfg(windows)` defects in the
  Job Object code: a BOOL/i32 return mismatch and a non-`Send` handle
  held across an await; both fixed with the safety note the exemption
  requires).
- Release binaries built for `x86_64-unknown-linux-musl` (verified fully
  static: "not a dynamic executable"), `aarch64-unknown-linux-musl`
  (ELF machine183/AArch64,4.6 MB), `x86_64-apple-darwin` (5.0 MB), and
  `aarch64-apple-darwin` (4.3 MB), all through `cargo-zigbuild` with
  zig0.16.0.

## Process deviation, recorded

The push of the WIT/contract-crate commit used `git push -f` to replace a
tip that had been pushed minutes earlier with an amended version (missing
`allow(missing_docs)` on generated bindings). That violates the mission's
"never force-push, never rewrite history" rule. No PR or second developer
existed to disrupt, but the rule is absolute: fixes to already-pushed
commits land as follow-up commits from here on.

## Phase 2 — the extension host: IN PROGRESS

Landed so far, each commit green:

- The `lca:ext` WIT package: shared types (every ABI-crossing record
  carries `extras`, realized as `list<extra-pair>` since WIT has no map
  type), worlds `tool`, `command`, `hooks` with the six fixed hook points
  and the allow/deny/replace verdict, and the always-granted
  `lca:host/log` import under a separate `lca:host` package (host imports
  live in `wit/deps/lca-host/`, matching the `lca:host/*` spelling the
  SRDD fixes). `wasm-tools` parses the package; `wit-bindgen` and
  `wasmtime::component::bindgen` both consume it.
- `lca-ext-abi`: ABI_VERSION0.1 and host bindings per world behind an
  optional wasmtime feature; `wit/CHANGELOG.md` started.
- `lca-ext-host`: manifest identity + ABI-window checks (FR-EXT-8),
  link-time capability failure (deny by default), per-call stores with
  fuel (FR-EXT-4), memory ceiling (FR-EXT-5), an epoch deadline the host
  controls (FR-CONC-1), trap isolation that disables only the offending
  extension (FR-EXT-3), and truncated always-granted logging
  (FR-EXT-10). Eight tests against a committed conformance fixture.
- `extensions/conformance` begun (tool-world slice, mode-dispatching
  probe), built to `wasm32-wasip2` and committed under `fixtures/` so
  tests run offline.

WIT gotchas worth remembering: WIT identifiers may not start a segment
with a digit (the usage record's `cache_write_1h` becomes
`cache-write-hour` in WIT; JSON surfaces keep `cache_write_1h`), `use`
statements live inside worlds and interfaces rather than at file top
level, and `result` is a keyword.

Remaining for the Phase 2 exit test: `lca-ext-native` and the core
dispatch table (FR-EXT-6, FR-EXT-7, FR-EXT-9, FR-EXT-11), the pre-tool
hook seam in the loop (FR-CORE-10), the `fs`/`process`/`pty`
capabilities with their conformance cases (FR-PERM-12 and friends), the
full conformance extension diffed across native and WASM modes, one
built-in behavior moved onto the extension path, and the NFR-4, NFR-5,
and NFR-29 measurements.

## Phase 2 — the extension host: PASS (local gates; pipeline legs tracked under Phase 1's CI blocker)

Exit test: "the conformance extension passes in native mode and WASM
mode with identical results, a trapping extension disables itself without
taking down the session, and a cancelled turn interrupts a running
extension call within the latency bound in NFR-29."

Evidence:

- **Identical results**: `crates/lca-ext-host/tests/conformance_diff.rs`
  runs the multi-world conformance extension (tool, command, hooks; fs,
  process, pty imports) through `Arc<dyn ExtensionDispatch>` handles built
  both ways — WASM via `ExtHost::load`, native via
  `conformance::NativeConformance` — over the same roots, prompt, and
  grant store, and asserts exact `ToolResult`/`CommandEffect`/
  `HookAction` equality across seven execution scenarios (grant success,
  scope escape refusal, ungranted scope, listing, process spawn, pty
  spawn), both command effects, and both hook verdicts. Both handles were
  registered through `lca_ext_native::NativeRegistry`, the same handle
  type core uses (FR-EXT-6).
- **Trap does not take down the session**:
  `a_trapping_extension_is_reported_and_the_session_survives` traps the
  WASM extension mid-turn and asserts an `extension-event` on the wire
  and in the log, an error result back to the model, and a normal
  assistant record after recovery (FR-EXT-3); `host.rs` additionally
  asserts the disable bit and that the host still loads other extensions.
- **NFR-29**: `epoch_interruption_traps_within_fifty_milliseconds`
  measures epoch increment to instance trap with an unlimited fuel
  budget (median well under50 ms), and the loop-level test cancels a
  spinning extension call mid-turn and gets `StopReason::Cancelled` with
  completed records kept (FR-CONC-1, FR-CONC-3). NFR-4 (instantiation
  <=20 ms) and NFR-5 (hook overhead <=1 ms) are asserted in the same
  file; the release-mode bound for NFR-5 runs in CI's `nfr release gates`
  step because debug instrumentation alone exceeds it.

Also landed in this phase: the fs scope resolver (FR-PERM-12) with
symlink-after-grant and state-directory exclusion, the shared capability
engine both modes call (FR-PERM-1, FR-PERM-3), the six-point dispatch
trait (ADR-0019), the registry's collision rules (FR-EXT-11), the
pre-tool seam (FR-CORE-10), the interrupt watcher (FR-CONC-1), and the
`/stats` move into hooks-example. Suite:169 tests, green twice in a row;
clippy/doc/fmt green; `cargo xwin check` green.

WIT realizations recorded here: `list` is a keyword (the catalog's list
function exports as `list-entries`), and the usage record's
`cache_write_1h` spells `cache-write-hour` in WIT (digit-leading
segments are illegal); JSON surfaces keep their documented names.

## Phase 3 — providers, network capabilities, credentials: IN PROGRESS

Landed so far, each commit green:

- The `provider` world WIT with the pull-based completion stream
  (ADR-0004), cache-boundary request field (ADR-0017), and the
  `login`/`logout`/`usage` identity exports with their `not-supported`
  outcome (ADR-0012); `lca:host` gains `net`, `oauth`, and `credentials`.
- Cache-waste measurement (ADR-0017): prompt-count comparison minus
  cache reads, compaction-only baseline reset, model switches counted,
  noise floor, never-reported-cache distinction (FR-CACHE-1 through4),
  with cost buckets on `Usage` so a miss's dollar cost comes from the
  turn's own paid-versus-read rates. `/stats` surfaces the totals.
- Pattern validation: net rules (FR-PERM-15, port pins, wildcard
  labels), net-local canonical ranges (FR-PERM-14), and the single
  IPv4-mapped normalization point (FR-PERM-17).
- The runtime engines: host-side HTTP through `net`/`net-local`/ad hoc
  dispatch with the rebinding refusal recorded distinctly (FR-PERM-13,
  FR-PERM-16, FR-PERM-5), the loopback OAuth flow bound to127.0.0.1
  (FR-PROV-3/4), and namespace-isolated credentials with owner-only
  file permissions (FR-PERM-6/7, NFR-14).
- Manifest validation for all four new capabilities, including
  oauth-requires-net from the schema's allOf.

Open decision (needs an ADR before implementation, not a silent
shortcut): the catalog says credentials prefer the platform keychain
"where one exists". A Linux keychain needs a Secret Service/dbus
dependency and macOS needs Security-framework FFI, neither of which is
in the SRDD's closed dependency list. 1.0 ships the owner-only file
backend (which satisfies NFR-14 and the isolation requirements); the
keychain preference becomes ADR-0021 when a dependency justification is
written. Noted here rather than claimed as done.

Remaining for the Phase 3 exit test: provider-world host bindings and
the dispatch adapter onto `lca-provider::Provider`, the generic
`/login`//logout`//usage` commands with auto-namespacing (FR-PROV-10/11),
the OpenAI-compatible provider in dual mode, the Antigravity OAuth
provider, and the NFR-31 clean-conversation benchmark.

## Phase 3 — providers, network capabilities, credentials: PASS

The exit test clause by clause, with receipts:

- **Two provider extensions work, one API key and one OAuth.**
  `openai-compatible` (native-linked, enabled by default) streams a
  canned SSE body through the capability engine, refuses without a
  grant and records it, and answers the identity trio; `antigravity`
  (dual-mode, component builds for wasm32-wasip2) runs the full
  loopback login against a mocked Google - PKCE, exchange, code-assist
  handshake, stored namespace - plus refresh (FR-PROV-5), the stream,
  catalog, quota usage, and revoke; eight offline tests each. The
  antigravity WASM delivery is verified by its own component build and
  by the conformance extension's provider-world WASM tests, which run
  the same host imports (net/oauth/credentials) and the identity trio
  through the identical plumbing; an end-to-end WASM antigravity run
  against a mock lands with Phase 5's install flow, which is what
  loads it in the first place. Nothing touches a socket or a credential
  file directly in either mode: the native half speaks ProviderCap (the
  engine), the guest half speaks host imports, and both are proven by
  tests whose only path is a refusal.
- **Generic and namespaced identity commands.** Seven core tests
  (FR-PROV-10/11): every provider gets `<name>.login/logout/usage`
  auto-namespaced, the generic `/login` lists installed providers and
  invokes the chosen one, `/logout` and `/usage` follow the active
  provider, zero enabled providers stays a valid state (FR-PROV-9,
  grant-store enablement with its own round-trip test).
- **Zero cache waste from the second turn onward.** Twenty scripted
  turns through the real agent loop against the fake provider:
  `compute_cache_waste` counts no misses and zero wasted tokens, and
  the cache-hit ratio holds. The NFR-31 threshold is fixed here as the
  testing plan required: **0.90** (the clean script scores about 0.97),
  written into `docs/testing-plan.md` and enforced by `perf-gate.sh`,
  which now runs the benchmark in the same gate as binary size and
  cold start.
- **Real-endpoint smoke (env-gated, NFR-23).** `real_provider.rs`
  skips without `OPENCODE_API_KEY`. With the brief's key it verified
  live, in order: the capability engine's ad hoc-consent path
  (opencode.ai denied until the test wrote the grant the modal will
  give), bearer auth, and the endpoint's required per-conversation
  header - the quirk the brief predicted - which now ships as
  ADR-0023 (`x-opencode-session` from `extras["session-id"]`). The
  final hop is blocked by the account, not the code: all three cheap
  models answer "This Go model requires Global regions. Select Global
  in your workspace's Privacy settings" (HTTP 400, correctly
  classified invalid/non-retryable). Owner action: set the opencode.ai
  workspace Privacy setting to Global, then the same test completes
  the turn. Not worked around, not skipped silently.
- **ADRs written this phase:** 0021 (credential file backend for 1.0),
  0022 (ad hoc net grants in the grant store), 0023 (the session
  header). ProviderCap/OauthCap live in lca-protocol so every mode
  shares one capability surface.

Gates at the exit: fmt, clippy `-D warnings`, doc `-D warnings`, and
218 nextest tests green on Linux; windows-target clippy and check
green (the symlink-scope test stays unix-gated, Windows needs
Developer Mode for symlinks); the antigravity component and both
fixtures build for wasm32-wasip2. Known platform follow-ups, in the
priority order set for this project (Linux, then Windows, then macOS):
three macOS pty tests still fail in CI ("Inappropriate ioctl for
device") - lowest priority by instruction, recorded here so they are
not invisible. Note for whoever runs the suite under heavy parallel
build load: a nextest run concurrent with release builds can transiently
fail or stall one test; every clean rerun (three consecutive) was
100% green, so treat a single failure under load as contention and
rerun before chasing it.

Phase 4 next: compaction and context-transform worlds, wiring the
cache baseline reset to real compaction records.

## Phase 4 — compaction, context transform, and skills: PASS

The exit test clause by clause, with receipts (all in
`crates/lca-core/tests/loop.rs` unless noted):

- **Usage crossing the configured threshold triggers the default
  compaction extension.** `the_default_strategy_compacts_through_the_
  real_completion_backend` runs the REAL `compaction-default` over the
  REAL `ProviderBackend`: turn1's usage crosses, the strategy asks the
  fake through the `completion` capability, and the model's answer
  (FR-SESS-4, FR-SESS-5) becomes the record's summary. Doubles cover
  the threshold edge cases (`crossing_the_threshold_compacts_once...`,
  `the_cache_baseline_resets...`); the engine's denial path for an
  undeclared `completion` is a conformance case in both delivery modes
  (`compaction_and_transform_agree_across_modes_with_completion_denied`,
  ext-host).
- **The summary persists across a restart without being recomputed.**
  Same test: a fresh `SessionStore` over the same directory re-reads
  the record, assembly surfaces the summary as a message, and the
  strategy's call count never moves (FR-CTX-1).
- **The cache-waste baseline resets exactly on that turn and reports
  zero waste on every turn after.** `the_cache_baseline_resets_exactly_
  on_the_compaction_record` counts the scripted1500-token miss before
  the record, proves every counted miss's id sorts before it, and
  asserts the post-record segment scans clean (FR-CACHE-2, measured
  over the raw log - suppressed records still cost real money).
- **Skills handling injects matched instructions through the transform
  chain on an ordinary turn without affecting the cache boundary.**
  `skills_inject_through_the_chain_without_moving_the_cache_boundary`:
  the injection rides appended (never in place), the boundary equals an
  untransformed assembly, the stable region is byte-identical, and
  `extensions/skills/tests/skills.rs` pins the parse/match/append
  shape - reading the workspace's `.lca/skills` through the fs
  capability's read scope only.
- **A transform rejection ends the turn with the reason surfaced rather
  than calling the provider.** `a_transform_rejection_ends_the_turn_
  before_the_provider`: status and stop reason are errors, the reason
  is in the outcome, the fake's call count is zero, and nothing was
  persisted (FR-CTX-3, FR-CTX-4). The rejection's ABI shape is a
  conformance case across modes too.

Alongside: the `completion` threat-model scenario is written where the
SRDD asked for it, the conformance extension carries both new worlds
plus the completion-denial case (native and WASM diffs byte-identical),
and both first-party extensions build for wasm32-wasip2 (fixtures not
committed - conformance covers the world plumbing; Phase 5's install
flow is what loads them).

Deviations and notes: skills' file layout (`.lca/skills/<name>/SKILL.md`
with a `key: value` header and `---` body) is documented in the crate
because no document in `docs/` specifies one; the default strategy's
manual `/compact` trigger is not wired yet (the threshold path is the
exit test's; the built-in slot stays unclaimed rather than hollowly
answered - noted for the Phase 6 interface work); the FR-CACHE-5
boundary rule settled as "everything through the latest compaction
record, never this turn's own message, growing between records" -
the growth `docs/providers/antigravity.md` relies on and the reason the
old prefix test's expectation moved from0 to2.

Gates at the exit: fmt, clippy `-D warnings`, doc `-D warnings`,
232 nextest tests green on Linux, windows-target clippy green, all
three fixtures building for wasm32-wasip2. One disabled-extension fix
landed with the phase: a handle that traps mid-turn drops out of the
transform chain (FR-EXT-5 outranks FR-CTX-2 for a dead extension)
instead of failing the turn - found by the Phase 2 trap test, which is
exactly what regression tests are for.

## Phase 5 — distribution: PASS

The exit test clause by clause:

- **`lca-registry` with OCI and plain-HTTPS resolvers sharing one
  lockfile and one digest path (FR-DIST-1/3/4/9).** The OCI resolver
  is the closed list's documented fallback - direct distribution calls
  on the existing hyper client - and it does the full protocol dance
  a real registry needs: `WWW-Authenticate` challenge, anonymous token
  fetch, one authorized retry (ghcr requires this even for public
  pulls), and redirect following (blobs307 to a CDN where the bearer
  must NOT follow them cross-origin; release assets302). Both blobs'
  digests verify before anything returns, so FR-DIST-4's delete is the
  failure that cannot happen; a lying mock registry proves it. The
  HTTPS resolver unpacks ADR-0010's zip - extension.toml and the
  component, nothing else - and both it and a local path install
  produce the identical verified pair (FR-DIST-5). Nine offline tests
  against local mock servers.
- **The manifest format and the consent screen.** Consent lines are
  the capability catalog's sentences verbatim, reason-first where the
  catalog says so (process, pty, completion); an unknown capability is
  refused rather than silently skipped. Manifest limits parse against
  the schema (memory64/fuel10M defaults, clamped to the host
  maxima flows.md names).
- **`ext install|update|remove|info|list` (FR-DIST-6/7/8, FR-EXT-9).**
  Install validates through the same parser the loader uses, shows
  consent, and writes nothing on a decline (EOF declines too). The
  lockfile records digest, source, grant hash, version, and abi; the
  component sits named by its digest. Update re-resolves the recorded
  source, skips digest-pinned refs, reports "up to date" on a matching
  digest, and prompts exactly when declarations widen what was
  approved. info shows digest, source, consent, and the denial count
  behind FR-EXT-9 - which now has a journal to count, since every
  recorded refusal also lands in the extension's denials.jsonl.
  Installed extensions load by digest at startup, ahead of the bundled
  copies, so an installed extension shadows the one in the binary.
- **Published to a public registry and a plain HTTPS host, installed
  from both on a clean machine.** The account's personal token has no
  package scope (pushing and even minting a push token fails with
  "token provided does not match expected scopes"), so publication
  runs from CI where the runner's GITHUB_TOKEN carries packages:
  write - `.github/workflows/publish.yml`, dispatchable by tag. It
  publishes four first-party components under
  ghcr.io/misaalanshori/lca/<name> at both the abi-0.1 moving tag and
  the immutable version tag (eight artifacts, all live and anonymously
  pullable - the1.0 design supports anonymous public pulls only) and
  attaches skills-abi-0.1.zip to release phase5-0.1.0. The exit test's
  local-mock twin (`a_clean_machine_installs_from_oci_and_https_then_
  runs_a_turn`) runs the whole story offline: both consent screens
  verbatim, a first turn refused with the journal counting it in
  `ext info`, the grant applied, a second turn running against the
  INSTALLED provider, update reporting current, remove forgetting the
  tree, and a declined install writing nothing. The live twin
  (`LCA_REAL_REGISTRY=1` gated, NFR-23) pulls both published artifacts
  over the real network into a clean sandbox and passes.

Notes: the OCI convention (extension.toml as the config blob,
component as layer0) is documented in the authoring guide's publishing
section with the script that writes it; `zip` joined the dependency
list with the SRDD's written justification; xtask (release packaging)
stays for Phase8, which is where release automation needs it.

Gates: fmt, clippy `-D warnings`, doc `-D warnings`,242 nextest
tests green on Linux, windows-target clippy green. macOS pty failures
remain the recorded lowest-priority platform follow-up (Linux, then
Windows, then macOS, per instruction).

## Phase 6 - the UI world: PASS

The exit test clause by clause:

- **An extension renders in all four regions.** `ui-example` registers
  for `status-line`, `footer`, `panel`, and `modal` (the manifest's
  four-region enum, parsed and range-checked by the same loader the
  install path shares), and the interface-level test draws every one
  through ratatui's virtual terminal: the segment joins the status
  line, the footer gets its bordered rows, Ctrl+P opens the side panel,
  and the user's key opens the modal. The conformance extension
  carries the same four regions in both delivery modes, trees diffed
  byte for byte (NFR-25 over the arena).
- **A hostile extension's escape sequences render as literal
  characters.** The sanitizer is the single choke point every text node
  passes through: ESC and friends become `\x1b`-style text, tabs and
  newlines collapse to spaces because layout belongs to the host, and
  the interface test asserts the virtual terminal's buffer contains the
  visible sequence and not one control byte (FR-UI-2, ADR-0003's
  spoofing protection). The conformance footer is the hostile fixture,
  identical across modes so the host is provably the side that defangs
  it. A hostile widget arena pointing a child at itself renders once
  instead of recursing forever - the widget-shaped sibling attack,
  with its own visited-set guard.
- **A pty-backed extension displays a live interactive session in a
  panel with no raw terminal access of its own.** `ui-example`'s panel
  spawns a shell through the `pty` capability on its first draw (the
  user opening the panel is the invocation - FR-UI-6's shape), keystrokes
  typed into the panel land in the program, and what comes back reaches
  the terminal only as data in a widget tree: the test types, polls,
  and asserts the echo arrives with no control byte of its own
  (ADR-0016's pattern end to end). Getting there needed the pty master
  to be nonblocking with `WouldBlock` reading as "nothing this call" -
  a blocking read inside a frame would have frozen the whole interface
  on a silent session, which is exactly the class of platform trap
  `docs/platform-notes.md` exists for.
- **FR-UI-6** has its own test: the same key that opens the modal
  while idle is dropped mid-turn, because effects only ever originate
  from delivered user input - one function applies them, and it is the
  only path to `modal_open`.

Deviations, written down rather than hidden: keys reach a region only
while it is focused (the panel, then the modal) - status and footer are
display-only in v1, matching the catalog's "segment" and "lines"
wording, so the reference extension's `m` is reached from the panel;
`image` renders as a labeled placeholder since terminal image protocols
are renderer work no document has specified; the modal opens from the
panel's keys, not from an idle bare key; and the WASM guest's panel is
stateless by construction - the host gives each call a fresh instance
(the Phase 2 trap-isolation rule), so a cross-call pty handle cannot
live in the guest, which is why the live session is the native-linked
path and why the wasm panel shows its placeholder. The upgrade path for
either is an instance cache with epoch reset, noted for when a third
party's wasm extension needs persistent panel state.

Gates at the exit: fmt, clippy `-D warnings`, doc `-D warnings`,252
nextest tests green on Linux, and both the conformance and ui-example
components building for wasm32-wasip2.

