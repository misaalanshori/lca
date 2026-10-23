# Changelog

Notable changes to LCA. Versions follow semantic versioning for the product;
the `lca:ext` ABI version is independent and is printed by `lca --version`.
Dates are UTC.

## [Unreleased]

### Fixed
- **`--mode rpc` no longer loses a turn when a command arrives mid-turn** (gh
  #489). The loop drove its turn with `run.take()` inside a `tokio::select!`
  branch future, and `select!` rebuilds every branch future on each iteration — so
  a stdin line arriving while a turn was streaming made the select drop the future
  holding the taken run, and `turn_state`, which had moved into that same future,
  died with it. All four promises the docs make broke at once: `prompt` answered
  `"started"` for a run that never called the provider, `follow_up` and `shutdown`
  had no run end to hang their queues off, and every later `prompt` on that session
  was a silent no-op. Measured on the shipped happy-path script: one turn lost in
  40 idle, **6 in 40 (15%) under load**, and 100% when a second line was written at
  any non-zero delay. The future is now a pinned local the select polls in place
  (`run.as_mut()`), cleared only by the arm that observed completion — so a command
  arriving mid-turn costs a poll, never a turn. Six rows in `e2e_rpc.rs` pin the
  four promises and both flanking cases; all six fail on the old code. The two rows
  quarantined under gh #195 ("turn-ends dropped under load") were this same bug —
  they are un-quarantined and green.

- **The extension authoring guide teaches a route that exists, and its example
  builds** (gh #439). It told authors to run `wkg get lca:ext@0.6.0 --format wit
  --output wit/`, and **nothing serves that package** — no registry is named
  anywhere in the repository, and the release pipeline publishes built
  *components* to `ghcr.io/misaalanshori/lca/<name>`, never the WIT package. An
  author following the guide hit a 404 at the first step. It now documents the
  path that works — vendor `wit/` from a checkout at the tag you target — and says
  the three things that decide which copy to take: the package line names the ABI
  line (`0.6`) and patches do not move it, `wit/` is the whole contract with
  `deps/lca-host/` included, and **a release tag's `wit/` is a snapshot that can
  lag the line** (the `v0.6.0` tag predates the `lca:host/net` import, so a
  copied `with:` block from the current line is refused as an unused remapping).
  The guide's example code was also wrong in four ways that each stop a build —
  `schema`/`execute` where the worlds export `get-schema`/`run`, `ToolResult::text`
  where it is a plain record, a missing `with:` map, and a `bindings` module that
  does not resolve. Every command and the whole example are now run from a
  scratch directory and pasted in the report, including the guest building for
  `wasm32-wasip2` from the vendored `wit/` and registering in a real host.
  Publishing the package itself is **not** done: `wkg publish` needs a registry
  namespace matching the package identity (`lca:ext` → repository `lca/ext`) and
  this project publishes under `misaalanshori/lca/<name>`; that is tracked, and
  the guide says so instead of pointing at a server.
- **Running `lca` from your home directory no longer reads your own config as a
  project's** (gh #481). From `$HOME`, `<cwd>/.lca/config.toml` and
  `~/.lca/config.toml` are the same file, so a documented user setting
  (`trust.default_project`) was classified as a project override — and FR-CFG-7
  correctly refuses that in a project file, which made a valid user config a fatal
  startup error. The two paths are now compared **canonically**, and when they name
  the same file there is no separate project layer: the setting loads, takes
  effect, and `lca config` / `/settings` attribute it to the user file. A *real*
  project file is still refused with the same message, so FR-PERM-9 is untouched.
  Two more paths with the same root cause came out of the sweep and are fixed too:
  the `/settings` picker no longer offers to rewrite your own keys as project
  overrides, and `mcp list` no longer loads your servers twice and label them
  `project`.

- **An audit test now enforces that every extension declares the environment it
  reads** (gh #484, follow-up to #461). A WASM component is instantiated with an
  empty `WasiCtx`, so `std::env::var` inside it answers `NotPresent` unless the
  manifest lists the name under `[capabilities.env]`. #461 found `openai-compatible`
  reading eight variables and declaring none; the audit that found it named four more
  manifests, and auditing by hand is how that issue came to exist — so the guard is
  now a test over **every** `extensions/*/extension.toml`, comparing each crate's
  literal env reads against its manifest and failing with the provider and the
  missing name. It catches the fifth instance on the day it is written. The four
  named manifests (`anthropic`, `antigravity`, `llama`, `openrouter`) turned out to
  be **deliberately** native-only: their reads sit under
  `#[cfg(not(target_arch = "wasm32"))]`, so the sandboxed build never executes them,
  and granting `env` would forward a secret the guest has no code to read. Their
  pages now say so — `anthropic`'s had gone stale ("that grant waits on #170", which
  landed two cycles ago) — and an artifact-level check confirms the sandboxed
  builds contain none of the secret variable names, while `openai-compatible`'s
  contains all eight it declares.

- **`/mcp` → Sign in no longer kills the whole session** (gh #476). Selecting *Sign
  in* on a remote server died deterministically — no UI, session gone — with
  `Cannot start a runtime from within a runtime` from the capability layer.
  `Capabilities::drive` read the ambient runtime handle and called `block_on` on it,
  which is legal from a worker thread and **panics** from inside a runtime *poll*;
  `Handle::try_current()` succeeds in both cases, so the old code could not tell them
  apart and documented the gap as an assumption ("every current caller is inside a
  blocking region") that a TUI callback breaks — `main` holds `Runtime::block_on` on a
  multi-threaded runtime, so every UI closure runs inside that poll. The fix is at the
  seam rather than in the caller, because `oauth_flow_net_request` is not special:
  `net_request`, the response-body readers and the `process` spawn path all drive
  through the same function. `tokio::task::block_in_place` re-enters a blocking region
  on a multi-threaded runtime, after which `block_on` is legal — tokio's own documented
  pairing — and it keeps the work on the **ambient** runtime, so a connection's
  background task is not orphaned. A current-thread runtime can neither do that nor
  block from inside its poll, so that case is refused by name instead of unwinding.

- **A pty read waits for output instead of spinning on it, and a fast-exiting
  program no longer loses what it printed** (gh #477). Two defects shared one
  site, which is why one CI failure looked like a flake. The **busy-spin**: the
  host answered "nothing yet" with an empty chunk, so a caller looping until
  end-of-stream re-called with no wait — measured at **614,104 read calls in a
  second** on this box, all but a handful of them empty. On a loaded runner that
  stole the core from the very program whose output was being awaited. The
  **readiness race**: the loop treated the first end-of-stream as final, but a
  pty master reports its end on terms of its own — the child exiting does not
  mean the bytes it wrote have been drained, which is exactly the `exit 0 ` with
  no output that macOS CI saw. The host now waits on the master (a readiness
  window, not a delay) and answers "the session ended" only on the brief's rule:
  **child exit plus a drained buffer**, with a short grace window for a tail that
  is still arriving. That drops the spin to **126 calls/second** — a ~4,900×
  reduction — and the failing row now passes six times out of six on a box under
  load 14. Nothing about the pty contract changed: `read` still returns `None`
  when the session ended, which was always the promise; the host was simply
  answering it from one syscall's point of view.
- **Tool annotations decode in the spelling everyone actually sends** (gh #454).
  `ToolAnnotations` derived `Deserialize` with no rename, so serde expected
  `read_only_hint` while every real payload says `readOnlyHint` — the MCP spelling,
  and the spelling a WIT guest puts in `extras["annotations"]`. A camelCase payload
  decoded to **all-`None`** without an error: the hints were dropped, not rejected.
  That is why an earlier measurement saw "hints arrive" while nothing arrived, and
  why the permission modal never showed them. The type now emits camelCase (the
  interop spelling) and accepts camelCase or the older snake_case, so a guest
  written against the previous documentation keeps working. An unknown key is
  ignored rather than fatal, deliberately: a hint is display metadata the permission
  layer never decides on, so a hint MCP adds later cannot break a peer. The new rows
  all **start from JSON text** — the previous ones built the struct in Rust, which
  is why a decode bug survived them.

- **A provider declaring `[capabilities.net-local]` now works as a built-in, not
  just as a `.wasm`** (gh #456). The native provider path assembled its capability
  grants as a partial literal and let `..Default::default()` absorb the rest, so
  it silently dropped `net_local` *and* `net_declared` while the WASM host copied
  both. #294 had just given `openai-compatible` exactly that grant for its
  `ollama`/`lmstudio` presets, so the native build's local-endpoint discovery was
  broken — the worse half of the #170 class, because a capability went *missing*
  rather than a feature. Both paths now call one constructor,
  `Manifest::capability_grants`, whose exhaustive literal makes a new grant field
  a compile error instead of a silent omission in one of two places; a
  field-by-field parity row over a manifest that populates every capability guards
  the behaviour. Two consequences worth knowing: a loopback endpoint no longer
  raises the redundant second consent prompt (the manifest already declared the
  reach and the user approved it at install), and a consent-first provider that
  ships `hosts = []` on purpose no longer risks "no network capability declared"
  on its first request.
- **The `openai-compatible` provider now declares the environment it reads, so the
  sandboxed build can see it** (gh #461, follow-up to gh #170). A WASM component is
  instantiated with an empty `WasiCtx`, so `std::env::var` inside it answers
  `NotPresent`; the host forwards only the names a manifest lists under
  `[capabilities.env]`. This provider read eight of them — the base URL, both key
  names, the model, the context window, and two behaviour switches — and declared
  none, so the `.wasm` build behaved as if nothing were configured while the
  compiled-in one read the host environment directly. That divergence is what the
  deleted `openai_ad_hoc_host()` workaround (gh #157) had been papering over: with
  it gone, `OPENAI_API_KEY` set and granted still produced *"no API key is
  configured"* from the guest. The eight names are now declared, the install consent
  names each one, and a row asserts the declared set is **exactly** the literals the
  crate reads — so a future broad grant shows up as a diff, not as a consent screen
  nobody reads. One limitation is documented rather than discovered: a named
  profile's key comes from the `provider-presets.toml` resource at run time, so under
  the sandbox it resolves from the stored credential instead (the native build still
  reads it).

- **Launching from `$HOME` no longer warns that every installed extension is a
  broken project extension** (gh #463). The project-extension directory is
  `<cwd>/.lca/extensions` and the data home is `~/.lca/extensions`; from `$HOME`
  they are the same place, so the scan read the install tree — whose components are
  named `sha256-<digest>.wasm`, never `component.wasm` — and printed
  `warning: skipping project extension ...: no component.wasm` for every globally
  installed extension, then loaded it successfully. Wrong twice. The scan is
  now skipped when the two directories are the same place, compared **canonically**
  (a symlinked `$HOME`, macOS's `/var` → `/private/var`, and Windows's `\\?\`
  prefix all defeat a string compare — the third cycle in a row for that class). The
  same confusion on the project-trust prompt is closed with it: from `$HOME`, the
  installed extensions no longer make the TUI offer "Trust this project folder?". A
  **genuine** project extension missing its component still warns, and a row pins
  that so the diagnostic cannot be disabled by accident.

- **A flag whose value is a program — or a shell string — now reviews instead of
  auto-approving** (gh #458). The same class gh #347 closed for evaluator flags and
  gh #365 closed for positional program text, reached another way. `vim -c ':!id' f`,
  `vim '+!id' f`, `ex -c '!id' f`, `ed f` and `tar xf a.tar --to-command=id` all ran
  with no prompt in a trusted folder. Archive flags are refused as a measured set:
  `--to-command` (whose value is shell-interpreted — `--to-command='id; echo PWNED'`
  runs both), `-I`/`--use-compress-program`, `-F`/`--info-script`,
  `--new-volume-script`, `--rsh-command`, and `--checkpoint-action=exec=` — five
  flags where the issue named one. An editor's `-c` is a command language
  (`:!`, `:r !`, `:g//!`, `:python`, `:terminal`), not a set, so the form reviews
  wholesale, `+cmd` included (it is the same hole — measured). `ed` reviews
  structurally: its whole language arrives on stdin. Ordinary use is untouched —
  `vim file.txt`, `vim -S script.vim`, `tar xf a.tar`, `tar czf b.tgz dir`,
  `tar xf a.tar -C out` all still auto-approve, and interactive editing through
  `$EDITOR` never reaches the analyzer. A refusal names the reviewable form
  (`-S` with a script file in the project), and a row asserts that advice is not
  itself a bypass.

- **OAuth discovery's last-resort fallback works, so a server with no RFC 9728
  document can still sign in** (gh #467). `origin_of` split a URL with
  `splitn(4, '/')` and rejected the second piece — the empty string *between the two
  slashes of `//`*, empty for every well-formed URL — so it could never succeed. It
  is the third step of discovery: configured metadata document, then the challenge's
  `resource_metadata` pointer, then the MCP server's own origin as its own issuer. So
  every server whose IdP advertises no RFC 9728 metadata, with no
  `authServerMetadataUrl` configured, got `cannot take the origin of …` instead of a
  sign-in. It now parses (scheme, then the authority cut at the path/query/fragment,
  userinfo stripped, IPv6 brackets kept), and the fallback branch has its own row —
  the branch nothing covered, which is how a function that cannot succeed survived.
  The cross-host sign-in row no longer depends on which discovery step happens to win
  on a given OS.

- **A silent MCP server that fails the handshake no longer hangs on `connecting`**
  (gh #459). `lca mcp list`, `/mcp`, and the startup wait all reported `connecting`
  until the 10-second budget expired and then still reported `connecting`, with the
  server's actual error never shown. The cause was the good kind of irony: the
  failure path wraps the error with the child's stderr tail so a chatty crash
  explains itself, and that tail read **blocked** on a live child whose stderr pipe
  was empty — no EOF, no data — so the path that exists to surface an error hung
  before it could surface one, and the worker never landed a row. The tail read is
  now non-blocking (`O_NONBLOCK` on Unix, `PeekNamedPipe` on Windows, both already
  in the tree for the pty capability): a live child with nothing to say returns
  immediately, a closed pipe still drains to the end, and a partial line still comes
  back. The ABI's `read-stderr` import keeps its blocking semantics — an extension
  asking for output must still wait for it — so the bounded read is a second path,
  not a quiet change under the first. A row still `connecting` after the budget now
  says so ("did not answer within Ns") instead of leaving a bare label that can
  outlive every deadline. The chatty-crash case is unchanged and still reports its
  tail - and now reliably: a child that has **exited** is drained to EOF (safe,
  because it will close its pipe), so a dying server's last stderr write is no
  longer lost to the race between the write and the read (measured through
  `lca mcp list`: 2 of 5 runs lost it before, 8 of 8 keep it after).
- **The footer's `tok/s` reading is a measurement again, not a report on the
  interface thread's scheduling** (gh #464). A live Antigravity session printed
  `5263158 tok/s`: `↓30` output tokens over a span of 5.7 microseconds. The meter
  opened its `Instant` window in `Chat::on_turn_event` — the interface thread — and
  that thread drains a bounded channel in a burst, so a finished response's deltas
  all arrived within microseconds of each other. Meanwhile hidden reasoning tokens
  were counted in the numerator and their generation time was not in the
  denominator. Both are fixed at the root: `lca-core` stamps each provider call's
  generation span (first delta to last, text *and* reasoning, tool runs excluded by
  construction because a tool runs between calls) where the stream is consumed, and
  carries it on that call's `Usage` as `extras["generation_us"]`; the interface sums
  the stamps and divides, holding no clock of its own. A physically impossible
  reading now renders **nothing** rather than a wrong number — clamping to a
  believable ceiling would still be a lie — while a turn that measured nothing
  leaves the previous reading standing. Token counting is untouched.

- **Switching tabs no longer cancels the running turn** (gh #462). #209
  promised that a turn keeps generating when you switch away — its tab item
  pulses, and an unread marker appears when it settles — but the shipped
  implementation refused every tab move mid-turn instead, so a 30–120 s
  `cargo test` held the whole interface hostage. A turn now belongs to the
  **tab** that started it: `begin_turn` pins the owner, each tab carries its
  own live transcript, steering queue, separator, meter, and session totals,
  and the visible tab's copies are swapped in and out on a switch
  (`mem::swap`, so a switch moves pointers rather than copying a
  conversation). A background turn's deltas therefore land in its own
  transcript and its spend lands on its own session — neither leaks into the
  tab you are reading. Three narrower rules came out of making that safe, and
  each is pinned: `Ctrl+C` cancels only the turn of the tab you can see (a
  keystroke that silently kills invisible work is how twenty minutes of a
  test run disappears); **closing** a tab whose own turn is running refuses,
  because the stream would land in a removed row; and **opening** a tab
  mid-turn refuses, because minting a session rewrites the host's
  current-session cell, which the running turn worker reads — a switch can be
  deferred to settle, a creation cannot. Tab moves are otherwise free, and
  the tab bar finally shows the `◌` and `•` markers #209 asked for.

### Added
- **`/persona` sets a model, thinking level, tool loadout, and instructions in one
  command** (gh #412). A persona is a named bundle under `[personas.<name>]` in the
  config, so gear-shifting from "planning with read-only tools at high reasoning" to
  "fast local model, no tools" stops costing four commands and a pasted paragraph.
  `/persona` with no argument lists every persona with what it carries; `/persona
  <name>` validates the **whole** bundle against the running session — the model is
  offered, *that model* accepts that thinking level, every tool is registered — and
  applies only if all of it holds. **A persona that fails any check applies
  nothing**, not even the fields that would have worked: a half-applied persona
  leaves you believing you are in one you are not in. Applying writes through the
  same paths `/model` and `/thinking` use, so the footer, the thinking border ramp,
  and the per-model overrides stay in sync with no second state to keep in step. A
  project file's persona replaces a same-named user persona wholesale — a bundle,
  not mergeable fields — and an untrusted project's personas never load at all
  (FR-PERM-9), since a persona names a model, a loadout, and instructions and is
  exactly the config-driven switch an untrusted file must not have. Instructions
  ride as an attributed **conversational** message, never a `System` one (gh #299:
  a trailing `System` message mutates the root prompt and busts the cache). The
  word *preset* stays reserved for provider endpoint entries, so this is `/persona`
  and `[personas]` everywhere.

### Fixed
- **The `/bug` bundle is scrubbed before it is written, and you review it first**
  (gh #411 gaps 3 and 2). `/bug`'s trust foundation was that the credential
  *store* never enters the bundle — but a key **pasted into the chat** sat in
  `session.jsonl` raw, and that bundle is made to be shared, so "no secrets" meant
  "no *stored* secrets". `/bug` now stages a scrubbed **copy**, shows you what is
  in it, and writes the zip only when you confirm. Secret-shaped strings become
  `<redacted>`: API keys, GitHub/Slack/AWS/Google token prefixes, bearer tokens,
  JWTs, OAuth `code=`/`access_token=` values, and any long opaque value beside a
  word like `api_key`, `secret`, `token`, or `authorization`. The surrounding
  sentence, roles, timestamps and tool names survive, so the bundle still debugs.
  The inspection lists the staged path, every file with its size, and how many
  matches of which kind were scrubbed — counts and kinds, never the values, not
  even shortened — because a silent scrub leaves you unable to judge whether it
  caught the thing you were worried about. `/bug` errs toward scrubbing: a false
  redaction costs a maintainer one round-trip, a missed one leaks a credential.
  `[n]`/`Escape` discards the staged copy entirely, and **your own session log is
  never touched** — the bundle is a copy, so the transcript you resume from still
  has everything. A staging or write failure is loud and removes the staging
  directory rather than shipping a half-scrubbed bundle. The credential store is
  still never read (gh #57's decoy row passes unchanged).
- **Key hints name the key you actually bound** (gh #410). `key_text(action)` was
  documented as producing hints that "cannot lie when the interface is rebound" and
  built a fresh **default** keybinding manager on every call. All fourteen hint
  sites used it, so rebinding `app.tools.expand` to `ctrl+e` left every truncation
  hint, the jump pill, the reasoning readout, the startup line and the mouse
  tooltips printing `ctrl+o` — while the key itself no longer worked: the label lied
  in exactly the situation the comment promised it could not. Hints now resolve from
  the interface's **live** manager, which is threaded to each site rather than
  rebuilt per frame (no new I/O; the #389 lesson). The free helper is gone, not
  deprecated: `default_key_text` is named for what it is and nothing in the UI uses
  it, and the one hint formatter *requires* a manager so the 15th call site cannot
  quietly reintroduce the shape. Formatting follows pi: the key in `dim`, the
  description in `muted`, multiple bindings `/`-joined, a chord keeping its `+`
  parts, and `option` for `alt` on macOS. An action the user unbound renders **no**
  hint rather than a blank key press. `/reload` repaints every hint on the next
  frame — including the startup line, which was previously baked as pre-rendered
  text and needed to become a live entry to follow a rebind. The transcript's render
  cache key was the width alone, which was unsound once a hint's text depended on
  the binding, so cached frames now carry an epoch bumped on rebind.

### Added
- **The real-terminal harness can wait for readiness instead of guessing at it**
  (gh #447). `main` had been red on most pushes since batch-13, and the shape was
  one class rather than five bugs: a row asked the pane one question and treated
  the answer as complete. `wait_for(head)` returns the frame the head was painted
  in, so a row asserting a *second* glyph read two moments as one
  (`e2e_picker_scroll`'s `╭`, `e2e_tree_navigator`'s pair of connectors,
  `e2e_color`'s separator); and `[session in` is painted by the first render,
  which says nothing about the input thread, so a scenario typed in that gap was
  lost with no trace to poll for (`e2e_slash_output`, `e2e_model_consent`'s
  0.82 s failure). Three primitives in `tests/common/readiness.rs` close both
  classes once: **`wait_until_painted`/`wait_settled_for`** capture until two
  consecutive frames are identical and assert on that frame; **`wait_for_input_ready`**
  types a unique probe, waits for it to be echoed through the live input path, and
  erases it so it never becomes the scenario; **`retry_once`** promotes the
  `conformance_diff.rs` house pattern so adopters stop re-writing it. None of them
  changes what a row asserts — a row that still fails on a settled frame is a real
  failure and keeps failing, and every primitive has a negative row in
  `harness_contract.rs` (a fake that never settles must fail with both frames, not
  hang; a `stty -echo` pane must fail with a message naming what a lost scenario
  means). The settle heuristic's ceiling is documented and pinned: a pause longer
  than the poll interval looks settled, which is why rows anchor on a needle where
  they can. `an_oauth_flow_host_does_not_survive_into_tool_traffic` needed the
  fix in kind rather than a longer window: it sampled the flow-scoped host set
  every 5 ms from outside the flow, so a fast runner could finish the sign-in
  before the first tick and the row failed while the product was correct — it now
  reads the set from the browser opener, which runs synchronously inside
  `sign_in_flow`, so the observation cannot miss.
- **An extension can serve a web interface, and the host owns the socket**
  (gh #363). Every network capability so far pointed outward; there was no way
  for an extension to *answer* a browser. A manifest can now declare
  `[capabilities.serve-http]`, and the host mounts that extension at
  `http://127.0.0.1:<port>/<session-id>/<extension-name>/` on one loopback
  listener shared by every extension that asked. The extension never binds a
  socket or holds a port: the host reads the request and calls the extension's
  handler, with the path prefix already stripped so `index.html` and
  `./style.css` work wherever it is mounted. Loopback is not private, so the
  token is mandatory rather than optional — any local program can reach the
  port, any web page you have open can POST to it, and a hostile name can
  resolve to `127.0.0.1` through DNS rebinding. A request without the token, or
  with a `Host` header that is not the listener's own, is refused before the
  extension is called. The token is generated per session, compared in constant
  time, and never logged: it is shown once in the session's notice, which names
  the port actually bound. A port in use moves to the next one, and no free port
  means the feature reports unavailable while the session carries on — the agent
  never fails to start because a web view could not listen. WebSocket bridging,
  a dedicated-port mode, and a root dashboard are deliberately not in this
  release.
- **Sandboxed extensions can read the environment variables they declare**
  (gh #170). A WASM extension started with an empty environment, so an extension
  that fell back to an API-key variable worked when compiled in and broke as a
  `.wasm` — which is why one provider needed a hardcoded workaround. A manifest
  can now declare `[capabilities.env] keys = ["SOME_KEY"]`, and the host puts
  exactly those variables into the extension's sandbox when it starts it. Exact
  names only: a wildcard would make the install screen say nothing, and the names
  are the whole point of the approval. A declared variable the host does not have
  is absent rather than empty (an empty key that looks present is how a provider
  sends a blank credential), a declared name the user did not approve is absent
  too, and the ambient environment is never forwarded — not even `PATH`. The
  value is a secret the extension was given, so it never reaches the session log,
  an export, or `ext info`; only the names do.
- **A trusted folder no longer auto-approves inline program text** (gh #365). A
  trusted folder means trusting the code *in* it, so a command that runs a
  program string typed on the command line should ask — an earlier fix closed
  that for evaluator flags (`python3 -c`, `node -e`), but several tools carry the
  same code as a plain argument instead: `awk 'BEGIN{system("curl evil")}'`,
  `gawk`/`mawk`/`nawk`, `osascript -e`, `irb -e`. Those now ask. A program that
  arrives as a **file in the folder** stays on the trust fast path
  (`awk -f scripts/run.awk`), and ordinary `sed` edits are untouched — only
  `sed`'s `e` flag, which re-runs the substitution result as a command, asks,
  because refusing `sed` wholesale would train people to approve without
  reading. The same review turned up a sharper hole: a denied program behind an
  exec wrapper (`timeout 5 curl …`, `busybox wget …`) auto-approved because the
  wrapper was the program and the denied name was just an argument. That is
  closed too. Editor command flags (`vim -c`) and `tar --to-command` are named as
  still open rather than implied fixed.
- **Preset declarations are load-bearing** (gh #294). Two provider presets
  pointed at a local server and could not list their models: the manifest
  reached only the internet host, and a loopback address is refused outright
  rather than consented to later, so choosing `Ollama` or `LM Studio` failed
  before it started. The manifest now carries the narrowest possible local
  grant — `localhost` and `127.0.0.1`, no address block and no network
  wildcard. Separately, a preset's `supports_reasoning` declaration was parsed
  and ignored: the request consulted a single setting fixed at startup rather
  than the preset the chosen model routes to, so the one preset that declares
  reasoning support never used it and every other preset silently inherited
  whatever the environment said. The preset now decides, and a preset that
  declares nothing stays off. Discovery failing still lets a user sign in and
  fall back to the preset's curated model list, never a blocked login.
- **Approval prompts name a tool's own hints** (gh #303). A tool can declare
  what it does — `read-only`, `destructive`, `open-world` — and the permission
  modal now shows them where the decision is made: a `shell` call's prompt reads
  `echo hi (in /work) (destructive, open-world)`. The hints are display only. A
  hint never approves or denies anything, and denying a hinted action still
  denies it. Two gaps are named rather than papered over: an MCP tool call is not
  individually consented (so it has no modal for a hint to appear in), and a
  guest that sends its hints as camelCase JSON in `extras` gets them silently
  dropped, because that decode expects snake_case.
- **Approval prompts show what a tool says about itself** (gh #303). A tool can
  declare hints — `read-only`, `destructive`, `open-world` — and the permission
  prompt is meant to show them, so approving an MCP call tells you when the
  server has called its own tool destructive. But the prompt object that every
  capability engine holds implemented only the plain path, so the hints were
  silently dropped on the route almost everything takes and the user saw a bare
  action. It now forwards the hints as well; the decision is still the prompt's
  own, and a hint never approves anything (ADR-0047). A first-class tool path
  passes no hints and is unchanged. The one remaining prompt that still drops
  hints is the SDK's embedder callback, which is filed as a follow-up.
- **The conformance suite compiles its fixture once per row, not three times**
  (gh #449). Two `lca-ext-host` rows sat at ~1.6x the test harness's ceiling and
  timed out on a loaded runner: they were compiling the same WASM component two
  and three times inside a single row, for things the first compile already
  answered — the guest's schema, and a second scripted dialog verdict. Each
  compile costs about six seconds in a test build, and it cannot be shared
  across rows because the harness runs every test in its own process. Removing
  the redundant loads took the two rows from 73–77 s to 26–27 s and the whole
  file from 110 s to 66 s, so no timeout override was needed. A regression test
  now pins the compile count per row, because the durable invariant is how many
  times a row compiles the fixture, not how many seconds it takes.
- **`webfetch` works in the sandbox** (gh #302). The `tool` world imported
  every capability a tool might need except the network, so a tool-world guest
  could not fetch at all: `webfetch`'s tool existed only on the native path and
  returned a deferral message in WASM mode. The world now carries
  `import lca:host/net@0.6.0`, and both halves of `webfetch` run the *same*
  `fetch` → `convert` pipeline over a `NetView` their mode supplies, so the two
  produce identical bytes for the same input. Redirects are now followed (up to
  five hops, in the shared pipeline, because a redirect changes the host being
  asked and that is a consent question the capability deliberately leaves to its
  caller) instead of failing on the 302. The import is a breaking *contract*
  change but additive for old artifacts — the host satisfies an import whether
  the guest calls it or not — which is proven, not asserted: a tool component
  built before the import still loads and runs. Consent is unchanged:
  `[capabilities.net]` is still required, an ungranted host still asks on a
  tool-world call (the model or user chose the URL), every other world keeps the
  plain deny so a provider probe never pops a modal, and deny rules still win.
- **Costs and context limits now cover real models, from a real catalog**
  (gh #260). The hand-curated eight-model price table inside `lca-core` is
  gone; prices, context windows, and reasoning flags come from a
  build-time stripped [`models.dev`](https://models.dev) catalog embedded in
  the binary (about 134 KiB gzipped, from 231 providers and 9,075 models),
  plus an optional drop-in at `~/.lca/catalogs/models.json` that takes
  precedence over it. Lookups are keyed by provider *and* model, so the same
  model resold at two prices bills correctly on each. A model the catalog
  does not list still records tokens only - no invented rates. `lca-core`
  now names no vendor at all. Refreshing the catalog needs a rebuild: there
  is still no runtime network call (FR-CFG-3). The catalog also carries each
  model's context window; `lca_registry::models` and
  `lca_core::catalog_context_window` expose it, and the provider
  extension's own window resolution is the remaining place to wire it.

  **A CI-built binary carries the checked-in stub catalog, not models.dev**
  (13 models across 5 providers: `openai`, `anthropic`, `google`, `deepseek`,
  `groq`). A release build never fetches: NFR-17's reproducible double build
  runs two independent `CARGO_TARGET_DIR` builds and compares hashes, so a
  network fetch in either half could fail the gate on unrelated upstream
  variance. Build with `LCA_MODELS_CATALOG=<api.json>` to embed the full
  matrix; the two builds then read the same pinned file and stay
  byte-identical. Developer builds still fetch and cache as before, at
  `$CARGO_TARGET_DIR/models-catalog.json.gz`.
- **`lca mcp add|remove|list|login|logout` manages servers from a shell**
  (gh #418 Phase F, gh #73; pi's `pi mcp`). The verbs work outside a session,
  so a script or an agent can run them through `bash`, and they are not gated
  by `--no-mcp` or the enablement record — managing a disabled setup is the
  point. `add` writes the user file or, with `--local`, the project's, and
  refuses an option that belongs to the other transport instead of ignoring
  it. `list` connects each server for the length of the command, reports
  state, tools, exposure, resources, and the stderr tail of a failure, and
  exits 1 on a config error or a non-connected enabled server; `--json` is
  machine-readable. `remove` takes the scope it was asked for and, when the
  entry lives in the other one, names that scope and the flag that reaches it.
  `login` captures the 401 challenge, runs the OAuth flow (browser plus the
  loopback callback, with the pasted-redirect fallback for a headless run),
  and reports the tool count. `logout` deletes the stored credentials.
  `--oauth-client-secret` is never echoed and never written to `mcp.json`: it
  goes to the `mcp` credential namespace.
- **`--no-mcp` skips MCP for one run** (gh #418 Phase E, pi's flag of the
  same name). No server process starts and no remote endpoint is contacted.
  It is a per-run flag like `--no-tools`: it never writes the enablement
  record, which stays `/mcp disable`'s job.

### Fixed
- **`/permissions deny read <file>` now actually refuses the read** (gh #261).
  The command accepted it, confirmed `deny rule read muffin-recipe.txt`, and then
  did nothing — because rule matching compares the glob against the action's
  *absolute path*, so the `read ` verb became part of the pattern and matched
  nothing; and even without the verb a bare relative filename is an exact
  comparison against `/home/you/proj/muffin-recipe.txt`, so it missed too. The only
  thing that worked was `*muffin-recipe.txt`. A rule is now **compiled** when it is
  entered: `read <path>` and `write <path>` name the action kind and the path is
  turned into a pattern the matcher can compare (a relative path spans directories;
  an absolute path or an existing glob passes through untouched). The confirmation
  line states the pattern that will actually be matched, and what you typed when
  the two differ:
  `deny rule `*muffin-recipe.txt` for read actions (from `read muffin-recipe.txt`)`.
  Existing `*muffin-recipe.txt` rules keep working. Deny still beats allow at any
  scope, and a deny still never prompts. Two shapes are refused at entry instead of
  being stored as dead rules: a word one keystroke from a verb (`raed secrets.txt`
  — did you mean `read`?) and a path of only wildcards (`read *`). A deny rule that
  silently does nothing is worse than no rule at all, because the user stops
  watching.
- **The Antigravity extension no longer advertises image generation it does not
  do** (gh #279). Its manifest description — the text you read before installing —
  and its catalog row both said *"Google Antigravity models, including image
  generation, via subscription login"*, while `is_selectable` refused every model
  whose id contains `image`. It now reads "Google Antigravity's Gemini and Claude
  models, via subscription login", which is what the code offers. The description
  was the side that moved, deliberately: the provider stream has no event an image
  *result* could arrive in (`StreamEvent` carries text, reasoning, tool calls,
  usage, errors and vendor events — and the extension's only vendor event is the
  thinking signature), so enabling image models would be a feature with no path,
  not a fix. `ContentBlock::Image` is the request side — an attached image becomes
  Gemini's `inlineData` part — which is image input, not generation. The provider
  reference (`pi-antigravity`) *does* generate images, through a separate direct
  image endpoint and a `generate_image` tool, neither of which LCA has; that is why
  the claim looked plausible and why the honest fix is to stop making it. The
  provider doc's `vendor-event` section, which claimed the channel exists to carry
  image results, now says what is actually emitted, and a drift guard keeps the
  manifest, the catalog row, the doc and the filter from disagreeing again.
- **An MCP server's own error reaches you, on both transports** (gh #336). Per
  JSON-RPC 2.0 a server that cannot parse the request id — or hits any
  transport-level failure — answers with `"id": null`. The SSE selector was taught
  that in an earlier cycle, but the **stdio** selector still classified such an
  error as "an id that is not ours" and reported `answered without an id … the
  stream is desynchronized`: LCA's guess, and a false one, with the server's
  `"error"` payload dropped. That is where the issue was originally filed from.
  Both selectors now classify a null-id error as a protocol-level answer and
  surface the server's `message` and `code` verbatim, inside LCA's own context
  rather than rewritten. Notifications (no `id` key) are still skipped, a foreign
  id is still a desync, an error for *our* id is still an attributed answer, and a
  reply we can attribute still wins over an unattributable error in the same
  envelope. Side by side on the same mock server:

  ```
  before: MCP server answered without an id (wanted id 2); the stream is desynchronized
  after:  MCP server errored: {"code":-32700,"message":"Parse error: could not read request id"}
  ```
- **A synchronous extension call no longer spawns a thread** (gh #286).
  `WasmExtension::blocking` ran `std::thread::spawn(..).join()` on every call, so
  each `render`/`on_event` — one per TUI frame — and each stream delta created and
  reaped an OS thread: syscalls, stack allocation, and latency jitter on the
  hottest path in the interface. Each extension now owns one long-lived driver
  thread, created on first use and reused for every call. Measured on this box,
  medians of 500-call runs (three runs each way): **~689 µs/call before, ~273
  µs/call after** — about 415 µs off every frame that touches an extension's UI.
  The thread still exists because the guest's WASI is linked synchronously, so a
  guest doing file I/O must not run on a runtime worker; it is now created once per
  extension instead of once per call. Cancellation is unchanged and pinned: an
  epoch bump still traps a guest mid-call wherever it runs, a trap still evicts the
  instance cache and disables the extension, and dropping an extension shuts its
  driver down. The driver holds its extension *weakly* — a strong handle closes a
  reference cycle that would keep the extension and its instance cache alive for
  the process lifetime, which the shutdown row caught in the first version of this
  change.
- **A typo in a manifest's `worlds` list is refused, not installed as an
  extension that does nothing** (gh #288). `worlds = ["contxt-transform"]` — one
  transposed letter — used to install successfully and load as an extension whose
  world set matched nothing: the install reported success, no error appeared
  anywhere, and the extension silently did nothing. Every entry is now validated
  against the world vocabulary at parse, and the refusal names what you wrote,
  lists the vocabulary, and suggests the nearest match (`did you mean
  'context-transform'?`). Two adjacent holes closed with it: a non-string entry
  (`worlds = ["tool", 7]`) used to be dropped on the floor by a `filter_map`,
  leaving a silently shorter list, and a duplicate (`["tool", "tool"]`) was
  accepted even though the schema already said `uniqueItems`; both are errors now.
  The `hooks-*` names are validated as a **fixed set**, so `hooks-tpyo` is refused
  exactly like any other typo rather than passing a prefix check. The vocabulary
  now lives in one table (`lca_ext_abi::WORLDS`) that both the manifest validator
  and the loader's dispatcher read — it had been three copies (a `match` in the
  dispatcher, a `==` chain in the loader, and the schema's `enum`), which is how
  the bug arose — and tests keep the schema, the authoring guide, and every
  shipped `extensions/*/extension.toml` in step with it. The loader's own world
  checks (`.any(|world| world == "ui")`, nine of them) now ask in terms of the
  `World` enum too, because a typo there was invisible to every validation test:
  the name the author wrote was accepted, and the loader silently never matched
  it.
- **A theme's missing roles fall back through a declared parent instead of
  guessing** (gh #387 gap 5). A theme file under `~/.lca/themes/` may name only
  some roles, and everything it omits used to resolve to `Text` (or to whatever
  the base palette happened to hold) — so a theme written a year ago could start
  rendering differently the moment a role was added, and a *background* role
  could be handed a foreground colour. Every role now declares a colour slot and
  a parent in the same slot, resolved once at load: `mdLink`, `mdCode`,
  `mdListBullet`, `syntaxKeyword`, `customMessageLabel` and `bashMode` follow
  `accent`; `syntaxType` follows `mdCode` and then `accent`; the thinking levels
  form a linear ramp where naming one level sets every hotter level. Setting a
  parent carries its children, naming a child still wins, and a role with no
  named ancestor keeps the base palette's own value — so a two-line theme does
  not flatten the whole palette onto one colour. The table is validated at load:
  every role has exactly one entry, no parent crosses the foreground/background
  boundary, and a declared cycle fails with the loop named. An unknown role name
  is still skipped, so a theme written against a newer LCA keeps loading.
  `docs/configuration.md` now documents the rule, role by role, with a test that
  fails if the doc and the table ever disagree.
- **`/reload` re-reads the active theme file.** It refreshed the `/theme`
  picker's list of names and then reported "reloaded: … themes …", but the
  palette on screen was never re-applied — so editing a theme and reloading
  changed nothing until a restart. Found while verifying the fallback chains
  above: set one parent role, edit it, `/reload`, and the frame was unchanged
  while the notice claimed otherwise.
- **The thinking-level border is a ramp you can read** (gh #387 gap 4). The
  seven levels are the one ambient signal of how much reasoning budget a turn is
  spending, and they were hand-picked hex with no ordering relationship at all:
  measured in OKLCH, the dark palette's last four steps moved 0.01–0.02 in
  lightness and two of them had *identical* hue (318.33° → 318.24°), so `high`
  and `xhigh` were the same colour to the eye. The light palette was worse — its
  lightness went up and down across the seven (−0.19, +0.01, −0.01, −0.03,
  −0.09, +0.04) and its hue reversed twice. Both now run one ladder: hue rotating
  slate → periwinkle → violet → magenta → flame, chroma rising at every step, and
  lightness moving *away* from the page by even amounts (a 1.2–1.3× spread
  between the largest and smallest step, where the dark palette used to be
  11.7×). On a light terminal the ramp steps *down*, because a ramp that got
  lighter there would fade into the background. With `ui.theme = "system"` the
  ladder is anchored to your terminal's reported background instead of a nominal
  one, so a Solarized session gets a Solarized ramp. `max` ends at flame rather
  than pi's red, which is the error colour's own hue — at `max` thinking the
  border and an error would otherwise look identical. In the plain theme, where
  every role is identity, the footer still names the level in text (NFR-28).
- **`system` message and tool panels are visible, and `COLORFGBG` 7/8 is no
  longer inverted** (gh #387 gaps 1 and 2). With `ui.theme = "system"` the
  user-message band and the custom band were set *equal to* the terminal's
  reported background, and the tool panels and selection wash inherited the
  same flatness from the scheme base — so a user message, a tool card, and the
  transcript all looked alike. Each panel is now derived from the probed
  background: the family's hue, the background's lightness stepped one
  perceptual notch away (lighter on dark, darker on light), and a floor of the
  background's own chroma so Solarized surfaces stay Solarized and Tokyo Night
  stays Tokyo Night. The step is identical for every family, so text contrast is
  equal across cards and the state reads by hue alone; the pending card is
  deliberately neutral, so it cannot collide with your own band. A terminal that
  reports no background keeps every panel transparent, so a translucent terminal
  is not painted over. Separately, `COLORFGBG` classified ANSI 7 (white) as dark
  and ANSI 8 (bright black — Solarized Dark's own background) as light, exactly
  backwards; indices now classify the way Vim and pi do, both `fg;bg` and
  `fg;xpm;bg` shapes are read, and an unusable value falls back rather than
  guessing. Also fixed: the default `ui.theme = "auto"` never rebuilt the theme
  from a reported background (the check tested the literal string `system`),
  so the panel derivation would not have run on an unconfigured install.
- **Typing works while startup finishes, and `Ctrl+C` is honoured from the
  first frame** (gh #400, type-while-loading slice). The terminal entered raw
  mode and started reading only *after* the extension registry, config, and
  session had loaded, so the startup window was deaf: text typed in it was held
  by the tty and appeared late in one lump after the header, and `Ctrl+C` did
  nothing at all — a user trying to leave a slow start had to wait for it, then
  press again (gh #225's armed exit meant the first press was simply spent). The
  terminal is now claimed before the load, buffering into an ordered list the
  interactive loop adopts and drains — same reader, same receiver, no handover
  gap — so keystrokes typed during startup land in the editor in the order they
  were typed, a bracketed paste stays one atomic segment, and a quit key acts.
  `Ctrl+C` is recognised in all three spellings the terminal uses (the raw byte,
  Kitty CSI-u, and xterm modifyOtherKeys); the two protocol forms are what a
  real terminal actually sends once the keyboard protocol is negotiated.
- **A project override can be seen and cleared** (gh #397, tri-state slice). A
  project override has three states, not two — the project file says nothing
  (inherit the user file or the default), says on, or says off — and the
  interface could only express the two set ones. `/settings` now shows each
  boolean key's project state as a word (`project inherit` / `on` / `off`) and
  cycles it with `Tab`. Returning to `inherit` **removes** the key instead of
  writing `false`, because a `false` written to stop overriding would keep
  beating the user file it was supposed to yield to; clearing the last key
  removes the emptied file so the round trip is byte-identical. `Enter` still
  writes the user file, `Tab` writes this project's: two different files, two
  different consequences, and the row names the source that actually won. An
  override in an untrusted project stays inert and says so (FR-PERM-9). Also
  fixed while there: writing `ui.quiet_startup` through `/settings` used to
  persist the TOML **string** `"true"`, which the loader then refused on
  `/reload` ("expected true, false, or \"header\", got a string") — a write the
  user could not read back.
- **The footer tells the truth about git, and stops reading the disk on every
  frame** (gh #389, closing #330). The branch was read from `<cwd>/.git/HEAD`
  and nothing else, so it vanished when you ran the agent from a subdirectory,
  from a linked worktree (where `.git` is a *file*), or on a detached HEAD —
  and a `reftable` repository (Git ≥2.45) printed the placeholder `.invalid` as
  if it were a branch name. Discovery now walks up to the repository root,
  follows the `gitdir:` indirection, shows a detached HEAD as its short object
  id, and resolves a `reftable` HEAD through `git --no-optional-locks` (so it
  never takes `.git/index.lock` out from under your own commit). The walk is
  bounded, and it is cached on a 2-second cadence driven by the existing tick
  rather than done per repaint: the footer repaints on every streaming delta, so
  a read there scaled with filesystem latency on the UI thread — a thousand
  frames now perform zero reads. An expired cache keeps the last known branch
  rather than blanking, because a footer flickering between `main` and nothing
  is worse than one that is a second stale. Also: a fresh session no longer
  prints `↑0 ↓0 R0 W0 0.0%` (each counter appears only when it carries
  information), `~` shortening respects a path boundary so `$HOME2/x` is not
  rendered as yours (`USERPROFILE` is now consulted on native Windows shells),
  and a long row degrades by dropping the provider prefix, then the context
  cell, before it truncates.
- **`$EDITOR` no longer adds a blank line to the prompt** (gh #388 gap 4).
  Every POSIX editor — `vim`, `nvim`, `nano` — terminates the file it saves
  with a newline, and the buffer was read back verbatim, so editing a one-line
  prompt came back as two lines with an empty second one. Fix the typo, get a
  message with a blank line you never typed. Exactly **one** terminator is now
  stripped, never `trim_end()`: a prompt you deliberately ended with a blank
  line keeps it. CRLF and a lone CR count as terminators (so a Windows editor
  leaves no stray carriage return in the composer), a leading UTF-8 BOM is
  stripped, an empty buffer clears the prompt rather than reusing the old text,
  and undecodable bytes refuse the edit instead of silently mangling it.
- **The dock keeps its height, and compaction has a status row of its own**
  (gh #388 gap 2). The footer's status line was pushed only once there was
  something to say, so the first turn added a row and every later frame kept
  it: the dock grew by a row the moment a turn started, moving the whole
  transcript above it. The row is now reserved whether or not it has text, so
  a turn's start and end leave the dock's row count unchanged. Separately,
  compaction used to be handled *outside* the separator - a compacting
  interface showed `Working` with no way to tell why, or nothing at all. It is
  now a first-class `SeparatorState::Compacting` carrying its reason
  (`manual`, `threshold`, `overflow`), rendered in the same row a turn uses and
  agreeing with the tab badge and taskbar spinner about what counts as work.
- **A collapsed tool card shows the tail, ticks while it runs, and tells
  `!` from `!!`** (gh #391). Three gaps in the preview pass, and the first
  was the serious one: the preview sliced five *logical* lines and then
  wrapped them, so one 4,000-character minified line became ~50 visual rows
  inside a card that called itself collapsed - and the five kept lines were
  the `Compiling …` boilerplate while the pass/fail summary sat at the tail.
  It now wraps first, keeps the last five *visual* rows for shells with the
  `… (N earlier lines, ctrl+o to expand)` marker above them, and keeps the
  head for tools that read top-down (`ls`, `grep`), all cached per width. A
  running card shows `Elapsed 4.2s` ticking on pi's three duration tiers and
  freezes to the recorded `Took` when it settles - a replayed session shows
  its historical time and never animates - driven by the interface's existing
  tick rather than a second clock. `!!` now draws quiet where `!` keeps the
  accent color, and both wear the marker the user typed, so the context
  boundary survives a terminal with no color (NFR-28). The tool layer's
  in-band `[full output: …]` footer is stripped before the preview, so it no
  longer spends two of the five tail rows.
- **Replacing the transcript drops the old selection** (gh #403 gap 1). A
  selection lives in *content* coordinates, so after a `/resume` switch, a
  `/tree` branch jump, a fork, a clone, or an import the old rectangles
  pointed at rows holding different text - highlights over content nobody
  selected, and a copy that returned bytes from a transcript that was gone.
  The reset now happens at the one seam every replacement path shares (a
  transcript *generation* that only a clear moves) rather than at five call
  sites, so a sixth path cannot forget it, and it drops the multi-click run
  as well as the rectangles (pi's `resetTextSelection` shape) so the next
  click cannot continue an old word-or-line run. A resize, a streaming
  delta, and a marker repaint leave the generation alone and keep the
  selection, which is pinned in the other direction.
- **`/bug`, `/export`, and `/import` run** (gh #443). Each had a working
  implementation and a `/help` line, but the interface router admits a name
  only when it is in `slash_commands`, and none of the three were — so all
  three answered `unknown command` and their code was unreachable. The host's
  own command list is now one constant (`HANDLED_NAMES`) that `slash_commands`
  derives from, and two guards keep the triangle honest: every registered name
  has a dispatch arm (a listed-but-unhandled name used to fall through to the
  extension dispatcher and silently do nothing), and every routed command has
  a `/help` line (which is how `/label`, `/labels`, `/jump`, `/rename`, and
  `/extensions` were reachable but invisible).
- **`tool.edit_requires_read` now governs extension editors too** (gh #436).
  The setting promised "refuse an edit to a file this session has not read",
  but `hash_edit` wrote through the capability engine's `fs_write`, which never
  consulted the read tracker the built-in `edit` uses — so the staleness guard
  had a hole exactly one extension wide. The issue left the fix as an owner
  call: make the tracker govern, or document an anchor-guarded exception. The
  exception is false, and three measured paths say so: a 3-character line hash
  carries no file identity (an anchor fetched from one file edits another), a
  change-then-revert revives a stale anchor, and only duplicate lines are
  caught. An anchor proves "this text was in *some* read"; the flag promises
  "this file was read recently". So `lca-tools` now keeps one process-scoped
  read record that the built-in `read` writes and both editors consult, with a
  single shared refusal string — two editors answering differently for one rule
  would have been the same defect in a new place. Creating a new file is never
  gated (nothing to be stale against), and with the flag off, blind edits still
  land, as pi does.
- **The user guide now names every command users can run, and stops
  promising a reload it does not do** (gh #419). `docs/usage.md` read like
  an exhaustive command list while omitting nine that ship; it is now a
  complete table, with `/help`'s own one-line text. Three of those commands
  - `/bug`, `/export`, `/import` - turned out to answer `unknown command`
  (fixed above in gh #443). Separately, three docs promised `/reload`
  applies keybinding edits without a restart, which
  is true everywhere except the prompt editor, which keeps its startup key
  map - measured in a pane, tracked as gh #332, and now stated as what
  works, what does not, and what to do about it.
- **A headless client can see that its message was accepted, not just
  drained** (gh #426). `docs/headless.md` documented `queue-queued` with a
  `mode` field and nothing emitted it, so a `--mode json`/`--mode rpc` script
  saw the `queue-flushed` drain without ever seeing the accept - and "queued
  and later injected" was indistinguishable from "never queued at all", which
  is the one thing the steering queue exists to distinguish. The accept now
  goes out from the same sink as the drain, the moment the queue takes the
  message, so it always precedes the flush; `mode` is the ADR-0038 submit mode
  actually used (`steer` / `follow-up`), never a default. A cancelled queue
  announces its accept and no drain, as it should. The #264 drift guard's
  `PRODUCED_ELSEWHERE` exception is gone and the guard is green - the proof
  the doc and the code agree again. Guard:
  `tests/regressions/426-queue-queued-is-emitted.rs`.
- **The slowest end-to-end install row now has a budget it can actually meet**
  (gh #429). `e2e_install`'s Phase 5 exit test — the one that installs from OCI
  *and* an HTTPS archive, grants, runs a turn, updates and removes — took 66–71 s
  under 2× load against a 120 s cap: passing, but with only ~1.7× margin, so a
  slower runner would eventually fail it for a reason unrelated to the product.
  The doubled-fixture theory turned out to be wrong, and measurement said so:
  fixture setup is 0.12 s of a 42 s idle run, while 94% is two turns at ~12 s
  each, because every turn-shaped spawn loads an installed WASM component and
  `wasmtime::component::Component::new` costs 21.8 s in a test build against
  1.7 s in release. Sharing setup could not have helped and splitting would have
  spread a fixed per-process cost across rows without lowering the total, so the
  row gets a nextest override scoped to its exact name, budgeted at twice the
  worst of seven loaded measurements (50–104 s) with the stage breakdown and the
  arithmetic in the config, and verified in both directions: a sibling row is
  still terminated at the default 120 s while this one survives past it.
- **The taskbar spinner survives a long turn, and tab status covers
  compaction, shell runs, and install consent** (gh #369). `OSC 9;4`
  progress was written once at turn start, but terminal emulators put a
  ~10-15 s watchdog on the sequence so a crashed CLI cannot spin a taskbar
  forever - so any turn longer than that (most of them) silently lost its
  indicator mid-flight. It is now re-emitted once a second from the
  interface's own 50 ms tick, not a new thread, and stops dead when the turn
  does. `OSC 7501` tab status covered turns only: a background `/compact`
  and an inline `!`/`!!` run now badge `working` (one `WorkKind` feeds both
  the badge and the spinner, so they cannot disagree), and the `/extensions`
  install-consent prompt - the one modal missing from the blocked list -
  badges `blocked`. States still emit on transition only, and a
  suspend/resume (`$EDITOR`) now restores the badge instead of leaving the
  tab blank. Guard:
  `tests/regressions/369-osc-keepalive-and-status-coverage.rs`.
- **The conformance fixture can no longer go stale against the ABI** (gh #427).
  The committed `extensions/conformance/fixtures/component.wasm` failed to
  link with `no exported instance named 'lca:ext/hook-message-end@0.6.0'` —
  a link error that looks like a host bug to anyone installing it. The
  package's fixtures had no refresh step anywhere (the publish workflow
  refreshes five other extensions' fixtures, never this one), so when the hook
  worlds landed in `wit/` the source moved on and the binaries did not. The
  stale file was an **unconsumed twin**: byte-identical to `tool-world.wasm`
  when it was added by the 0.5.0 release commit, referenced by nothing in
  history — so it was removed rather than rebuilt, which would have doubled the
  very rebuild-both-or-drift trap that caused this. The fixture tests actually
  load stays, byte-identical to a fresh build, and three guards keep it that
  way: `scripts/wasm-check.sh` byte-compares it against its own build on every
  push, a load guard in `lca-ext-host` links every fixture against every world
  the manifest declares, and a regression row refuses to let any committed
  fixture sit without a test file that names it.
- **`lca ext install` pulls from a real registry again** (gh #308). Three
  defects sat in one fetch path. The post-401 retry checked `is_success()` and
  nothing else, so the *normal* ghcr blob shape - 401, then a token, then a 307
  to the CDN - died with `HTTP 307 Temporary Redirect after authorization`;
  the redirect loop existed, on the one attempt that did not need it. The token
  parse read only `token` and ignored the spec's `access_token`, so a registry
  answering the other field left the retry anonymous. And the client was a bare
  connector: no `HTTP_PROXY`/`HTTPS_PROXY` (the `net` client has had them since
  gh #145), no byte cap, no timeout - so a hostile or stalled registry was
  accumulated in full *before* the digest ran, which inverts
  digest-before-install. All three now share one redirect loop with one
  five-hop budget and the same cross-origin rule (a redirect to the CDN drops
  the bearer; it never needed it and forwarding it is a leak), accept either
  token field and refuse by name - quoting the endpoint - when neither is
  present, and bound every fetch in time and bytes *while reading*. The proxy
  rules are the `net` client's, spelled out in `docs/environment-variables.md`
  so the two cannot drift.
- **An enterprise Copilot account gets its hosts validated and a refusal
  that says what to do** (gh #282). `login_options` asks for an
  `enterprise-domain`, and the flow then requests
  `https://<enterprise>/login/device/code` — but the ad-hoc `net` grant
  offer only ever saw the manifest's declared host (`github.com`), because
  the enterprise domain is field data. The offer runs *after* a successful
  `login-submit`, so the submit's own first request was denied with
  `matches no granted pattern` and no way forward. The field is now
  validated as a hostname (a scheme, path, port, user info, wildcard or
  whitespace is refused at the field with the command that fixes it), the
  three hosts an enterprise account needs are derived in one place and
  shared with the inference URL so the granted host is the requested host,
  and an ungranted enterprise login now names every derived host plus
  `--allow-host` for each and points at the interactive path. `github.com`
  derives no extra hosts and its failures are never rewritten. Guard:
  `tests/regressions/282-enterprise-hosts-offered.rs`.
- **Copilot's discovered models keep their real context windows** (gh #283).
  Live discovery mapped every model from GitHub's `/models` to
  `context_window: 0`, discarding the curated table's windows. Zero is not
  "small" here - the footer reads it as `ctx ?` and the FR-SESS-4 threshold
  falls back to `docs/compaction.md`'s 128k estimate, which is right only for
  a model whose real window happens to be 128k. Copilot's curated figures are
  200k, so every Claude and o-series session was using a threshold far below
  the real one: it compacted early, and the footer told the user nothing. Rows now resolve through the
  precedence the `openai-compatible` provider already uses, narrowed to what
  this gateway publishes: the curated table wins for an id it knows (its
  figures are vendor-documented; the account's tier can be narrower), the
  endpoint's `capabilities.limits.max_context_window_tokens` sizes an
  uncatalogued id (falling back to `max_prompt_tokens`, as opencode's decoder
  does), and nothing is invented when neither answers - the footer keeps
  `ctx ?`. The same mapping stops echoing the id as the name and forwards the
  endpoint's `max_output_tokens` and capability hints (`image.vision`,
  `thinking_levels`) through the extras channel; that last part needed the
  WASM guest to stop dropping extras, since the provider ships as a
  component. Guard: `tests/regressions/283-discovered-models-keep-their-windows.rs`.
- **`/login` now lists every provider that can sign in** (gh #297). Six
  subscription providers - `antigravity`, `codex`, `grok`, `kimi-coding`,
  `meta` and `openrouter` - sign in through their own OAuth or device-code
  flow and declared no picker presets, and the picker only listed providers
  with presets. A user with a ChatGPT Plus, SuperGrok, Meta, Kimi or Google
  subscription saw a picker that did not mention any of them, and could only
  sign in by already knowing the undocumented `/login <name>` spelling. Those
  providers now declare `[login] identity = true` in their manifest, and a
  bare `/login` shows `Sign in with <provider>` for each; choosing the row
  runs that provider's own flow.

  Declared rather than discovered, deliberately: the only way to *ask* a
  provider whether it can sign in is to call its `identity_login`, which
  starts the flow - opening a browser or blocking on a device poll for every
  installed subscription provider the moment `/login` opens. The declarative
  marker also means the pick path had to learn a second route: these
  providers do not implement `login-submit`, whose trait default answers
  "ok" having stored nothing, so a synthesized row routed down the submit
  path would have reported a successful sign-in and left no credential.
  Guard: `tests/regressions/297-login-lists-every-provider.rs` and
  `tests/regressions/297-providers-declare-their-sign-in.rs`.
- **The antigravity request no longer sends your system prompt twice** (gh
  #280). `build_request` pulled every system message into `systemInstruction`
  (correct) and then walked the same messages into `contents` again, because
  the role match ended in a `_ => "user"` wildcard that swallowed
  `MessageRole::System`. So every request carried the whole preamble twice:
  once as instructions, once as a prior user turn. That billed the largest
  fixed block in the request on every turn, framed directives as if you had
  said them - a fidelity problem and a prompt-injection surface - and put the
  preamble in the leading `contents` slot, so the cacheable prefix shifted
  whenever anything upstream touched the instructions. A system message is now
  never a turn, and the arms name every role so the next variant cannot fall
  into `"user"` the same way. Extra system messages (a transform could add one)
  merge into `systemInstruction.parts`, the array shape pi-antigravity uses,
  rather than being dropped; uncollected blocks are appended, already-collected
  ones are not repeated, and `parts` is never empty. Tool turns, the
  `thoughtSignature` sibling placement, and the model-fallback path are
  untouched.
- **A second compaction no longer forgets the first one's summary when the
  model is unavailable** (gh #281). The mechanical fallback - what runs when
  the summarization call fails or has no budget - selected the host's carried
  `previous-summary` marker records correctly but then decoded every one of
  them with a hardcoded `compaction` kind. That branch reads a `summary` key;
  a marker record keeps its text under `data.text`, so each carried summary
  decoded to the empty string and vanished. The practical effect: on a long
  session that had to compact more than once without a working model, the
  `Earlier summary:` line was blank or absent and every distilled fact from
  the first pass was gone. Each excerpt is now decoded by its own kind, an
  empty summary leaves no stray header, and the 600-character cap and
  ordering are unchanged. Guard:
  `tests/regressions/281-mechanical-compaction-carries-summaries.rs`, plus six
  rows in `extensions/compaction-default/tests/compaction.rs`.
- **`hash_edit` can finally be called** (gh #285). The tool resolves line
  ranges by 3-character content hashes, and the only function that produced
  those hashes had **no caller outside the crate's own tests**: the built-in
  `read` answers with decimal line numbers, and the extension exported neither
  a `tool_result` hook nor a `hash_read` tool. So the model could never see an
  anchor, and a tool whose arguments cannot be constructed is a tool that does
  nothing — it shipped, was listed, and was unusable. It now also exports the
  `hooks-tool-result` world and annotates a successful text `read` with a hashed
  gutter in front of each line, so the anchors arrive with the read the model
  was already going to perform: one read, one tool, no second read path, no new
  capability. Only `read` results are touched, only when the line-number format
  parses, and anything unparseable passes through byte-identical — an
  annotation hook must never make a result worse. Its writes still bypass
  `ToolExecutor`'s read tracker, because the tracker is private to `crates/lca-tools`
  and unreachable from an extension (a follow-up needs that API change); what
  holds instead is the property the tracker exists for — an anchor only exists
  because a read produced it, and a changed file stops matching it, so no stale
  write lands.
- **The queued-message band behaves like pi's, and a compaction no longer
  refuses your typing** (gh #401). Three queue-UX gaps around the shipped
  abort rescue: (1) stopping a turn now says
  `Restored N queued messages to editor` and merges the queue *ahead of*
  whatever you typed while waiting, `\n\n`-separated with the caret at the
  end - before, the restore was silent and a live draft overwrote it; (2) a
  prompt typed during `/compact` or an auto-compaction is queued for after
  it (`Queued message for after compaction`) instead of being turned away,
  then flushes in order - leading commands, then the first prompt starting
  the turn, the rest behind it - and rolls back whole if no turn can start;
  (3) the band labels messages `Steering:` / `Follow-up:` (pi's words) and
  hints `↳ alt+up to edit all queued messages` from the live binding
  registry, with `Alt+Up` (pi's key) doing the restore on demand. `Alt+Up`
  therefore leaves the prompt-jump binding, which keeps `Shift+Up` and
  `Ctrl+Up`; `Alt+E` survives as the second dequeue spelling. The abort
  rescue itself is unchanged and now pinned by a regression row.
- **A rate-limited provider's `retry-after` is honoured again** (gh #278).
  Every provider reads the HTTP `retry-after` header and appends it to the
  error text, where the host's retry engine parses it (gh #52) — but the
  anthropic extension closed the response handle *before* reading the headers.
  `net_close_response` drops the response from the host's handle table, so the
  read answered `NotFound`, the `&&` chain short-circuited, and the server's
  backoff intent was silently discarded: the client retried on its own
  exponential schedule instead of the delay the server asked for. Two sibling
  extensions had the same class of defect reached differently —
  `openrouter` and `openai-compatible` (the default provider) never read the
  header at all, so a 429's delay was lost either way. All three now read it
  while the handle is live and forward the value verbatim; the host already
  accepts both seconds and an IMF-fixdate and clamps with
  `provider.retry_max_delay_ms`, so a hostile `retry-after: 86400` cannot
  wedge a session. The token is attached only for throttling statuses
  (429/500/502/503/529) — a 401's answer is to re-authenticate, not to wait.
  The shipped test for this passed over the defect because its capability
  double returned headers unconditionally regardless of the close; the guards
  now model the real lifecycle, which is what makes the ordering observable.
- **One malformed entry no longer discards the whole OpenRouter catalog**
  (gh #295). `discover_models` read the gateway's `/models` array with
  `entry.get("id")?.as_str()?` inside its loop — `?` returns from the
  *function*, not the iteration, so a single non-conforming row
  (`{"id": null}`, a deprecation or rate-limit notice shaped
  `{"message": …}`, a numeric id) discarded every row and fell back to the
  preset's three curated models. A 300-model catalog became three, silently.
  A row now needs a non-empty string `id` to be listed and is otherwise
  skipped in place, so the surviving rows keep their gateway order; the drop
  is counted and reported through each mode's diagnostic channel (the guest's
  `log` import, the native twin's warning stream) rather than swallowed, so a
  partial catalog never looks complete. A payload with nothing usable still
  takes the curated fallback — resilience is not an empty picker.
- **`stream-guard` reports each match once, and its WASM mode honours project
  trust** (gh #300). Two defects with one shape — the extension's delivery
  modes disagreed with each other and with common sense.
  - **Re-fire storm.** `Scanner::feed` appended deltas to a rolling 4 KiB
    window and emitted a match when a rule hit, but never advanced past the
    matched span, so the same text re-matched on every subsequent delta for
    the rest of the window — one `.unwrap()` in a long answer produced a hit
    per remaining token. The window was also never reset across turns, so
    turn N's match re-fired on turn N+1's first token. Each rule now keeps a
    cursor to the end of what it has already reported, so only new text is
    examined: a rule that legitimately matches again still fires, and a
    pattern split across deltas still matches (the window is unchanged). The
    handle now also declares the `Hooks` world, which is where the registry
    dispatches `pre-turn`; declaring only `HooksStream` is why its
    `on_pre_turn` was dead code and the window lived across turns.
  - **Trust bypass.** The WASM guest read `.lca/extensions/stream-guard/rules.toml`
    unconditionally while the native handle refuses workspace rules unless the
    project is trusted, so an untrusted repository could inject stream rules
    simply by being opened — a consent-mandating surface honouring consent in
    one delivery mode and not the other. The guest now loads through the one
    trust-gated `load_rules`, which means an installed WASM `stream-guard`
    scans the user's global rules and not the project's. A WASM guest has no
    host import that answers "is this project trusted"; the documented route
    (`docs/extension-authoring.md`) is to refuse the trust-dependent half with
    a reason, which is what `project-memory` already does.
- **`/tree` no longer walks a long conversation off the screen** (gh #394).
  The navigator indented one gutter per *visible ancestor*, so a linear
  40-turn session - no forks at all - indented its eightieth row by 160
  columns and pushed the whole transcript past the right edge. Indentation
  now grows only where the history really forks, plus one grouping level
  beneath a fresh fork, so a linear chain stays at column 0 however long it
  is; the recompute runs after every filter, so hiding a dead-end branch
  releases its indent too. Three further rules landed with it, all pi's
  `tree-selector.ts`: the branch holding the live leaf sorts first at every
  level (stable, so two frames of one session are byte-identical), a filter
  change walks `parent[]` to the nearest visible ancestor instead of
  clamping the numeric index, and folded branches paint `⊟`/`⊞` in the
  connector cell (zero extra columns) with `Alt+←`/`Alt+→` folding a branch
  or jumping to the next one. A deeply indented selection pans horizontally
  with the cursor gutter pinned, and no row is ever painted wider than the
  box. Tool-result rows now render their call's arguments
  (`[read: src/main.rs:1-50]`), and a textless tool-dispatch assistant row
  hides unless it is the live leaf. The navigator's layout math moved to
  `crates/lca-ui/src/chat_tree.rs` (the file ceiling), and the guard is
  `tests/regressions/394-tree-layout-math.rs` plus two real-terminal rows in
  `crates/lca-cli/tests/e2e_tree_layout.rs`.
- **`--mode rpc` sessions wire the nested completion backend** (gh #269).
  `headless` and the interactive runner both build one and install it on the
  agent; the RPC entry point did not, so LLM compaction in a daemon session
  silently fell back to the mechanical excerpt summary and an extension asking
  for the `completion` capability had nothing to call.
- **The system prompt advertises only the tools the run offers** (gh #271). It
  hardcoded every built-in, so `lca -p "…" --no-tools` told the model about
  tools the same request did not declare — an invitation to wasted turns and
  "tool not found" errors. The prompt's tool list now comes from the run's
  effective set, the same resolution the request's declaration reads, so the
  two cannot disagree.
- **The `question` tool declares one schema in both delivery modes** (gh #298).
  The WASM guest hand-wrote a shortened copy of the tool's parameter schema
  with the `items` object missing, so a WASM-mode model was told `questions`
  was an untyped array. It answered with plain strings
  (`{"questions": ["Which port?"]}`) — the shape the schema invited — and the
  tool rejected them, because its parser requires `header` and `question` per
  entry. The failure got sharper when gh #306's validator began enforcing an
  array's `items`: the same answer now fails the host's own check before the
  guest's parser ever runs. Both modes now build the schema with one
  constructor (`question_tool_spec`, at the crate root so it compiles for
  `wasm32` as well), and a test asserts the two outputs are byte-identical
  rather than spot-checking a field. `hash-edit` and `webfetch` already
  worked this way; `question` was the outlier. `docs/extension-authoring.md`
  now states the rule for dual-mode extensions.
- **The environment variables each provider's preset declares now
  authenticate it** (gh #292). `provider-presets.toml` documents a canonical
  variable per service — `DEEPSEEK_API_KEY`, `GROQ_API_KEY`,
  `MISTRAL_API_KEY`, `CEREBRAS_API_KEY`, and so on for 21 presets — and the
  preset parser read the list, but key resolution consulted only
  `OPENAI_API_KEY` and `OPENCODE_API_KEY`. A developer who exported the
  variable their provider's own page names was told to authenticate, because
  the request went out with no `authorization` header at all; the only ways
  forward were aliasing the variable or typing the secret into `/login`. The
  declared names were decorative data making a promise nothing kept. Key
  resolution is now one documented order for every profile:
  `OPENAI_API_KEY`, `OPENCODE_API_KEY`, then the active preset's own names in
  declaration order, then the stored credential. Only the *active* preset's
  names are read — a `GROQ_API_KEY` in the environment still cannot
  authenticate a `deepseek` profile, so a valid secret never travels to an
  endpoint the user did not associate with it — and a profile whose id is not
  in the preset table reads no environment at all. The two universal names
  keep working unchanged for the default profile.
- **`lca -p --mode json` emits `extension-event`** (gh #264). The sink that
  serializes a turn had no arm for extension lifecycle events, so every one of
  them fell into the catch-all and vanished. A script waiting for a `disabled`
  event to learn an extension was evicted waited forever; one counting
  lifecycle events undercounted — silently, with no error and no gap marker,
  against a row the docs promised. `--mode rpc` shares the same sink, so one
  arm fixes both and there is no second emitter to drift. Text mode stays
  silent, as for every other host diagnostic. A new row in the sink's tests
  reads the event table out of `docs/headless.md` and fails if any documented
  kind has no producer, so a future refactor cannot re-drop one.
- **`/formatter run` no longer reformats build output or walks Git internals**
  (gh #357). A bare run descended every directory the workspace allowed, so it
  handed `target/**/*.rs` (build-script output) to `rustfmt --write` — rebuild
  churn at best, divergent generated code at worst — and stat'ed every file
  under `.git/objects` through the capability boundary for nothing. It now
  skips `.git`, `target`, and `node_modules` at descent time, using the same
  `lca_tools::SKIP_DIRS` constant the `glob`/`grep` walker already used rather
  than a second copy that could drift. Matching is by path *component*, so
  `src/targeted/x.rs` still formats. Naming a generated path explicitly
  (`/formatter run target/gen/out.rs`) is refused with a reason instead of
  being formatted, and the auto-format hook refuses one the same way.
- **Self-update fetches are bounded in time and size** (gh #355). `lca update
  --check` and `--changelog` awaited the release feed with no timeout, so a
  stalled host hung the command indefinitely, and `install_binary` collected an
  uncapped body into memory before the installer's verify-then-move path could
  run — a hostile mirror could exhaust memory first. `plain_get` now carries a
  30-second deadline over the whole request *and* body read, and an 8 MiB cap
  enforced while the frames arrive (a declared `Content-Length` over the cap is
  refused from the header, before any body is read). Every caller inherits both
  — the self-update paths, the catalog fetch, and anything later — and a
  refusal names the host and the limit. The daily background check keeps its
  tighter 5-second FR-CFG-6 bound.
- **A disabled MCP no longer spawns its servers or connects to its remotes**
  (gh #418 Phase E, gh #266). `registry::assemble` built the bridge *after*
  `apply_enablement` had already swept the registry, so `mcp` was never
  consulted by the disablement rules and `McpManager::load` started every
  configured server regardless — `lca ext disable mcp`, `/mcp disable`, and
  the Extension Hub all had no effect on startup. Enablement (the persisted
  record **and** `--no-mcp`) is now asked before anything connects, and the
  bridge is registered before the sweep so it is covered like every other
  handle.
- **MCP no longer blocks startup** (gh #418 Phase E, NFR-3). `McpManager::load`
  connected every server synchronously, so cold start was the sum of every
  server's handshake or timeout — a hung server could hold the interface shut
  for a minute. Servers now connect on background workers and assembly
  returns at once; a server's row reads `connecting` until it lands. The first
  turn waits up to 10 seconds (pi's `startupWaitMs`) only for servers whose
  tools are declared directly to the model, so they appear in the first
  request; `tool_search`, codemode, and resource tools wait on demand
  instead, and a server still connecting is reported rather than silently
  missing.
- **An MCP server whose authorization server is on another host can sign in**
  (gh #418 Phase D, gh #290). Two defects made that impossible: `sign_in`
  passed an empty challenge map to discovery, so the RFC 9728
  `resource_metadata` pointer a 401 supplies was discarded and the
  discovery branch was dead; and grant assembly covered only the server's
  own base-URL host, so the metadata fetch and token exchange were denied by
  the capability layer before any prompt. The whole `WWW-Authenticate`
  challenge now survives the round-trip, and a discovered host is admitted
  **for the sign-in flow only** — dropped when the flow ends, never
  persisted, and invisible to ordinary tool traffic. A host the user
  configured (`oauth.authServerMetadataUrl`) joins the static grants the way
  the server URL already does. Discovered endpoints must be `https://`, or
  `http://` on loopback (RFC 8252), checked before any request leaves, and
  the sign-in copy names an outside host (`via <server>: authorization host
  <host>`) so the user sees where the credential goes.
- **`mcp.json` is written atomically and owner-only** (gh #418 Phase C, gh
  #267). Editing a server through `/mcp` used to rewrite the file in place
  with the process umask and the host's own JSON formatting, so an
  interrupted write could leave a truncated config and a config carrying
  `env`/`headers` secrets could land world-readable. Writes now go to a
  dot-prefixed sibling, are restricted to the owner (`0600`, an owner-only
  DACL on Windows), and are renamed over the target; the file's existing
  indentation is preserved. The `exposure` confirmation also names the
  server instead of echoing the raw argument back.
- **A stdio MCP server's `env` and `cwd` work as documented** (gh #418 Phase
  C). `env` was refused outright with "needs the 0.7 process-env seam", and
  `cwd` was read as a capability scope name rather than a path. `env` now
  overlays the inherited environment (values expand `${NAME}`; the child
  keeps `PATH`), and `cwd` is a directory path resolved against the session
  working directory with `~` expansion, still confined to the workspace
  scope. A server that dies during the handshake now reports the last 2 KB of
  its stderr, so `/mcp` says why, not just that it did.
- **Two MCP servers or tools whose names differ only in `-` vs `_` no longer
  collide silently** (gh #418 Phase C). Server names that normalize into one
  `mcp__` namespace are refused at load with both names in the message;
  colliding or over-64-character tool names take pi's deterministic
  `_<hash8>` suffix, so both tools stay separately callable.
- **A JSON-RPC error with `"id": null` is reported instead of dropped** (gh
  #418 Phase C, gh #336). Servers that answer a parse or internal error that
  way read as "answered without our response", which hid the server's own
  explanation.
- **Tool-annotation hints reach the approval prompt** (gh #418 Phase C, gh
  #303). `SharedPrompt` — the object every capability engine holds — used the
  trait's default `ask_annotated`, which drops the hints and calls `ask`, so
  `readOnlyHint`/`destructiveHint` were stripped before the real prompt saw
  them. Hints stay display-only: a `readOnlyHint: true` call is approved or
  denied by exactly the same rules as before.

### Changed
- **MCP is now part of the binary's core tooling rather than a separately
  published extension** (gh #418, ADR-0047, superseding ADR-0045). Nothing
  behaves differently: the same servers, the same `mcp.json`, the same
  `mcp__<server>__<tool>` names, the same per-spawn permission prompt. What
  changed is where it lives and what gets published. The crate moved from
  `extensions/mcp` to `crates/lca-mcp` as a first-class subsystem, its
  single-consumer wire kit (`lca-wire-mcp`) folded into it as `src/wire.rs`,
  and its WASM guest was deleted along with the `mcp.wasm` artifacts — the
  `tool-catalog` world exports no `net`, `oauth`, or `credentials`, so the
  component could never serve the HTTP/OAuth/resources features it advertised.
  If you installed MCP from the registry, it now ships with the binary:
  remove the `mcp` entry from your `extensions`/`catalog` state and nothing
  else changes. `lca-core` no longer carries any MCP dependency at run time.

### Added
- **Extensions-only publish (`extensions-abi-0.6-20261011`).**
  Thirteen shippable first-party extensions ride the `abi-0.6` line;
  this release publishes eight: `antigravity` 1.0.1 (listing-fuel
  fix #236 changed its bytes since v0.6.0, hence the patch bump)
  plus first publishes `anthropic`, `openrouter`, `github-copilot`,
  `meta`, `kimi-coding`, `llama`, and `mcp`, all 0.1.0.
  `openai-compatible` 1.0.0, `skills` 1.0.0, `compaction-default`
  1.0.0, `codex` 0.1.0, and `grok` 0.1.0 are source-identical since
  v0.6.0 and keep their versions and tags untouched (immutable tag
  rule). No binary rides this release (it stays on the `unstable`
  line); no WIT or schema change.

### Security
- **Binary MCP resources staged in shared world-writable `/tmp` (gh
  #344).** `read_result` wrote a server's binary resource payload to
  `<system tmp>/lca-mcp-resources/{server}-{counter}-{safe}` with a raw
  `std::fs::write`. That directory is shared and world-writable, the
  counter restarts at 0 on every process, and the write follows symlinks —
  so a local user could pre-plant a link at a predictable name and have the
  user's own process overwrite any file they can write. The same call also
  bypassed the capability scopes entirely and never cleaned up. Staging now
  goes through a new `Capabilities::fs_write_new` on the `temp` scope:
  `create_new` (a planted entry is an error, never a target), owner-only
  `0600` on the file and `0700` on its directory, the scope check running
  first and recording refusals, and the session-temp guard removing the
  directory at exit. **Found on the way:** the `temp` scope was itself
  unusable on a real installation — its root is `<data>/tmp/<session>`, so
  the state-directory exclusion refused every resolution, and the sibling
  test fixtures hid it. `temp` now gets the same sanctioned exemption
  `private` already had, still confined to its own root.

- **Two hostile-artifact surfaces in `lca-registry` (gh #348, #350).**
  - **gh #348:** a catalog `id` from `[[extensions.catalogs]]` was joined
    straight onto `<data>/catalogs/{id}.toml`. A *trusted* project's
    `.lca/config.toml` carrying `id = "../../cron.d/malicious"` made the
    next refresh write attacker-chosen bytes wherever the user can write
    (an untrusted project file contributes no catalog at all — FR-PERM-9 —
    so the attack needs a repo the user chose to trust). The id is now a
    charset — ASCII letters, digits, `-`, `_` — enforced when the
    configuration is parsed *and* again where the cache path is built, so
    `.`/`/`/`\` are outside the vocabulary rather than blocked. Both
    refusals name the id and the rule.
  - **gh #350:** `read_archive` decompressed members with no read ceiling.
    Components had no size check at all, `resources/*` was measured only
    *after* `read_to_end` had already allocated, and `extension.toml` had
    no bound anywhere — so a few-kilobyte archive could expand to gigabytes
    in RAM during `lca ext install`. Every member now reads through
    `Read::take(limit + 1)` and is refused past its limit
    (`MAX_COMPONENT_BYTES` 64 MiB, the existing
    `RESOURCE_FILE_MAX_BYTES` 1 MiB, `MAX_MANIFEST_BYTES` 1 MiB), the
    member count is capped before any entry is opened, and the download
    itself is capped. Every refusal names the member and the limit and says
    to repackage it smaller.

- **`git+` sources could smuggle git options (gh #349).** The registry's
  git resolver built `git clone --quiet <url> <dir>` and
  `git checkout --quiet <rev>` from user-supplied url/rev strings with no
  `--` end-of-options separator, and `parse_git_source` never rejected a
  value starting with `-`. A source like `git+--upload-pack=<cmd>` reached
  git as an option, and `--upload-pack` runs a command for local
  transports — arbitrary execution during `lca ext install`. The parser
  now refuses an empty or `-`-prefixed url or rev, the clone passes its
  positionals after `--`, and the checkout relies on that validation
  (`git checkout -- <rev>` would read the rev as a pathspec). The git
  subprocess also gained a 60 s timeout with both pipes drained while it
  runs, so a stalled transport cannot wedge the install — the clone path
  only ever used `Command::output()`, which already drained, so no pipe
  deadlock existed to fix there.
- **`logout` left preset profile secrets on disk (gh #293).**
  `openai-compatible`'s `run_logout` deleted only the bare
  `api_key`/`base_url`, while the login path also writes
  `profile.<id>.api_key`, `profile.<id>.base_url`, and the cached
  `models`/`model`/`preset` settings. A user who ran `/logout` or
  `lca auth logout openai-compatible` believed their keys were gone
  while every preset profile secret stayed in
  `<data>/credentials/openai-compatible.json` and `auth status` kept
  reporting ready. `run_logout` is now the exact inverse of the write
  paths: it purges the bare keys, the cached settings, and every
  `profile.<id>.<field>` for each preset in the merged table, with the
  per-profile field list hoisted to one shared constant so login and
  logout cannot drift apart again. A `profile.<id>.*` key whose preset
  id has vanished from the table cannot be enumerated (the capability
  surface has no credential listing) and survives; the `ponytail:`
  upgrade path is a `credentials_list` interface on the ABI.
- **Debug-log secret hygiene (gh #343, #342).** Two defects in
  `lca-debug`'s `~/.lca/logs/` write path.
  - **gh #343:** `redact_url`/`redact_json`/`redact_cookie` matched
    keys by exact equality against a six-name list, so standard OAuth
    keys — `access_token`, `id_token`, and the callback `code` — were
    written in cleartext to `debug-wire.log`, along with
    `sessionId`-style camelCase cookies. Matching is now one shared
    `is_sensitive_key` over a normalized name (lowercase, `_`/`-`
    stripped; exact + suffix), so `access_token`, `accessToken`,
    `Access-Token`, `client_secret`, `device_code`, and `authorization`
    all mask. A **container** under a sensitive key (e.g. `token`,
    `credentials`, `auth`, `session`) masks every leaf inside it — keys
    and structure stay visible, but an innocuous child name (`value`,
    `id`, `bearer`, `AccessKeyId`) no longer lets a leaf through.
    Usage counters (`max_tokens`, `input_tokens`, `prompt_tokens`) and
    `token_type` deliberately stay visible; the accepted over-mask
    (`monkey`, `zip_code`) is documented — a false mask costs a debug
    line, a false pass leaks a credential.
  - **gh #342:** `RotatingWriter` opened the debug files with the
    default mode (umask 022 → `0644`), world-readable to any local
    user. New opens use `0600` and repair a pre-existing `0644` file
    on the next open; rotation renames the already-repaired file, so
    the `.1` spare is `0600` too. Windows relies on the inherited
    `%USERPROFILE%` profile ACL (documented, not asserted).
- **`{file:}` substitution bypassed the credential store through a
  symlink (gh #341).** `read_secret_file` refused protected stores on
  the lexical path only, then `std::fs::read_to_string` followed
  symlinks, so a workspace link `key-link ->
  ~/.lca/credentials/anthropic.json` read a plaintext API key or
  refresh token into a config value at load time — the
  untrusted-project attacker reading the credential store. The path is
  now canonicalized before the refusal re-runs, so a link into
  `credentials/`, `sessions/`, or `grants` refuses by its resolved
  location, and any path resolving into the state directory `~/.lca`
  refuses wholesale (the SRDD state-directory exclusion). A dangling
  link refuses like a missing file; legitimate relative, absolute, and
  `~/` files are unchanged. Both callers route through the one
  function (`lca-config` config layers and the `provider-presets.toml`
  login path).
- **Folder-trust shell analyzer escape hatches closed (gh #345,
  #346, #347).** Three `p1-high` defects let a hostile repository
  run network egress, arbitrary inline code, and out-of-workspace
  reads with no prompt while the folder was trusted.
  - **gh #345:** `segment_ok` read the git subcommand from
    `toks[i + 1]`, so any global flag before it hid the subcommand
    from the egress and config checks — `git -C . push`,
    `git --no-pager push`, and `git -c user.email=x config` all
    auto-approved. A new `git_subcommand` parser classifies past
    global flags (consuming `-C`/`-c`/`--git-dir`/`--work-tree`/
    `--namespace`/`--config-env` and their `=`-attached forms) and
    refuses the invocation when it carries a config override, an
    `--exec-path`/`--super-prefix` redirect, a dangling
    flag-argument, or no subcommand at all.
  - **gh #346:** `shell::path_ok` normalized lexically only, so a
    workspace symlink pointing outside (`ln -s /etc ./etc_link`)
    auto-approved `cat ./etc_link/passwd`. It now resolves through
    the same `canonicalize_deepest` the `fs` scope uses. The shared
    helper also folds a lexically-normalized tail, closing a
    parent-traversal escape in `ScopeRoots::resolve` (FR-PERM-12)
    when a `..` sat in a non-existent tail.
  - **gh #347:** no language runtime was covered by `DENY_PROGRAMS`,
    so `python3 -c "<code>"`, `node -e`, and the like
    auto-approved — inline code is not in the workspace and cannot
    be reasoned about. A new `INTERPRETER_EVAL_FLAGS` table refuses
    inline eval/module flags per runtime; script-file invocations
    (`python3 scripts/run.py`) stay scoped. Known trade:
    `python3 -m pytest` now reviews. **Fix-up (same day):** the table
    matched by exact basename and exact token, so three ordinary evasion
    forms still auto-approved — versioned names (`python3.12`),
    Windows executable suffixes (`python.exe`, `bash.exe`), and the glued
    short-flag idiom (`python3 -c'print(1)'` tokenizes to `-cprint(1)`).
    `basename` now strips `.exe`/`.cmd`/`.bat`/`.com`/`.ps1`, the name
    match accepts a tight `[0-9.]t?` version suffix, and a single-letter
    eval flag matches its glued form; the first also closes the same
    bypass in `DENY_PROGRAMS`.

### Fixed
- **A provider/core batch where each defect was cheap to paper over and
  expensive to leave (gh #305, #306, #274, #275, #277, #356).**
  - **gh #305:** a second `ToolCallStart` for an already-open call id recorded
    `ProtocolError::DuplicateStart` and then renamed the open call and cleared
    its fragments. The follow-up `ToolCallEnd` therefore published a call whose
    arguments were stitched from fragments belonging to the earlier start, and
    the host ran it. A duplicated start now *poisons* the call: it stays
    tracked (so later fragments and the end are not misreported as orphans) but
    never becomes runnable, and the EOF report does not double-count a problem
    `DuplicateStart` already named.
  - **gh #306:** `validate_against_schema` bailed out whenever `type` was not a
    string, so `["string", "null"]` - the standard spelling for nullable - made
    the whole subtree unvalidated, and a type-less schema did the same. `enum`
    was never inspected, so `extensions/project-memory`'s
    `"scope": {"enum": ["project", "global"]}` accepted anything. Union types,
    `enum` (with or without `type`), and value-driven structural recursion are
    all enforced now.
  - **gh #274:** the curated price table billed `cache_read` at the full input
    rate, and the turn loop copied only `usage.cost` onto the persisted record,
    leaving `cost_input`, `cost_cache_read`, and `cost_cache_write` at `0.0`.
    Those three fields are exactly what the cache-waste measurement reads
    (ADR-0017), so on any table-priced model the cache saving was reported as
    precisely `$0.00`. Cache reads and writes now carry their own published
    rates and the breakdown is persisted. Measured: a gpt-4o turn of 2M input +
    1M cache-read + 1M output costs **$16.25** rather than $17.50, with
    `cost_input = 5.0` and `cost_cache_read = 1.25`.
  - **gh #275:** the stable-prefix boundary was computed before the context
    transforms ran, so an inserted message shifted every index after it; and it
    clamped to the compaction summary whenever a compaction had ever been seen,
    which froze the boundary there for the rest of the session - everything
    after the summary was permanently un-cacheable. It is now computed on the
    post-transform list and advances normally after a compaction. Separately,
    `sent_stable` now records only the durable prefix, not the per-turn
    injections appended after the user message; those are never logged
    (FR-CTX-4), so recording them made the *next* turn read as a divergence and
    narrow the boundary for no reason.
  - **gh #277:** the overflow-recovery branch emitted `Error` and `TurnEnded`
    before attempting to recover. A recovered turn then carried on streaming
    after its own `TurnEnded`, and an unrecovered one emitted both events
    twice. Nothing is emitted before the attempt now: recovery is marked by
    `CompactionStarted`/`CompactionEnded { reason: "overflow" }`, and
    `TurnEnded` fires once when the turn really ends.
  - **gh #356:** every native provider bridge flattened its wire kit's
    `StreamFailure { message, class, retryable }` into
    `DispatchError::Failed(String)`, forcing the turn loop to re-derive the
    classification by looking for `"provider returned HTTP NNN"` in the text.
    An `invalid_api_key` body, an expired session, or a vendor-worded 400 fell
    through to `("transport", true)` and was retried three times with backoff
    before failing on the wrong code. `DispatchError::ProviderFailed` carries
    the class and retryability across the boundary, and the text fallback (the
    only route a WASM guest has) now recognizes auth refusals.

- **Three places where the WASM path silently did less than the native path- **Five `lca-tools` durability and hygiene defects, each of which failed
  silently rather than loudly (gh #318, #319, #320, #321, #322).**
  - **gh #318:** `process_wait` and `pty_wait` waited on the child while
    holding the handle table's mutex and then left the entry in place, so
    every normally-exiting child and PTY leaked a `HandleEntry` (and a PTY's
    master fd) for the rest of the session. The entry is now removed before
    the blocking wait, which also stops a slow child from stalling every
    other handle operation; a wrong-type handle is still refused without
    being removed, so asking the process API about a PTY cannot destroy it.
  - **gh #319:** the loopback OAuth redirect server answered browsers with a
    malformed HTTP/1.1 response - 36 leading spaces on two lines, bare `\n`
    endings, `content-length:63` for a 104-byte body - so a browser either
    hung waiting for the bytes it was promised or read the indentation as
    obsolete header folding. The callback query decoder also widened each
    percent-decoded byte as Latin-1 (`char::from(byte)`), turning `%C3%A9`
    into `Ã©`, corrupting any literal non-ASCII byte the same way, and
    mapping a bad hex digit to `0` so `%zz` became a NUL byte. The reply is
    now a correctly framed response with a length that matches its body, and
    the decoder accumulates bytes and decodes UTF-8 once, preserving a
    malformed `%` sequence verbatim.
  - **gh #320:** `state_write` wrote in place with `std::fs::write`, which
    truncates before it writes - a crash or full disk mid-write left a
    shorter value that still parsed, so an extension's state was lost with no
    error anywhere. It now writes a `.<key>.tmp` sibling and renames it into
    place, `state_list` ignores a leftover temp file, and the per-namespace
    byte cap does not count one (otherwise a single crash could make a
    namespace permanently unrefillable). The temp name is dot-prefixed rather
    than `Path::with_extension("tmp")`: key `foo.bar` would map to `foo.tmp`
    under `with_extension`, colliding with the real key `foo.tmp`. A leading
    dot is now refused as a key so that space belongs to the temp files.
  - **gh #321:** `truncate_head_tail` admitted the first line and the last
    line unconditionally (`&& index > 0`, `&& back > 0`), so a two-line file
    of 100 KiB lines was emitted whole - 204801 bytes against a 64 KiB cap,
    measured live through the `shell` tool. Whole lines are now only accepted
    when they fit their budget; when a single line is longer than its budget
    the head and tail are byte windows cut on UTF-8 character boundaries, and
    the omission marker cites bytes instead of an impossible line range
    (`showing lines 1..0 and 3..2` was the first draft's output). Measured
    after: 65535 bytes of content under the 65536 cap.
  - **gh #322:** the Win32 Job Object calls were unchecked.
    `Job::new` ignored `SetInformationJobObject`'s return, so a job that
    failed to acquire `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` was still handed
    back as usable - the tree-kill guarantee degraded to "kill the child"
    with nothing said. `platform_exec` went further and ignored both
    `Job::new()` and `assign(pid)`, so a shell spawn could proceed with no
    job at all. The setup result is now checked (closing the handle and
    returning `None` on failure), and all three Windows spawn sites
    (`ops`, `process`, `pty`) route through the one checked `Job::attach`,
    failing loud rather than running a tree they cannot kill.

- **The embedding SDK lost state and capabilities that only show up in a long
  session, and three durability defects beside it (gh #309, #310, #313, #312,
  #317).**
  - **gh #309:** `Session::create` accepted a `data_dir`, used it to open the
    store, then dropped it — and `AgentConfig::default()` leaves `data_dir`
    empty, so every shell child of an embedded turn saw `LCA_DATA_DIR=""`, the
    one variable `docs/configuration.md` documents for finding the agent's data
    home. The SDK also had no `resume` (though `id()` documents itself as the
    handle a resume takes), no way to attach an `ExtensionRegistry`, and no way
    to cancel a running turn — the `CancelFlag` was a local of `send`, so a host
    could watch a turn start and not stop it. All four exist now:
    `Session::resume`, `with_extensions`, `with_agent_config`, `cancel`, and
    `send_with_cancel`.
  - **gh #310:** `send` rebuilt `ToolExecutor`, `AgentConfig`, and the cancel
    flag on every turn. That reset the executor's `ReadTracker` (so
    `edit_requires_read: true` refused an edit on turn 2 of a file read on turn
    1), its subfolder-instruction dedup, and the config's `sent_stable` cell —
    which is exactly what FR-CACHE-6's prompt-cache divergence check reads, so
    the check could only ever compare a turn with itself. All three are now
    session-scoped and shared across turns.
  - **gh #313:** `migrate` rewrote `log.jsonl` with `std::fs::write`, which
    truncates before it writes — a crash or full disk mid-write left a truncated
    session, and the backup meant to survive that was written by the same code
    path. It now uses the crate's atomic temp-file-and-rename helper. Backups
    were named `migrate-<unix-seconds>`, so two migrations inside one second
    collided and the second silently overwrote the first; milliseconds plus a
    collision-suffixed `create_dir` claim make each backup its own directory.
  - **gh #312:** the HTML export emitted `class="messagemessage-user"` — two
    class names concatenated into one token that matches none of the
    stylesheet's `.message-*` selectors, so every exported transcript rendered
    unstyled — and interpolated an image's `media_type` unescaped into the
    `data:` URI, where a quote closes the `src` attribute. Both fixed, and the
    sidebar's `href` now escapes the anchor id the same way the article does.
  - **gh #317:** `ScratchDir::new` keyed its path on name and pid only, so two
    tests in one binary sharing a name computed the same path and the second
    call's `remove_dir_all` deleted the first one's live directory. A
    process-wide counter makes every call unique. This exposed one test that had
    been relying on the collision (`extensions/mcp`'s `engine(name)` plus a
    second `scratch(name)`), which now hands its root back instead.

- **Three places where the WASM path silently did less than the native path
  (gh #314, #315, #316).** One crate (`lca-subscription`), one failure mode: an
  omission that compiles, succeeds quietly, and costs a feature.
  - **gh #314:** the WASM provider guest implemented the net
    request/status/body/close calls and the three credential calls, and stopped.
    The two it left out have trait defaults that look fine and are not:
    `net_response_headers` answered `Ok(vec![])`, so a WASM codex/grok turn
    never saw `retry-after` and could not honour a server-directed backoff the
    way native does; `resource_read` answered `NotFound` for every path, so a
    WASM provider could not read its own `resources/` bag — presets, curated
    model lists, any data-only package content — even though the
    `lca:host/resources` import was already bound in the provider world. Both
    now delegate, with the same error mapping as their siblings.
  - **gh #315:** a token refresh purged credentials only on HTTP 401. An
    expired or revoked *refresh token* is RFC 6749 §5.2's **400
    `invalid_grant`** — the request was well formed, the grant is dead — so the
    dead token stayed in place, every later turn retried it, got the same 400,
    and the user was trapped in a refresh loop whose only exit was knowing
    `/logout` exists. A 400 whose parsed `error` field is `invalid_grant` now
    purges and says to run `/login`; every other status keeps the credentials,
    including a 400 with a different error code, because a wrong client id or a
    server outage says nothing about whether the token is still good.
  - **gh #316:** the WASM model listing hardcoded `extras: Vec::new()`, so a
    WASM provider advertised every model as having no metadata — no vision, no
    thinking levels, no prompt-cache lifetimes — while native kept it. The ABI
    record already carried the field, so no WIT change was needed; the
    conformance fixture's models also carried *empty* extras on both sides,
    which is why the cross-mode equality test agreed and could not see it. The
    fixture now publishes real metadata, so that test is no longer vacuous.

- **The configuration vocabulary drifted from itself, and unknown keys were
  silent (gh #272, #273).** Several hand-maintained lists describe one
  vocabulary - the `Config` fields, `Config::resolved()` (what `lca config` and
  `/settings` print, FR-CFG-2), `KNOWN_KEYS` (what a `LCA_*` variable can set),
  and the TOML application loop - and nothing checked that they agreed.
  - **gh #272:** `skills.inject_matched` was missing from `KNOWN_KEYS`, which is
    the list the environment loop iterates, so `LCA_SKILLS_INJECT_MATCHED=true`
    parsed (the parser knew the key) and then did nothing at all. Seven
    settings - `compaction.strategy`, `provider.retry_base_delay_ms`,
    `provider.retry_max_delay_ms`, `queue.steering_mode`, `queue.follow_up_mode`,
    `thinking.budgets`, `extensions.catalogs` - were missing from `resolved()`,
    so a user who set one and went looking for it found no row, which reads as
    "this setting does not exist". Both lists are complete, and the new rows
    carry their winning source like every other row.
  - **gh #273 (silent keys):** the loader walks a fixed list of known keys and
    looks each up, so a key outside the list was never examined - the branch
    that reports an unknown key was unreachable. A typo such as
    `compaction.theshold = 0.5` loaded clean and changed nothing. The walk now
    runs over the parsed table and refuses anything outside the vocabulary by
    name, whether spelled dotted or as a section, at any depth; table-valued
    keys keep owning their subtrees. The refusal says what to check (the
    spelling, or a newer `lca`), and an untrusted project file stays inert
    including its junk keys, so a repository cannot brick startup (FR-PERM-9).
  - **gh #273 (double parse):** `[provider]` retry keys were applied by two code
    paths - the section-form lookup and a hand-written block after the loop -
    which agreed on two of the three keys and handled neither the third
    (`retry_max_delay_ms`) nor a future edit. One path now.
  - **`/settings` filters as you type.** pi's `ConfigSelector` filters its
    list; LCA's did not, which made a 40-row sectioned list a scroll hunt. The
    query matches the key, the label, and the description, case-insensitively;
    `Esc` clears it before closing; an empty result says `No matching settings`;
    and the cursor indexes the filtered set, so a filter can never leave `Enter`
    pointed at a row that is not on screen.
  - **The class fix:** a drift guard pins the five mirrors against each other
    (`Config` fields, `resolved()`, `KNOWN_KEYS`, `TABLE_KEYS`, and the
    `docs/configuration.md` key table), so the next key added to one and
    forgotten in another fails a test instead of shipping. It was measured
    biting: dropping a `resolved()` row, dropping a `KNOWN_KEYS` entry, and
    removing a doc row each turn a named row red.

- **Three defects in the OpenAI-family wire kit (gh #337, #338, #359).**
  All in `crates/lca-wire-openai`, the SSE and request/usage seam shared by
  every OpenAI-shaped provider.
  - **gh #337:** both streaming decoders did
    `buffer.push_str(&String::from_utf8_lossy(chunk))` **per network chunk**.
    HTTP byte streams do not align with character boundaries, so a
    multi-byte codepoint straddling two chunks became `U+FFFD U+FFFD`:
    an emoji, an accented letter, or CJK text corrupted whenever a packet
    boundary landed mid-character — a matter of timing, not input. Frames
    now split at the byte level and only a *complete* frame is decoded, so a
    split codepoint waits harmlessly in the buffer. Both decoders share one
    framer, which is what stops the twins drifting again.
  - **gh #338 (images):** `build_responses_body` filtered user content for
    `Text` only, so an attached image was discarded — and an image-only
    message joined to an empty string and vanished, meaning the model
    received nothing at all. Images now serialize into the input message's
    content array as `input_image` parts with the bytes inlined as data
    URLs (pi's shape, `detail: "auto"`).
  - **gh #338 (cache accounting):** `responses_usage` hardcoded
    `cache_read: 0` instead of reading
    `usage.input_tokens_details.cached_tokens`, so every prompt-cache hit on
    codex and grok was counted as fresh input and the footer and `/usage`
    overstated spend. It now maps the field and reports `input` as the
    uncached remainder, matching the Chat Completions mapper.
  - **gh #359:** the Responses driver dropped its trailing buffer at EOF
    (a final frame that lost its `\n\n` terminator was discarded) and
    stayed silent on a 200 body carrying no frames (an empty assistant
    message, indistinguishable from a model that said nothing). EOF now
    parses the tail — a `data:` line means a frame that lost its
    terminator, anything else surfaces as a transport error — and a body
    with no frames is an explicit non-retryable transport failure. The
    Chat Completions twin had the same gaps; it gets the same rule.

- **An extension's own fetch to an ungranted host could not ask for
  consent (gh #302, 0.6.x half).** Two defects stacked. `webfetch` ships
  `[capabilities.net] hosts = []` on purpose — every host arrives by
  consent or an ad hoc grant — but the capability layer required a
  *non-empty* host list to consider `net` declared, so the first fetch died
  with `no network capability declared` on the declaration gate, before the
  pattern check a prompt could have helped. And the deny below it was
  terminal: the ad hoc-grant modal and store lived in the tool/permission
  layer and were never consulted for an extension's host-mediated fetch, so
  every URL failed without `--allow-host` (which worked only because it
  plants a grant before the check). Declaration is now the gate
  (`net_declared`, so "declared, no hosts yet" stays distinct from "never
  declared"), and a user-initiated fetch asks where the deny happens —
  allow once, allow always (persisted per project, silent afterwards), or
  deny — through the same modal, store, deny-rule precedence and yolo answer
  every other approval uses. The ask belongs to the call: a provider's
  background endpoint probe still never prompts. Headless is unchanged, and
  a manifest with no `net` table still gets the flat refusal and is never
  offered consent. The WIT `net` import for tool-world guests — without
  which the WASM half cannot fetch at all — rides the 0.7 train
  (#123/#149/#354/#284/#352/#353) and is not in this change; the WASM half
  still reports the deferral plainly.

- **Every turn's cost was counted twice in the footer (gh #327).** Two
  accumulation points existed for the same money: the stream's
  `TurnEvent::Usage` handler grew `chat.usage.*` (including `cost`) as
  events arrived, and `TurnState::reap` added `outcome.usage.cost` again at
  completion — so the footer, `/session` and `/usage` reported double the
  real spend. The stream-side `+=` had a second defect under gh #276's
  partial-usage model: a provider refining its totals across two events
  double-counted even with `reap` removed. Stream events now drive only
  display (the footer's prompt size, the tok/s meter, a turn-local view
  that `absorb`s a refinement and sums a new tool step); the session totals
  fold core's authoritative per-turn usage **once** through one
  `Chat::fold_turn_usage`, which the record-replay path shares so a resumed
  session still reports its history.

### Security
- **Login option routing ignored the provider, storing a key in another
  provider's credential namespace (gh #265).** `LoginFlow::pick` matched
  options by `id` alone across *all* providers and then overwrote the
  requested provider with the matched row's owner. Several providers
  declare `id: "api-key"` / `id: "oauth"`, so choosing "api-key" under
  OpenRouter stored the key in Anthropic's namespace — and that provider
  then gets *sent* the key. A cross-vendor credential leak, not a
  mislabelled row. Matching is now scoped to the named provider, with the
  id-only fallback kept only for the bare `/login` pick (whose rows carry
  their own provider). A miss under a named provider is a named refusal
  rather than a silent reroute.

### Fixed
- **Project memory never froze an *empty* snapshot, so a mid-session
  `learn` mutated `messages[0]` (gh #296).** The pin was
  `Mutex<Option<String>>`, which conflates "not yet initialised" with
  "initialised but empty": a session starting with no `MEMORY.md` stored
  `None`, so a `learn` mid-session (which writes the file) made the next
  transform **re-read** and inject a block into the root system prompt on
  turn 2+ — the same KV-cache bust class gh #299 had just fixed, from a
  different producer. The pin is now tri-state
  (`Mutex<Option<Option<String>>>`: outer = frozen, inner = the snapshot),
  so empty freezes as an initialised `None` and no later `learn` can
  reopen it. Second defect in the same function, also fixed: the read was
  `MemoryScope::Project` only, so facts learned with `scope: "global"`
  were never injected at all — global and project memory are now read
  together into the one snapshot block (ADR-0017, FR-CACHE-5).
- **Skill injection and instruction deltas mutated the vendor's root
  system prompt (gh #299, and the same class in gh #217's path).** Both
  appended their text as a `MessageRole::System` message. Every provider
  builder joins all `System` messages into the vendor's top-level
  `system` / `systemInstruction`, so a *trailing* system message is not
  trailing at all: it is hoisted into the ROOT prompt. The root grew on
  every turn a skill matched and shrank again on the next turn that
  matched nothing — two full KV-cache busts per match/unmatch cycle, and
  the cache-boundary narrowing never fired because the leading messages
  were untouched. Both now send a `User`-role message carrying a
  `[skill instructions]` / `[instructions changed]` wrapper, which stays
  in the conversational stream where it belongs; `messages[0]` and the
  hoisted root string stay byte-identical across match and unmatch turns
  (ADR-0017, FR-CACHE-5/6).
- **A JSON-RPC notification desynchronized the stdio MCP transport
  (gh #291).** `round_trip` and `Session::resource_templates` each read
  exactly one line after sending a request, so a server notification
  (`method` with no `id` — `notifications/message`,
  `notifications/progress`, `notifications/tools/list_changed`, all
  ordinary MCP traffic) was parsed as the reply: "answered without a
  result" from one reader, "answered another call" from the other. The
  real reply then stayed queued, so **every later call on that connection
  read a stale line** — a permanent desync that broke every call after the
  first notification. The selection rules now live once in
  `lca_wire_mcp::response_for` (notifications discarded, our `id` answered,
  a foreign `id` failing loudly and named, bad JSON still a hard error) and
  both readers use it. `Reply` keeps a JSON-RPC error for our id distinct
  from a transport failure, so an unimplemented `resources/templates/list`
  still reads as "no templates" while a desynchronized stream can no
  longer masquerade as an empty listing.

### Changed
- **The reasoning-signature extras bridge is recorded in-tree and pinned
  (gh #289).** The 0.6 `content-block` variant has no `reasoning` case, so
  a thinking run's replay signature crosses the ABI in the message's
  reserved extras map under `thinking-signature`, and the guest rebuilds
  the `Reasoning` block from it. That workaround is live-verified
  (`gemini-3.8-flash` tool turns complete, follow-ups answer) and is now
  the signature transport for the whole 0.6 line, documented in
  `docs/abi-versioning.md` (the 0.6-compat window) and
  `docs/providers/antigravity.md`. The host end — writing the key
  alongside, forwarding `message.extras` verbatim, never overwriting a
  caller's own key — is pinned by `lca-ext-host`'s `reasoning_bridge_tests`;
  the guest end by the antigravity extension's rows. The typed `reasoning`
  WIT case stays 0.7 work (#352, on the #123/#149/#354/#284/#302/#353
  train) and no `wit/` file changed here.

### Fixed
- **A cancellation test's hang guard fired after CI's own kill (CI
  `39044027340`).** `a_fresh_turn_starts_clean_after_cancellation` guards
  its two turns with `tokio::time::timeout` so a hang names itself instead
  of taking the pipeline down. The guards sat at 30 s, but nextest
  terminates a test after 120 s (`.config/nextest.toml`), and on a hosted
  runner the whole test exceeded 60 s of *load* while its second turn's
  own work stayed in the tens of milliseconds — so the guard fired after
  the job was already dead, printing nothing and letting the anonymous
  `TIMEOUT` store the failure (and the panic could not bring the process
  down either: the spinning guest's thread is never joined). Both guards
  now sit at 110 s, just under the terminate, which buys the elapsed
  breakdown and a named failure. No assertion was touched; the test's real
  elapsed-time claim (turn one stopping within 3 s of the cancel) is
  unchanged. See `docs/platform-notes.md` for the load-scale evidence and
  for why the #276 usage merge cannot be the cause.
- **Partial usage reports overwrote instead of merging (gh #276).** A
  provider may report its counters across several stream events —
  Anthropic's `message_start` carries input/cache with output 0, its
  `message_delta` carries output with the prompt counters absent. Every
  accumulator did a whole-struct assignment, so the delta's zeros wiped
  the prompt metrics. The consequence was not cosmetic: prompt tokens read
  0, so `maybe_compact`'s threshold could never fire and a long Anthropic
  session grew until the provider rejected it (FR-SESS-4), while every
  `Record::Assistant` persisted zeros and the footer/`/session`/`/usage`
  under-reported. `Usage::absorb` now merges per field (a later non-zero
  value wins, zero never erases) and the cost block transfers wholesale
  when a later event prices it; the four accumulator sites use it. A
  single-report provider (the OpenAI shape) absorbs to exactly itself, so
  its records are unchanged.
- **Anthropic cache breakpoints grew with history (gh #334).** The
  Messages body planted a `cache_control` breakpoint on the last block of
  **every user message**, so a session with three user turns emitted five
  (system + tools + three messages). Anthropic caps a request at four and
  rejects the rest with `invalid_request_error: At most 4 cache_control
  breakpoints may be specified`. The same arm matched only the user role,
  so a request ending in a tool result got **no** final breakpoint at all
  — the cache tail was silently missing exactly on tool rounds. There are
  now three fixed anchors and never more: the system prompt, the last tool
  declaration, and one post-loop breakpoint on the final block of the last
  serialized message (which covers a tool-result final, since that
  serializes as `role: user`).
- **Anthropic assistant `thinking` blocks serialized after text
  (gh #335).** `content_blocks` merged the text segments and pushed them
  at index 0, then appended the reasoning blocks. Anthropic requires
  `thinking`/`redacted_thinking` to precede every other assistant content
  block, so replaying a thinking turn failed with
  `thinking blocks must precede all other content blocks` — turn 2 of any
  thinking conversation. Reasoning now serializes first (relative order
  preserved, signatures verbatim), then the merged text, then `tool_use`.
- **`[limits] memory_mb` was parsed as bytes, and `[limits]` was ignored
  without `[capabilities]` (gh #287).** Two defects in the manifest's
  resource-hint table. `memory_bytes` was assigned the raw `memory_mb`
  number, so a manifest asking for 64 MiB got a **64-byte** limiter —
  Wasmtime then traps on the guest's first 64 KiB page with `memory
  minimum size of 17 pages exceeds memory limits`, and the extension never
  runs at all. The clamp order was wrong twice over as well:
  `.min(MAX_MEMORY_BYTES)` ran on the un-multiplied value, so a 1000-MiB
  ask clamped to 1000 *bytes*. `memory_bytes` is now
  `memory_mb.saturating_mul(1 MiB).min(MAX_MEMORY_BYTES)` — multiply
  first, clamp the bytes after. Separately, the parse call sat inside the
  `[capabilities]` branch, so a `[limits]`-declaring extension with no
  capabilities table silently ran on the host's defaults; it is parsed
  unconditionally now (`Ok(None)` when the table is absent). The schema's
  `memory_mb` description never stated the unit, which is how the drift
  survived — it now says mebibytes. No shipped extension declares
  `[limits]`, so no released manifest was affected; the trap was reachable
  by any third-party manifest that did.
- **A `tools` capability blocked every install (gh #307).** The
  install-time consent generator enumerated the capability vocabulary
  with a nine-arm match and a wildcard error arm. The `tools` capability
  (ABI 0.6, gh #77) had landed in the WIT, the host's manifest parser,
  and `docs/capabilities.md`, but never got its consent arm — so
  installing ANY extension declaring `[capabilities.tools]` failed
  immediately with `invalid artifact: unknown capability 'tools' in the
  manifest`, before a consent screen could appear. The arm is added with
  the documented sentence ("This lets it list, activate, and call other
  tools in the session."), and the root-cause class — vocabulary drift
  between mirrors — gets a guard: a test reads the capability names from
  `schemas/extension-manifest.schema.json` (the normative manifest source)
  and asserts `consent_lines` handles every one, so the next capability
  added to the schema without an arm fails at the vocabulary's growth
  point. The reverse direction is now gated too:
  `scripts/docs-consistency.sh` fails when a schema capability has no
  `### <name>` section in `docs/capabilities.md`, and the `tools`
  section's quoted consent sentence was corrected to match the code.
- **`session gc` deleted live tool-result images (gh #311).** The mark
  set was built by a walker that matched `user` attachments and a
  `tool-result`'s oversized-text `attachment`, but never
  `ToolResult.images` — the field gh #113 added for images a tool
  returned. A result carrying only images has `attachment: None`, so the
  pattern did not match at all and `lca session gc` **permanently deleted
  live images**, after which turn assembly and export failed on the
  missing files. Silent, destructive, on a documented user command.
  The root cause was drift: the export walker had learned about `images`
  in #113 and the gc walker had not, so there are now **three** callers
  of one shared walker,
  `lca_session::attachments::referenced_attachments` — gc's mark set, the
  export's attachment map, and the export's inlined-image set — with a
  checklist naming each hash-bearing record field and each deliberately
  excluded one. `ToolResult.full_output_path` is excluded because it is a
  path derived from `attachment`, not an independent hash. Verified: a
  tool-result image survives gc, a text spill survives, a user attachment
  survives, a genuinely unreferenced file is still swept (gc is not
  neutered), and export lists exactly the same hash set before and after.
- **Truncated WebP headers panicked the image widget (gh #358).** The
  dimension sniffer proved 16 bytes of `RIFF….WEBP` magic and then read
  the canvas size with direct indexing: `VP8X` and `VP8 ` reached byte 29,
  `VP8L` byte 24. Any 16–29-byte file carrying the magic — a truncated
  download, a corrupt attachment, a text file renamed `.webp` — aborted
  the process with `index out of bounds` inside `image_dimensions`, which
  runs on **every** transcript render and resize (a panic there is fatal
  in the release profile, not recoverable). All three arms now read
  through checked slices, so a short header reports no dimensions and the
  renderer draws its placeholder. Sibling audit: the PNG and GIF arms were
  already safe, and the JPEG segment walker is **provably** bounded — its
  `while i + 9 < bytes.len()` guard means the largest index the body
  touches (`i + 8`) satisfies `i + 8 <= len - 2`, so no input escapes
  (verified over ~303k probes and 8.3M loop-body entries, margin ≥ 1);
  it was left unchanged. Guards: the full 0..=30 truncation matrix per
  chunk kind, positive dimension rows per chunk kind, two `proptest`
  properties (no byte buffer panics, no magic-prefixed buffer panics), and
  a real-terminal row attaching a truncated `.webp`.
- **`@file` truncation panicked on multi-byte UTF-8 (gh #263).**
  `expand_at_files` capped inlined files with a plain
  `String::truncate(INPUT_MAX_BYTES)`, which panics when the 1 MiB index
  lands inside a multi-byte character. A CJK-, emoji-, or
  accent-heavy file of just over 1 MiB therefore crashed the CLI with
  `byte index 1048576 is not a char boundary` before any model call
  (exit 101 — a panic in a production path). The cut now lands on the
  nearest character boundary at or below the bound via
  `str::floor_char_boundary` (stable on the pinned 1.98.1, no new
  dependency), so the straddling character is dropped whole and the
  truncation marker still appends. Same-class sibling audit: the piped
  stdin cap truncates a `Vec<u8>` (cannot panic),
  `lca-ui/src/render.rs` truncates a `Vec<String>` (cannot panic),
  `lca-tools`' `read_up_to` truncates a byte buffer, the TUI editor's
  cuts come from `char_indices`, the markdown fence trim subtracts a
  `trim_start` byte length, and antigravity's family trim subtracts an
  ASCII suffix length — all provably on char boundaries, none changed.
- **Dependency audit: three `zip` extraction advisories, manager-approved
  ignore (gh #349 follow-on).** A fresh advisory-database pull (2026-10-09/10)
  now flags `zip`'s extraction path: RUSTSEC-2026-0367 (SUID/SGID bits
  preserved on extract), 0368 (`ZipArchive::extract` symlinks escaping the
  destination), and 0369 (Unix permissions applied through those
  symlinks). All three fire only through `ZipStreamReader::extract()` /
  `ZipArchive::extract()`, which LCA never calls — `read_archive` does
  per-entry reads behind its own `valid_archive_path` traversal guard, and
  the writes use `ZipWriter`. Patch is `zip >= 9.0.0` with no 2.x
  backport; bumping to 9 warps in `nix`/`typed-path`/`zopfli` for an
  unreachable code path, so the three IDs join `deny.toml`'s ignore list
  with the dead-code evidence. The `zip` 2.x line is also being
  yanked/un-yanked upstream (the index toggled repeatedly on
  2026-10-16/17), so `zip` carries a cargo-deny yank-ignore in the same
  array, making the yank check deterministic without moving the pin. No
  behavior change; the `version = "2"` manifest line is unchanged.
- **Antigravity model listing on real payloads (gh #236).** The
claimed 10M-fuel exhaustion did not reproduce on the fresh
component (measured floor: ~1.5M for a 12 KiB / 32-model catalog,
~6M for 49 KiB / 128 models); the production budget moves to a
measured 25M anyway for catalog-growth headroom, shared as one
const across both host sites and the manifest default. A trapped
listing now warns loudly with the extension named instead of
dissolving into `offers no models`, and provider switching falls
back to the new provider's first model when neither the current
nor the default id matches. Guards: a realistic-catalog listing
through the shipped component, a half-budget margin probe at 4x
catalog size, and the refreshed component fixture.
- **Overlay SGR hygiene (gh #238).** One `splice_segment` seam
now serves the jump indicator, side panel, tooltip, and drawer
glyph: each layer opens and closes its own SGR state, so
transcript italics and background bands neither tint overlays nor
leak past them (the right slice re-arms the base state as FR-UI-23
established).
- **C-c clears the composer (gh #225).** The first C-c with text
present clears it (standing down an armed exit); empty-armed C-c
exits on the second tap, as the hint always promised.
- **Hover no longer scrolls pickers (gh #230).** The rolling window
holds while the selection sits inside it, so hover highlights in
place; wheel and keys rescroll by leaving it.
- **Drawer tab pinned, panel tinted (gh #237).** The drawer tab
sits on the transcript window's bottom row (left of the scrollbar
closed, panel edge open) with a `│` border and `sidePanel` tint
covering the transcript behind it.
- **Friendly settings labels (gh #235).** The selector paints human
labels with the highlighted row's help in the pinned hint, sections
breathe, and `ui.tree_filter_mode` joins the inventory.
- **Session migration (gh #98).** `lca migrate <id>`
backfills linkage on pre-linkage logs with backup and lossless
verification (views identical or refused); already-current logs
report and write nothing; mismatches and truncated logs refuse
with what to fix. Unknown-future lines ride through verbatim.
- **Session tabs (gh #209).** Ctrl+T opens a tab, Ctrl+W closes
(empty composer; with text it still deletes a word), Ctrl+Tab cycles,
Alt+1-9 jump, click selects/closes/adds. Each tab keeps its session,
composer draft, and scroll; transcripts replay from the log on
switch (scrollback appends a transition divider, fullscreen swaps
in place). Single tab collapses to `[+]`. Ctrl+T belonged to
thinking-expand, which moves to Alt+T. Mid-turn tab moves refuse;
background turns stay a follow-up.
- **Session extras (gh #75).** Resume rows carry bookmark names
and the filter matches them; `/name` aliases `/rename`; `lca delete`
and `/resume` Ctrl+D (with inline confirm, never the live session)
trash recoverably. `--no-session` stays the cheap volatile answer
(true in-memory needs a RAM backend: deferred, not built).
- **Lazy sessions (gh #122).** Fresh sessions leave no directory
until the first record (TUI, headless, and tabs alike); pending
titles, renames, and model choices apply at materialize; reads stay
empty and closes stay silent until then.
- **Custom tree rows (gh #137).** Extension `custom`/`custom-message`
records join the entry tree as their own kinds; assembly already
injects only the live chain, pinned by a guard.
- **DAG tree navigator (gh #231).** `/tree` is a graphical
navigator now: branch connectors (`├─`/`└─`/`│`), role markers,
Up/Down walk, Left/Right fold, `f` filter cycling
(`default`/`no-tools`/`user-only`/`labeled-only`/`all`, opening on
`ui.tree_filter_mode`), Enter branches in place, `e` edits the
bookmark through the host. Tool traffic joins the entry tree as its
own kind (hidden by default).
- **Slash outputs ride the transcript (gh #234).** Informational
multi-line outputs (`/help`, `/hotkeys`, `/changelog`, a bookmark
list) append as scrollable transcript entries pi's way; the dock
stays a 1-2 line anchor for short confirmations and errors.
- **Async catalog discovery (gh #232).** `/model` opens instantly on
the #233 snapshot (or a loading spinner when unlisted) and a
background thread fills the picker in place; an empty discovery
still falls through to endpoint consent (#31). Async refresh runs
only on a cache miss; login/switch/grant/reload/scope events cancel
a pending arrival.
- **Slash dispatch freeze (gh #233).** The model catalog enumerates
lazily on bare `/model` only and snapshots behind it; every other
command dispatches without touching providers. Login, switch,
grant, reload, consent-delivery, and scope events invalidate the
snapshot. Async discovery stays a follow-up (#232).

- **Sessions phase 3 (gh #37, epic complete).** `/tree` browses the
live session's entry tree (oldest-first rows, depth indent, label
marks, live ticks) on the rolling-window picker chrome, and Enter
branches there in place with the chain replayed (session switches
stay on `/resume`, forking on `/fork`). `/rename <name>` trails a
`session-info` entry through the shared rename path (`lca rename`,
`--name`, and clone titling ride it); `/resume` keeps showing the
meta title. No migration is built (note below).
- **Sessions phase 2 (gh #37).** In-file branching on top of forks
(ADR-0046): every record carries an additive optional `parent`
(old lines load parentless under the unknown-fields rule);
`branch-point` jumps continue the same log with no new session
directory; `branch-summary` records the abandoned tip plus lesson
and assembles as context. Display reads, the transcript, and model
context follow the tip ancestry (log-order fallback where links are
absent); audit, gc reachability (minus compacted-away records),
labels, and exports see the whole file. Forking at a branched
record inherits its chain.
- **Sessions phase 1 (gh #37, #80-part).** ADR-0046 keeps fork
directories as the branch container and layers tree semantics on
top: `/label [n] <name>` bookmarks a message, `/labels` lists every
bookmark, `/jump <name>` branches at the mark and switches (all on
record ids; latest label wins, absent clears, export carries them).
Trust flags onto ADR-0039: `-a`/`--approve` trusts the project for
the run, `--no-approve` (`--na`) treats it as untrusted, refusing
each other with exit 2; without flags the stored decision stands,
else `trust.default_project` (`ask`/`always`/`never`, default
`ask`, user file only) is the fallback. Session trust now opens the
project file at load, so `/reload` picks up a mid-session trust
without a restart.
- **MCP bridge phase 3 (gh #53, epic complete).** `mcp.json`
(user + trusted-project, pi merge rules) loads through the session
manager: `/mcp` verbs (status, reconnect, enable/disable with
project overrides, exposure set/cycle, background OAuth login,
logout), resources (`list_mcp_resources`,
`list_mcp_resource_templates`, `read_mcp_resource` at the widest
exposure), per-tool exposure (`toolExposure`) with `tool_search`
discovery, and the `## MCP servers` prompt section. Shell
management waits for #171 (0.7).
- **MCP bridge phase 1 (gh #53).** ADR-0045 decides extension-bridge
over host-service: `extensions/mcp` serves external stdio MCP servers
as a `tool-catalog` suite (pi's `mcp__<server>__<tool>` names,
`direct`, annotations carried). Each spawn passes the permission
prompt; a denial is recorded and the server never starts. A
one-tool echo fixture plus a scripted turn prove it end to end.
OAuth/remote/resources/management stay explicitly out (later
phases).
- **MCP bridge phase 2 (gh #53).** Streamable-HTTP transport over
`net` (JSON/SSE envelopes, session ids, transient retry on reads,
per-request timeouts) plus OAuth (dynamic registration, PKCE
loopback flow, credential-namespace token store, refresh, purge,
step-up scopes). Mock remote + mock IdP journeys prove it end to
end. Still out (phase 3): `mcp.json`, `/mcp` + CLI, resources,
exposure levels, system-prompt section.
- **Mouse keystone + scrollbar drag (gh #164).** SGR-1006 normalizes
  to `MouseEvent` (Down/Up/Move/Wheel + modifiers, 1003 hover) with
  a `MouseRegion` hit-test table (no component tree). The scrollbar
  thumb paints solid on hover and drags map pointer Y to scroll;
  selections never start on the bar, and a drag release never copies.
- **Editor mouse (gh #165).** Clicking the composer places the caret
  (wrap/padding/wide-glyph aware) and focuses even with a picker open
  (the picker takes the Escape path). In-buffer drag-select stays a
  named follow-up (it needs an editor selection model).
- **Picker mouse (gh #167).** Hover highlights, press-and-release
  confirms (the checklist toggles), wheels walk the selection, and
  the backdrop dismisses without submitting. Host dialogs and native
  consent modals keep their keys - the backdrop never answers one.
- **Tabs widget (gh #208).** A horizontal bar / vertical rail
  primitive (plain markers + host spans, keyboard + mouse) for future
  session tabs and panel sections. No product surface yet.
- **Transcript clicks (gh #166).** Thinking clicks cycle snippet →
  full → hidden (Ctrl+T shares the cycle); tool-card headers expand
  that card only (bare running cards ignore the click); the jump
  indicator was already clickable and stays pinned.
- **Drawer tab (gh #207).** A one-cell ◀/▶ handle toggles the
  extension panel: margin when closed (only with a registered panel),
  panel edge when open, accent on hover, hovering names it (see below).
- **Hover tooltips (gh #210).** A stationary pointer names drawer,
  steppers, jump, tool cards (Expand/Collapse follows state), and
  thinking runs: borderless blocks, quadrant-flipped inside the frame,
  250 ms to show, gone on any move, press, wheel, key, or resize.
- **Wheel velocity (gh #206).** Pi's acceleration curve in `auto`
  (one line isolated, toward six on a fast spin) with Alt ×5; numeric
  settings stay fixed.
- **Invocation: `@file`, stdin, `--`, `--offline` (gh #71).**
  `@path` positionals inline text (`<file name>` shape, 1 MiB bound)
  or stage images; piped stdin prepends the first prompt; `--` stops
  parsing; redirected streams mean print mode; `--offline`/`LCA_OFFLINE`
  silences automatic network (model traffic still flows).
- **Tool selection flags (gh #67).** `-t/--tools` (allowlist or
  `+`/`-` deltas, `*` globs), `--exclude-tools`, `--no-builtin-tools`,
  `--no-tools`; unknowns warn; `tool_search` pins with the flags.
  `defaultTools` stays a follow-up.
- **Session flags (gh #69).** `--session-id` (exact or created,
  pi charset), `--no-session` (volatile, grants stay real),
  `--name` (titles fresh, renames opened), `--fork` (tip clone,
  `--session-id` chooses its id); pi exclusivity refused up front.
  `--session-dir` stays a follow-up.
- **Resource flags (gh #70).** `-e` files register per-run (same
  consent; dirs contribute skills), `--skill`/`--theme` load per-run
  (CLI precedence; explicit theme dirs first), `--no-extensions`
  (providers still resolve), `--no-skills`, `--no-themes`; bad paths
  refuse. `--prompt-template` stays a follow-up.
- **Startup header + `ui.quiet_startup` + `/changelog` (gh #131).**
  The transcript head names the version plus the loaded resources
  (context paths that exist, extension/skill/template counts).
  `ui.quiet_startup` (`false` | `true` | `"header"`, pi naming)
  hides in layers. `/changelog` shows the embedded changelog's
  latest released section; the update notice points at it.
- **Double-escape action (gh #132).** Esc Esc with an empty editor
  opens the branch tree or the fork picker per
  `ui.double_escape_action` (`tree` | `fork` | `none`, pi default
  `tree`); `ui.tree_filter_mode` parses but stays inert by
  documentation (LCA's tree lists sessions, nothing to filter).
- **Display settings inventory (gh #82).** Nineteen typed keys in
  LCA naming (`ui.*`, `terminal.*`, `images.*`, `markdown.*`) with
  pi's domains; the thirteen that bite apply live through
  `/settings`, six stay inert by documentation (see the key table
  for which and why).
- **Prompt templates (gh #58).** Markdown files become `/`
  commands: user (`~/.lca/prompts`) + project (`.lca/prompts`)
  dirs, frontmatter description + argument-hint in completion,
  `$1`/`$@`/`${1:-default}`/`${@:N:L}` over shell-like args.
  Invoking fills the editor (never auto-submits).
- **`tool.edit_requires_read` toggle (gh #117).** Off is pi parity
  (blind edits allowed, the default); on keeps LCA's read-before-edit
  staleness guard. Documented as a settled divergence (kept,
  default-off).
- **`/fork` picker + in-process switch (gh #203).** Bare `/fork`
  opens the transcript's user messages (latest selected, pi's
  `UserMessageSelector` shape); choosing forks AND switches in
  place with the text restored into the editor. `/fork <n>` skips
  the picker down the same path.
- **`/scoped-models` checklist (gh #204).** Toggle the `Ctrl+P`
  rotation from a checklist modal (pi's `scoped-models-selector`
  shape): space toggles, `a` flips all, enter persists
  `models.enabled` and updates the live rotation, escape discards.
  A set covering every model persists empty (no restriction); an
  empty set refuses.
- **Categorized `/settings` (gh #174).** Section dividers over the
  full scalar inventory (pi's list is flat), with inline editing
  for free-text and numeric rows (enter applies, escape cancels,
  untouched buffers write nothing).
- **Prompt stepper (gh #173).** Jumps pin the prompt to the
  viewport's top row; the scrollbar gains clickable ▲/▼ steppers;
  `Shift+Up/Down` step like `Alt+Up/Down`; Shift+wheel steps in
  fullscreen (plain wheel still scrolls).
- **Bare `-r` picker + `--session` (gh #110).** `lca -r` opens the
  session picker over a fresh session (pi parity); `--session
  <id|path>` resumes direct, resolving session-directory paths too;
  `-r <id>` keeps working. A picker next to a prompt is a usage
  error, as are the contradictory pairs.
- **Token-derived cost (gh #125).** A curated `pricing.toml` (verified
  entries only) fills a zero reported cost at the turn, so `/session`
  and the footer show running cost for listed models; unlisted models
  stay tokens-only, and a reported cost is never overwritten.
- **`/reload` without restart (gh #130).** Re-runs discovery for
  settings, extensions, prompts, themes, and keybindings (the registry
  swaps under a lock turns never disturb); skills and context were
  already live per turn. Does not reload: the provider/model
  generation, credentials, the shell backend, markdown transformers,
  and applied interface chrome.
- **`shell.command_prefix` (gh #133).** A prefix prepended to every
  shell command (pi's `shellCommandPrefix`), joined with a newline so
  `export`/`source` lines take effect; applies to both transports.
- **Typed `ui.fullscreen` (gh #112).** The key joins the config
  schema (TOML honored, malformed loud); `ui.json` parses typed (an
  unrelated `true` no longer enables) with precedence persisted >
  config > scrollback, and malformed persistence falls back with a
  transcript-head warning.
- **`builtin:` disable syntax (gh #139).** `lca ext disable
  builtin:compaction-default` writes the same per-project flag the
  loader reads for the plain name (pi's spelling); no second record.
- **Thinking budgets, signatures, picker hiding (gh #41).**
  `[thinking.budgets]` tunes per-level token budgets (pi's
  built-ins) onto the Anthropic wire as `thinking.enabled`; the
  assistant record persists `reasoning_signature` +
  `provider_thinking_level` and replay resends signatures verbatim
  (redacted blocks keep theirs); the `/thinking` picker hides
  levels the model does not offer. Deferred responses stay a
  documented deferral (no producer yet).
- **Anthropic inline-tools beta kit (gh #201).** `frozen_tools` +
  `__pi_deferred_placeholder__`, `tool_addition`/`tool_removal`
  system blocks, and the `inline-tools-2026-09-15` beta constant
  land in `lca-wire-anthropic`; the header and turn state ride
  with `extensions/anthropic` (#183).
- **Anthropic provider extension (gh #183).** `extensions/anthropic`:
  Messages on the shared wire kit (cache breakpoints, P5A thinking
  signatures/budgets), API key + Claude Pro/Max OAuth (browser
  loopback + copy-code paste, JSON exchange), a four-model static
  table. The host's callback parser accepts the `code#state` paste.
- **OpenRouter provider extension (gh #185).** `extensions/openrouter`:
  one-click OAuth PKCE provisioning a permanent key, chat completions
  on the shared OpenAI kit, live `/models` discovery with a curated
  floor. `openai-compatible` stays strictly key-only (guarded).
- **GitHub Copilot provider extension (gh #184).** `extensions/github-copilot`:
  device-code flow, Copilot token mint, `proxy-ep` routing, Copilot
  headers, per-account model catalog. The host shows device codes
  beside the page (a `#code=` fragment convention — freeze-safe, no
  new host import) and skips the paste prompt while polling.
- **Meta provider extension (gh #186).** `extensions/meta`: RFC 8628
  device flow splitting identity from the minted day-key (refresh is
  the same mint; 401/403 purges), chat completions, two-model table.
- **Kimi Code provider extension (gh #187).** `extensions/kimi-coding`:
  device flow to the token triple, refresh retrying 429/5xx, chat
  completions, two-model table.
- **llama-server provider extension (gh #62).** `extensions/llama`:
  loopback router over `net-local` (chat completions + catalog with
  load states and windows), one-shot `/llama list|load|unload`
  commands (native delivery; the sandboxed command world has no net
  and declines in text). No curated floor for your own GGUFs.
- **Retry inventory (gh #83).** `provider.retry_base_delay_ms`
  tunes the backoff base (pi's `retry.baseDelayMs`); `retry_limit = 0`
  is pi's `retry.enabled = false`. No transport switch, no max-delay
  cap, no per-request timeout — documented divergences. The proxy
  stays env-only by decision (no `net.proxy_url` key: credentials
  must not land in files).
- **Crash reports + `lca doctor` (gh #81).** The panic hook writes
  `~/.lca/crash-*.log` (version, loaded extensions, message,
  faulting frames; newest ten kept) and `lca doctor` dumps version,
  paths, extensions, config, sessions, and crashes.
- **Project-local extensions (gh #138).** `.lca/extensions/<name>/`
  (`extension.toml` + `component.wasm`) loads when the project is
  trusted, grants prompting as usual; untrusted projects ignore it.
  Most-specific scope wins over installed copies.
- **Session environment for shell children (gh #129).** A turn's
  shell sees `LCA_SESSION_ID`/`LCA_SESSION_DIR`/`LCA_PROVIDER`/
  `LCA_MODEL`/`LCA_THINKING`/`LCA_DATA_DIR` (pi's `PI_*` row under
  `LCA_*` names); identifiers and paths only, never secrets.
- **Proxy variables honored (gh #145).** The `net` host client
  routes through `HTTP_PROXY`/`HTTPS_PROXY` (`ALL_PROXY` fallback,
  `NO_PROXY` bypass, lowercase honored): absolute form for http,
  `CONNECT` tunnels for https (pi's `proxyTunnel` shape). Grant and
  rebinding checks still run first on direct DNS; loopback needs an
  explicit `NO_PROXY` entry.

### Fixed
- **Session temp without the global (gh #160).** `SESSION_TEMP`
  OnceLock is gone: the temp dir resolves per session from the
  session id (creation failures propagate), process-long contexts
  use the system temp dir explicitly.
- **Tool calls record their real source (gh #128).**
  Extension-handled calls log `ToolSource::Extension` (plain
  variant, not `Extension { id }`: no log-shape change); built-ins
  stay `Builtin`.

### Added
- **Instant `/clone [name]` (gh #205).** Duplicates the live session
  at its tip and switches in one step (no picker): `clone_session`
  forks at the latest record and titles (`Clone of <parent>` by
  default, inheriting model/provider meta), the transcript replays
  through the switch path, and `lca clone <session> [title]` does the
  same headless.
- **Ctrl+X copies the OAuth sign-in URL (gh #200).** The message-copy
  key works on the waiting login screen (a wait is not a question),
  naming the exact sign-in URL out of the OSC 8 wrapped label through
  the copy ladder (native clipboard, else OSC 52 reported unverified);
  a verified native copy confirms `✓ Copied sign-in URL to
  clipboard`, and the waiting screen names the key. The `[Enter]
  opens browser` hint is deliberately absent: LCA has no such binding.

### Fixed
- **One-shot version string (gh #154).** `version_static` computes at
  most once behind a `OnceLock`; repeated calls share one address.
- **Shell description whitespace (gh #162).** The `shell` tool's
  sentence gaps collapse to single spaces via line continuations.
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
