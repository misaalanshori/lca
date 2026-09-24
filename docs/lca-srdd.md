# Lightweight coding agent with WASM extensibility

Software requirements and design document, revision 0.1.

The project name in this document is LCA. The binary is `lca` and the crates use the `lca-` prefix.

## Companion documents

This document sets the requirements and the architecture. Eighteen decisions that support it are written up separately as architecture decision records under `docs/adr/`, numbered 0001 through 0018, covering runtime selection, crate decomposition, the widget tree, the provider stream shape, filesystem scopes, the permission store split, the decision against an out-of-process runner, extension composition, the extension update path, distribution beyond OCI, local network access, provider identity operations, the three kinds of pluggability, the async execution model, compaction and context transform, the pty capability, prompt cache preservation and measurement, and web-embedded extension hosting. Where this document and an ADR could drift, the ADR is the more current statement of the reasoning; this document is the more current statement of the requirement itself.

Thirteen further documents fill in detail this one only summarizes: the capability catalog at `docs/capabilities.md`, the extension authoring guide at `docs/extension-authoring.md`, the ABI versioning policy at `docs/abi-versioning.md`, the session log format at `docs/session-log-format.md`, the runtime flows as diagrams at `docs/flows.md`, the threat model at `docs/threat-model.md`, the release and versioning policy at `docs/release-policy.md`, the software testing plan at `docs/testing-plan.md`, per-platform implementation notes at `docs/platform-notes.md`, the configuration key reference at `docs/configuration.md`, the headless and scripting contract at `docs/headless.md`, a glossary of project-specific terminology at `docs/glossary.md`, and the provenance of what this design takes from Pi and fx at `docs/inspiration.md`. First-party provider extensions are documented individually under `docs/providers/`. The extension manifest schema is at `schemas/extension-manifest.schema.json` and is the normative validation source; the manifest examples in this document are illustrative.

## What this is

LCA is a terminal coding agent. It reads a prompt, calls a language model, runs tools against the local file system and shell, and shows the result in a terminal user interface. It ships as one static executable per platform. It needs no Node.js, no Python, and no system package manager at runtime.

The core stays small on purpose. The core owns the session loop, the built-in file and shell tools, the terminal renderer, the permission system, and the extension host. Everything else is an extension: model providers, slash commands, lifecycle hooks, custom renderers, compaction, context transformation, and workflow logic. Not everything that varies is an extension in this sense, though; see ADR-0013 for the distinction between something the project swaps at build time and something a user installs and grants capabilities to at runtime. The agent runs with zero providers enabled as a valid, ordinary state, not an error condition; it ships with one provider enabled by default so a fresh install has something to talk to, and nothing stops a user from disabling it.

Extensions are WebAssembly components. They are not dynamic libraries and they are not npm packages. An extension is a `.wasm` file plus a manifest. The manifest declares which interfaces the extension implements and which capabilities it needs. The host grants capabilities at instantiation time and enforces them at the import boundary. An extension that never gets the network capability cannot reach the network, whatever its code says.

The same extension source can also compile into the main binary as a normal Rust dependency. This gives first-party extensions zero call overhead and gives third-party extensions a sandbox. The choice is a build flag, not a code change.

Two design ideas drive the whole document. First, a small trusted core with a wide extension surface beats a large core with a narrow one. Second, a capability boundary that the host enforces beats a policy that the extension author promises to follow.

## Scope

In scope for the first stable release:

The agent runs on Linux, macOS, and Windows as a native binary. It also builds to WebAssembly for embedding in a browser or a Node host. It supports interactive and non-interactive use. It stores sessions on disk and can resume them. It ships built-in tools for reading, writing, editing, listing, searching, and running shell commands. It hosts WASM extensions and enforces capabilities. It fetches extensions from an OCI registry or a plain HTTPS-hosted archive, both with a built-in client, and from a local path. It provides a stable, versioned extension ABI defined in WIT. It ships with one provider extension, speaking any OpenAI-compatible endpoint, enabled by default and built into the binary; every other provider, including ones for a local model server or a subscription login, is installed like any other extension. The agent can run with none enabled.

Out of scope for the first stable release:

LCA does not ship a hosted registry, a web dashboard, a cloud account system, or a paid service. It does not implement a plugin marketplace with ratings and reviews. It does not bundle a language server, a debugger, or an editor. It does not run extensions written in JavaScript through an embedded JS engine. It does not provide multi-user collaboration or shared sessions. It does not do automatic model routing across providers.

Deliberately excluded from the core, because extensions cover them:

Sub-agent orchestration, to-do tracking, plan mode, background task supervision, git workflow automation, notification delivery, skills handling, and project-specific prompt libraries. The core provides the hooks and worlds that make these possible. It does not implement them.

## Capabilities

The agent has an interactive terminal mode and a headless mode. Interactive mode shows a scrollback of messages, a streaming response area, a status line, and an input editor with history and completion. Headless mode takes a prompt, runs to completion, and writes structured output to standard output for use in scripts and CI.

Session state is durable. Every turn appends to a session log on disk. The user can resume a session, fork it at any message, rename it, and export it. Compaction runs when context use crosses a threshold; the strategy that decides what to keep is a `compaction` extension, and the mechanism that reshapes what actually goes out on the wire on every turn, for redaction or context injection, is a separate `context-transform` extension. Neither touches the stored log the same way: compaction's result is written back as a durable record, a transform's is not. See ADR-0015.

Built-in tools cover the common file and shell operations. The read tool returns file content with line numbers and supports offsets. The edit tool does exact string replacement and rejects a write when the file changed since the last read. The write tool creates or replaces a file. The list, glob, and grep tools search the workspace. The shell tool runs a command in the platform shell with a timeout and streamed output.

Model access comes from provider extensions. The core knows nothing about any specific vendor. It knows the provider interface: list models, start a completion, stream events, handle authentication, and report login, logout, and usage in a common shape so the host can offer a generic picker and an aliased `/usage` alongside each provider's own namespaced commands. A provider extension can run an OAuth flow through a host capability, so subscription logins work without giving the extension raw socket access.

The permission system sits between every sensitive action and the thing that performs it. Shell commands, writes outside the workspace, network calls from extensions, and credential reads all pass through it. The user can pre-approve patterns, approve once, or deny. Approval state is per project and persists.

Extension management happens inside the agent. The user can list installed extensions, install one from an OCI reference, a plain HTTPS archive, or a local file, inspect what it asks for, enable or disable it per project, and remove it.

## Architecture

### Process model

LCA is a single process. There is no daemon, no server, and no helper binary. Asynchronous work runs on a Tokio runtime inside that process. Shell commands run as child processes. WASM extensions run inside the embedded runtime in the same process, isolated by the WASM sandbox rather than by the operating system.

This keeps startup fast and keeps deployment to one file. The trade is that a WASM sandbox escape is a host compromise. The security section covers how the design limits that risk.

### The three kinds of pluggability

Not everything that varies in this design varies the same way, and conflating the three is where confusion creeps in. Fixed core has one implementation and no extension point at all: the agent loop's shape, the session log's record framing, the permission enforcement path. A build-time backend has more than one implementation, chosen when the binary or web bundle is produced, with no manifest and no consent screen, because the project itself supplies every option. The read, write, and shell tools are the clearest case: no capability gates them, because they are what defines the model's access to the workspace rather than something requesting it, but their implementation differs behind a Rust trait, a native backend for desktop and a host-delegated backend for the web target that defers to whatever the embedding JavaScript application supplies, per FR-WEB-3. A runtime extension is installed by the user, potentially from a party the project does not control, gated by the capability model, and shown a consent screen naming exactly what it can reach. Providers, skills handling, compaction, and context transforms all live here, whether or not a particular one ships enabled by default; shipping enabled by default is a packaging decision, not a different category. See ADR-0013 for the full reasoning and a table classifying every major feature in the system this way.

### Crate decomposition

The repository is one Cargo workspace. Each crate has one job and depends downward only. A crate never depends on `lca-cli`, and only `lca-cli` depends on everything.

`lca-config` reads and merges configuration. It has no other dependencies inside the workspace.

`lca-protocol` holds the shared data types: messages, tool calls, tool results, stream events, session records. Every other crate speaks these types. It has no I/O.

`lca-session` owns session storage, the append-only log, forking, resume, export, and the durable compaction records. It depends on `lca-protocol`.

`lca-tools` implements the built-in tools. It depends on `lca-protocol` and on the permission interface from `lca-permissions`.

`lca-permissions` implements the capability grant model, the approval prompts interface, and the per-project approval store.

`lca-provider` defines the provider trait that the core calls, plus the streaming types. It does not implement any vendor.

`lca-ext-abi` holds the `.wit` files and the generated host and guest bindings. It is the contract crate. It is published separately so extension authors can depend on it without pulling the agent.

`lca-ext-host` embeds the WASM runtime, instantiates components, wires host imports, enforces capabilities, and applies resource limits.

`lca-ext-native` registers extensions that are compiled into the binary. It exposes the same handle type as `lca-ext-host`.

First-party extensions, including the default OpenAI-compatible provider and skills handling, are not separate crates under `crates/`; their source lives under `extensions/`, the same tree third-party extension authors work in, built against the public ABI like anything else there. `lca-cli` exposes each one behind its own Cargo feature, on by default for the ones meant to ship enabled and off by default for the ones, such as Antigravity and Codex, that ship only as installable WASM artifacts. A feature being off by default is a packaging choice; the same source still compiles to `wasm32-wasip2` for anyone who wants to build a custom binary with it included, or install it separately the way any third-party extension is installed. See ADR-0013.

`lca-registry` is the OCI client. It resolves references, fetches manifests and blobs over HTTPS, verifies digests, and owns the extension lockfile that records each installed extension's resolved digest, source reference, and approved capability hash. It also resolves the plain-HTTPS-archive source kind described in ADR-0010, sharing the same lockfile and digest-verification path as the OCI resolver.

`lca-tui` is the terminal renderer, the widget model, the input editor, and the event loop.

`lca-core` holds the agent loop, the dispatch table, and the compaction and context-transform dispatch. It sits above the domain and platform crates, and only `lca-sdk` and `lca-cli` depend on it.

`lca-sdk` is the embedding API for host applications, native and WASM.

`lca-testkit` holds the fake provider, the test harness, and shared fixtures. It is a dev dependency everywhere.

`lca-cli` is the binary. It wires everything, parses arguments, and owns `main`.

`xtask` is the build automation crate. It runs cross-compilation, size checks, WIT validation, and release packaging.

### Agent loop

The loop is small enough to describe in a paragraph. The user submits input. The session appends a user message. The core asks the active provider to stream a completion. Stream events arrive and update the render state. When the model asks for a tool call, the core resolves the tool by name, checks permissions, runs it, appends the result, and continues the turn. When the model stops without a tool call, the turn ends. Hooks fire at defined points around each step.

Hook points are `pre-turn`, `pre-tool-use`, `post-tool-use`, `post-turn-end`, `attention-required`, and `session-close`: before a turn starts, before a tool call runs, after a tool call returns, after a turn ends, when the agent needs user attention, and when the session is about to close. Each hook can observe, and the pre-tool hook can also deny or rewrite a call; a rewritten call passes through the permission layer like any other call and is not fed back through the hooks.

### Extension model

An extension implements one or more WIT worlds. The worlds are `provider`, `tool`, `command`, `hooks`, `ui`, `compaction`, and `context-transform`. A single extension may implement several. A provider extension that also adds a slash command and a tool is normal; a compaction extension typically also implements `command`, for a manual trigger, which needs no special mechanism since worlds already compose freely.

Both delivery modes produce the same runtime behavior through one dispatch interface. The core holds a list of extension handles. A handle is either a native handle that calls a Rust trait object directly, or a WASM handle that calls into a component instance through generated bindings. Call sites in the core do not branch on the mode.

Tool and command names each form one namespace. Built-in names are reserved. Where an extension collides with a built-in name or with a name an already-enabled extension registered, the earlier or built-in registration wins, the later one is disabled for the session, and the collision is reported (FR-EXT-11).

Native mode is for first-party extensions that ship with the binary. It has no marshaling cost and no sandbox. Because it has no sandbox, the agent labels native extensions as unsandboxed in the extension list, and the manifest capability display says so plainly. A native extension still calls only through the WIT-defined interface. This rule is a code review rule, not a compiler rule, and it keeps the two modes interchangeable.

WASM mode is for everything else. The host instantiates the component with a store that carries the granted capability set, a fuel budget, and a memory limit. Every host import checks the grant before it acts.

### Runtime selection

The native build uses Wasmtime with the Cranelift backend. Cranelift compiles components ahead of time or just in time and gives near-native speed on x86-64 and aarch64.

Wasmtime also ships Pulley, a portable bytecode interpreter for platforms with no Cranelift backend and for environments that forbid executable memory. Pulley is the fallback for any target where a JIT cannot run.

WASM extension calls run on the same Tokio runtime as everything else in the process, through Wasmtime's async host function support, rather than each needing a dedicated OS thread. Two distinct Wasmtime mechanisms serve two distinct needs and are kept separate: fuel is a resource budget, configured per extension from the manifest, and epoch interruption is a cancellation mechanism, used to force-preempt a running instance the moment the user cancels a turn, independent of how much of its fuel budget remains. See ADR-0014 for the full reasoning and the concurrency and cancellation section below.

The web build does not nest one WASM engine inside another. A JavaScript host already has a fast engine. The build transpiles the agent component and each extension component into ES modules with jco, and the JS orchestration layer wires the agent's imports to the extensions' exports. Both modules run as peers on the host engine. Pulley is not needed for the browser case.

### Capability model

Capabilities are named grants attached to an extension instance. The first release defines these:

`net` grants outbound HTTPS to a list of host patterns, each of which may pin a non-default port. The extension calls a host import, not a socket. The host checks the target against the granted patterns, resolves the hostname, and refuses to connect if the resolved address falls in a loopback, private-use, link-local, unique-local, or carrier-grade NAT range even when the pattern matched, to prevent DNS rebinding from reaching a local address under cover of an ordinary-looking public hostname; the canonical range list is in `docs/capabilities.md`. See ADR-0011. A manifest cannot declare a bare wildcard covering every host; a provider whose actual host isn't known until the user configures it, such as one speaking to an arbitrary OpenAI-compatible endpoint, gets that specific host added as an ad hoc grant at the point of configuration instead, the same mechanism `fs` uses beyond its own fixed vocabulary. See ADR-0005 and the capability catalog.

`net-local` grants HTTP or HTTPS, any port, to `localhost` or a loopback address, a private-use, link-local, unique-local, or carrier-grade NAT (tailnet) range, or an mDNS `.local` hostname. Separate from `net` because the risk shape, the port and scheme rules, and the pattern syntax all differ, and because it reaches other devices on the user's network, not only the machine the agent runs on. See ADR-0011 and the capability catalog.

`fs` grants read or write access to one or more named scopes: `workspace`, `private`, `home-config`, or `temp`. The manifest names a scope and a mode; it never carries a path. The host resolves each name to a real path and passes a preopened directory handle, and it refuses any resolution that leaves the scope, including through a parent traversal or a symbolic link. The host also refuses any resolution that enters the agent's own state directory, where sessions, the extension tree, and the credential store live, under every scope and every ad hoc grant; this is what keeps credential isolation true even on platforms whose configuration directory also holds application data. A user can attach an additional ad hoc path grant outside the vocabulary at install time or later; the manifest cannot request one. See the capability catalog and ADR-0005 for the full scope table and the reasoning against a two-scope design.

`oauth` grants the loopback authorization flow. The extension asks the host to start a listener and gets back a redirect URL. The host runs the listener, receives the callback, and returns the parsed parameters. The extension never binds a port.

`credentials` grants read and write access to one namespace in the credential store. The namespace is the extension's own identity. There is no cross-namespace read.

`ui` grants the extension the right to register widgets in defined regions: the status line, the footer, a side panel, and a modal.

`process` grants the right to ask the host to run a shell command. Commands from an extension pass through the same permission prompt as commands from the model.

`pty` grants allocation of a pseudo-terminal for an interactively spawned program, distinct from `process`'s plain pipe-backed streams. This is what makes something tmux-shaped buildable as a sandboxed extension rather than forcing the native-linked path: the PTY allocation itself happens host-side, behind the import. See ADR-0016.

`completion` grants the right to ask the host for a response from whichever provider is currently active, rather than the extension calling another extension directly; there is no extension-to-extension call path in this design, per ADR-0008. Added to the 1.0 set by ADR-0015, once the default compaction strategy's need to summarize well gave the deliberately deferred service-access case a real consumer.

Anything not granted is absent from the import table. A component that calls an ungranted import gets a trap or an explicit permission error, depending on whether the import exists in a denied state or does not exist at all.

### Concurrency and cancellation

One Tokio runtime for the whole process drives the provider stream, shell and subprocess execution through `tokio::process`, and every WASM extension call. Tool calls within a single turn run sequentially for 1.0; this is a deliberate simplification stated as a requirement rather than an incidental behavior, since running independent tool calls concurrently would need a defined execution order for their results and a way to present interleaved output that nothing currently requires.

Cancellation, triggered by the user pressing the cancel key mid-turn, uses epoch interruption specifically, not fuel exhaustion: the host increments the epoch and every running instance traps at its next yield point regardless of its remaining budget. The in-flight provider stream task, any running extension call, and any running shell command are all stopped concurrently rather than in sequence, and whatever session records are already complete are kept. See ADR-0014 and the cancellation flow in `docs/flows.md`.

### Session and storage layout

Sessions live under the user data directory, grouped by project path hash. Each session is a directory with a metadata file and an append-only log of records. A record is one of: user message, assistant message, tool call, tool result, compaction marker, or fork point.

Append-only storage makes fork and resume cheap, and makes corruption recoverable. The agent never rewrites a record in place. Compaction writes a marker and a summary record, and leaves the original records on disk. The full record schema, the framing rules, and the recovery behavior for a truncated file are specified in `docs/session-log-format.md`.

Installed extensions live under the same user data directory, in a separate tree keyed by extension name. Each entry holds the component bytes named by their content digest, the parsed manifest, and the approved capability set. A single lockfile at the top of that tree records, per extension, the resolved digest, the source reference it was resolved from, and the hash of the approved capability set that `lca ext update` compares against before it prompts. This is the state `lca-registry` reads and writes; it is distinct from the session log and from the permission store described below. The hash covers the manifest-declared grant set only; ad hoc grants live in the user grant store and persist across updates.

### Terminal rendering

The TUI does not let extensions write escape sequences. An extension returns a widget tree built from a small vocabulary: text spans with semantic color roles, images, boxes, rows, columns, a spinner, a progress bar, and a key-value list. The host lays out and draws the tree. An image widget carries a media type and image bytes; the host persists anything large as a session attachment and records the reference.

This costs flexibility. It buys two things. Extensions cannot inject escape sequences to spoof output or hide text. The renderer can change without breaking extensions.

## Interfaces

### Extension ABI

The ABI is a WIT package, `lca:ext`, versioned with semver. The package is published as an OCI artifact and as a crate. Below is the shape of each world. The final `.wit` files live in `wit/` and are the normative source. Every record type that crosses the boundary, messages, usage, tool calls, and tool results, carries a reserved `extras` map of string pairs, so non-structural data can be added without breaking the ABI.

The `tool` world exports a schema function and an execute function. Schema returns a name, a description, and a JSON schema for the parameters. Execute takes a call and returns a result that carries text, structured content, or an error.

The `command` world exports a spec function and an invoke function. Spec returns the command name, the argument hint, and the completion behavior. Invoke takes the argument string and returns an effect: insert text into the input, submit a prompt, show a widget, or do nothing.

The `hooks` world exports one function per hook point. The pre-tool hook returns an action: allow, deny with a reason, or replace the call.

The `provider` world exports model listing, a completion call that returns a stream resource, the authentication functions, and `login`, `logout`, and `usage`, each always exported and each returning a defined not-supported result when a provider has none. Promoting these three to world-level exports, rather than leaving them as ad hoc commands each author names differently, is what lets the host build a generic `/login` picker across every installed provider and a `/usage` that follows whichever one is active, alongside an automatically namespaced per-provider form such as `/antigravity.usage`. See ADR-0012. The completion call carries, alongside the message list, a count of leading messages the host considers the stable, cacheable prefix, computed from the most recent compaction record; a provider extension uses this to place a vendor-specific cache marker where one exists, and ignores it safely where one doesn't. See ADR-0017. Streaming uses a resource with a read function that returns the next event or end of stream; the host drives it from an async task without dedicating an operating system thread to a call, and the Phase 0 spike validates the polling shape. The `usage` stream event carries `cache_read`, `cache_write`, and `cache_write_1h` token counts alongside the ordinary input and output counts, and the turn's cost, since cache accounting is what makes the cache behavior testing in `docs/testing-plan.md` possible at all.

The `ui` world exports a render function that returns a widget tree for a named region, and an event handler that takes a user interaction and returns an effect.

The `compaction` world exports a single function that takes a candidate range of session records and returns a summary. The host calls it when configured usage crosses a threshold or the user runs a manual compact command, and writes the result as a durable record in the session log; every later read reuses it without recomputing anything. This is also where the cache boundary described above resets. See ADR-0015 and ADR-0017.

The `context-transform` world exports a single function that takes the resolved message list about to be sent to the model and returns either a transformed list or a rejection. The host chains every enabled transform extension in order on every turn; nothing it returns is written to the session log. A rejection ends the turn with that reason surfaced, the same shape a hook denial uses. Where a turn's post-transform content inside the stable cache boundary differs from what was sent to the provider on the previous turn, the host narrows the boundary it reports for that turn to end before the earliest differing message, records the divergence as an extension event, and does not reject the turn; a transform whose output has settled narrows nothing further. See ADR-0015 and ADR-0017.

Host imports are grouped by capability and match the capability names above: `lca:host/net`, `lca:host/net-local`, `lca:host/fs`, `lca:host/oauth`, `lca:host/credentials`, `lca:host/ui`, `lca:host/process`, `lca:host/pty`, `lca:host/completion`, plus `lca:host/log` which is always granted.

### Extension manifest

Every extension ships a manifest next to the component. The format is TOML. It declares identity, the ABI version it targets, the worlds it implements, and the capabilities it needs with their parameters.

```toml
name = "example-provider"
version = "0.3.1"
abi = "1.0"
worlds = ["provider", "command"]
description = "Model provider for Example Cloud."

[capabilities.net]
hosts = ["api.example.com", "*.example-cdn.com"]

[capabilities.oauth]
redirect_path = "/callback"

[capabilities.credentials]
namespace = "example-provider"
```

The manifest is the consent surface. The install flow shows exactly these grants before it writes anything to disk.

A local provider's manifest looks different in a telling way: no `credentials`, no `oauth`, just the one capability it actually needs.

```toml
name = "lmstudio"
version = "1.0.0"
abi = "1.0"
worlds = ["provider", "command"]
description = "Connects to a local LM Studio server."

[capabilities.net-local]
addresses = ["127.0.0.1", "192.168.0.0/16", "*.local"]
```

### Command line

`lca` with no arguments opens the interactive TUI in the current directory. `lca -p "prompt"` runs one turn headless and prints the result. `lca resume` lists sessions and reopens one. `lca fork <session> <message>`, `lca rename <session> <title>`, and `lca export <session> [--audit]` fork at a message, rename, and export a session. `lca ext list`, `lca ext install <ref>`, `lca ext update <name>`, `lca ext remove <name>`, and `lca ext info <name>` manage extensions. `lca ext update --all` updates every installed extension. `lca config` prints the merged configuration and its sources. `lca --version` prints the agent version, the ABI version, the crate version, and the build target.

Headless mode supports `--json` for machine-readable output: one JSON object per line, each with a type field. The envelope shape and the exit-code table are in `docs/headless.md`.

### Configuration

Configuration is TOML. Sources merge in this precedence order, highest first: command line flags, environment variables, the project file at `.lca/config.toml`, the user file in the platform config directory, and built-in defaults. The key reference, with types, defaults, and environment-variable names, is in `docs/configuration.md`.

The project file is trusted only after the user marks the project as trusted. An untrusted project file cannot enable extensions or change permission defaults.

### Embedding SDK

`lca-sdk` exposes a session handle, an event stream, and an input channel. A host application creates a session, subscribes to events, and sends input. The same API compiles for native hosts and for the WASM target.

For JavaScript hosts, the build produces an ES module through jco transpilation. The module exposes the same session and event API in JavaScript, plus a registration function so the JS layer can supply transpiled extension modules.

### User interface

The interactive screen has four regions. The scrollback shows the conversation. The active area shows the streaming response and running tool calls. The status line shows the model, the context use, the session cost, and any extension-provided segments. The input editor sits at the bottom with multi-line support, history, file path completion, and slash command completion. Built-in slash commands are `/login`, `/logout`, `/usage`, `/model`, `/compact`, and `/stats`; an extension's own commands are namespaced under its extension name, and the provider identity commands also appear there automatically.

Keyboard control follows terminal conventions. Enter submits. Shift+Enter inserts a newline. Ctrl+C cancels the running turn and does not exit. A second Ctrl+C on an idle prompt exits. Escape closes a modal or clears the input.

The permission prompt is a modal. It names the action, shows the exact command or path, and offers allow once, allow always for this pattern, and deny.

## Functional requirements

Requirements use EARS notation. The subject is the agent unless stated otherwise.

### Core behavior

FR-CORE-1. The agent SHALL run as one executable file that needs no separate language runtime.

FR-CORE-2. WHEN the user starts the agent with no arguments, the agent SHALL open the interactive interface in the current working directory.

FR-CORE-3. WHEN the user starts the agent with a prompt flag, the agent SHALL run one turn without an interactive interface and write the result to standard output.

FR-CORE-4. WHILE a model response is streaming, the agent SHALL render partial content as it arrives.

FR-CORE-5. WHEN the user sends the cancel key during a turn, the agent SHALL stop the in-flight request and keep the session history intact.

FR-CORE-6. IF a provider call fails with a retryable transport error, THEN the agent SHALL retry up to the configured limit with exponential backoff.

FR-CORE-7. IF a provider call fails after the retry limit, THEN the agent SHALL show the error, keep the session open, and return control to the user.

FR-CORE-8. The agent SHALL record the token count and the cost of each turn, including cache-read, cache-write, and extended-cache-write token counts separately from ordinary input and output tokens when the active provider reports them.

FR-CORE-9. IF a turn exceeds the configured maximum tool-call iteration count, THEN the agent SHALL end the turn with an iteration-limit error and return control to the user.

FR-CORE-10. The pre-tool hook SHALL run before the permission check, and a hook denial SHALL end the call without a user prompt.

### Session management

FR-SESS-1. The agent SHALL write each session to disk as an append-only log.

FR-SESS-2. WHEN the user runs the resume command, the agent SHALL list sessions for the current project with the newest first.

FR-SESS-3. WHEN the user selects a message and asks to fork, the agent SHALL create a new session that shares history up to that message.

FR-SESS-4. WHEN context use crosses the configured threshold, the agent SHALL invoke the enabled `compaction` extension.

FR-SESS-5. The agent SHALL perform compaction exclusively through a `compaction` world extension; there is no separate built-in compaction path outside that world. A default extension SHALL be enabled unless the user disables it.

FR-SESS-6. IF a session log contains a record that fails to parse, THEN the agent SHALL load the records before the failure and report a truncated session.

FR-SESS-7. WHEN the user runs the export command, the agent SHALL produce the export specified in `docs/session-log-format.md`, including redaction and the stripping of `permission` and `extension-event` records unless an audit flag is passed.

### Compaction and context transformation

FR-CTX-1. The agent SHALL write a `compaction` extension's result as a durable session record and SHALL reuse it on later reads without invoking the extension again until usage next crosses the threshold.

FR-CTX-2. The agent SHALL apply every enabled `context-transform` extension to the resolved message list, in installation order, before every provider call.

FR-CTX-3. IF a `context-transform` extension returns a rejection, THEN the agent SHALL end the turn and surface the rejection reason without calling the provider.

FR-CTX-4. The agent SHALL NOT write a `context-transform` extension's output to the session log.

### Cache behavior

FR-CACHE-1. The agent SHALL compute cache waste for a session from provider-reported usage numbers, comparing each assistant turn's prompt token count against the previous turn's and subtracting tokens actually read from cache.

FR-CACHE-2. The agent SHALL reset the cache-waste baseline on a `compaction` record and SHALL NOT reset it on a provider or model change.

FR-CACHE-3. The agent SHALL NOT count a cache miss below a configured noise-floor token count.

FR-CACHE-4. WHERE a provider has never reported cache activity within a scan, the agent SHALL treat that provider's turns as having no measurable cache waste rather than as a total miss.

FR-CACHE-5. The agent SHALL pass the current stable-prefix boundary, computed from the most recent compaction record, to the active provider extension on every completion call.

FR-CACHE-6. IF the post-transform content within the stable-prefix boundary differs from what was sent to the provider on the previous turn, THEN the agent SHALL narrow the boundary reported for that turn to end before the earliest differing message and SHALL record the divergence as an extension event rather than rejecting the turn.

### Tools

FR-TOOL-1. The agent SHALL provide built-in tools for read, write, edit, list, glob, grep, and shell.

FR-TOOL-2. IF an edit call targets a file that changed after the last read in this session, THEN the agent SHALL reject the call and return an error to the model.

FR-TOOL-3. WHEN a tool call targets a path outside the workspace root, the agent SHALL ask the user for approval before it runs the call.

FR-TOOL-4. WHILE a shell command runs, the agent SHALL stream its output to the interface.

FR-TOOL-5. IF a shell command exceeds its timeout, THEN the agent SHALL stop the command's process tree and return a timeout error.

FR-TOOL-6. WHERE the host operating system is Windows, the agent SHALL run shell calls through the platform shell.

FR-TOOL-7. The agent SHALL truncate a tool result that exceeds the configured size limit and mark it as truncated.

### Extension host

FR-EXT-1. The agent SHALL load extensions that implement a published world of the `lca:ext` package.

FR-EXT-2. WHEN the agent starts, the agent SHALL instantiate each enabled extension before it accepts the first user input.

FR-EXT-3. IF an extension traps during a call, THEN the agent SHALL disable that extension for the session, report the failure, and continue.

FR-EXT-4. IF an extension exceeds its fuel budget during a call, THEN the agent SHALL cancel the call and return an error to the caller.

FR-EXT-5. IF an extension exceeds its memory limit, THEN the agent SHALL stop the instance and disable the extension for the session.

FR-EXT-6. WHERE an extension is compiled into the binary, the agent SHALL register it through the same dispatch interface as a WASM extension.

FR-EXT-7. The agent SHALL show whether each extension runs sandboxed or in-process in the extension list.

FR-EXT-8. IF an extension declares an ABI version outside the host's supported window at load time, THEN the agent SHALL disable that extension for the session, report, on a best-effort non-blocking check, whether a compatible version exists in the registry, and continue the session rather than fail to start.

FR-EXT-9. WHEN the user inspects an extension, the agent SHALL show the denial count recorded for it.

FR-EXT-10. IF an extension log message exceeds the configured limit, THEN the host SHALL truncate it before writing it to diagnostic output.

FR-EXT-11. IF an extension registers a tool or command name that collides with a built-in name or with a name an already-enabled extension registered, THEN the agent SHALL keep the earlier or built-in registration, disable the later one for the session, and report the collision.

### Capabilities and permissions

FR-PERM-1. The extension manifest SHALL declare every capability that the extension needs.

FR-PERM-2. WHEN the user installs an extension, the agent SHALL show the declared capabilities and ask for confirmation before it writes the extension to disk.

FR-PERM-3. IF an extension calls a host import for a capability that its manifest does not declare, THEN the host SHALL return a permission error and record the attempt.

FR-PERM-4. WHEN an extension makes an outbound request, the host SHALL compare the target host and port against the granted patterns.

FR-PERM-5. IF the target host and port do not match a granted pattern, THEN the host SHALL deny the request and record the denial.

FR-PERM-6. The host SHALL restrict each extension to its own credential namespace.

FR-PERM-7. IF an extension reads a credential namespace that it does not own, THEN the host SHALL deny the read.

FR-PERM-8. WHEN the user approves an action with the always option, the agent SHALL write the approval to the user grant store, keyed by the canonical path of the current project, and SHALL NOT write it to any file inside the project directory.

FR-PERM-9. WHILE a project is untrusted, the agent SHALL ignore extension settings and permission defaults from the project configuration file.

FR-PERM-10. WHEN the project's permission proposal set changes after the user has approved an earlier version of it, the agent SHALL prompt with the difference between the approved set and the new set before it applies the change.

FR-PERM-11. The agent SHALL NOT enforce a grant that exists only in the project configuration file and has not been copied into the user grant store.

FR-PERM-12. IF a guest path resolution under the `fs` capability leaves its granted scope, THEN the host SHALL refuse the operation and record the attempt.

FR-PERM-13. IF a hostname granted under the `net` capability resolves to an address in one of the local ranges the capability catalog lists for `net-local`, THEN the host SHALL refuse the connection and record the attempt as a rebinding case, distinct from an ordinary denial.

FR-PERM-14. IF a manifest declares a `net-local` address outside the local ranges the capability catalog lists, THEN the agent SHALL reject the manifest at install time.

FR-PERM-15. IF a manifest declares a bare wildcard covering every host under `net`, THEN the agent SHALL reject the manifest at install time.

FR-PERM-16. WHERE a grant beyond the `fs` or `net` fixed vocabulary is genuinely needed, the agent SHALL allow the user to attach it as an ad hoc grant, at install time or later, with consent text naming the specific path or host being added rather than the manifest requesting it.

FR-PERM-17. The host SHALL normalize IPv4-mapped IPv6 addresses before any range check under `net` or `net-local`.

FR-PERM-18. WHEN the user attaches an ad hoc grant during a session, the agent SHALL honor it for subsequent calls in that session without requiring a restart.

FR-PERM-19. The agent SHALL store project trust state, per-project extension enablement, and ad hoc grants in the user grant store, keyed by the canonical path of the current project.

### Providers

FR-PROV-1. The core SHALL contain no vendor-specific model logic.

FR-PROV-2. WHERE a provider extension is enabled, the agent SHALL list its models in the model picker.

FR-PROV-3. WHEN a provider extension starts an authorization flow, the host SHALL open the loopback listener and return the callback parameters to the extension.

FR-PROV-4. The host SHALL bind the loopback listener on the local interface only.

FR-PROV-5. IF a stored credential is expired and a refresh token exists, THEN the provider extension SHALL refresh the credential before the next call.

FR-PROV-6. IF no provider extension is enabled, THEN the agent SHALL report that no model is available and offer the install command.

FR-PROV-7. The provider extension SHALL emit a tool call start event before any argument delta for that call.

FR-PROV-8. IF an argument delta arrives for a call identifier with no open start event, THEN the host SHALL discard the delta and record a protocol error.

FR-PROV-9. The agent SHALL ship with the OpenAI-compatible provider extension enabled by default and SHALL allow the user to disable it, resulting in zero enabled providers as a valid state.

FR-PROV-10. WHERE a provider extension exports `login`, `logout`, or `usage`, the agent SHALL register both a generic command that dispatches to the active provider and a command namespaced under the extension's own name.

FR-PROV-11. WHEN the user runs the generic login command, the agent SHALL list every installed provider extension by name and invoke the chosen one's `login` export.

### User interface

FR-UI-1. The agent SHALL render extension contributions from a declarative widget tree.

FR-UI-2. The host SHALL NOT pass terminal escape sequences from an extension to the terminal.

FR-UI-3. WHEN the terminal is resized, the agent SHALL re-render the layout without losing scrollback.

FR-UI-4. WHILE the agent waits for user approval, the agent SHALL show the exact command or path that triggered the prompt.

FR-UI-5. WHERE the terminal does not support color, the agent SHALL render with plain text only.

FR-UI-6. An extension SHALL NOT open a modal during a running turn unless the user invoked it.

### Distribution and installation

FR-DIST-1. The agent SHALL fetch extensions over the OCI distribution protocol with a client built into the binary.

FR-DIST-2. The agent SHALL NOT need any external command line tool to install an extension.

FR-DIST-3. The agent SHALL verify the content digest of a downloaded artifact before it runs the artifact.

FR-DIST-4. IF the digest does not match the manifest, THEN the agent SHALL delete the download and report the failure.

FR-DIST-5. WHEN the user installs from a local path, the agent SHALL read the component and the manifest from that path and apply the same consent flow.

FR-DIST-6. The agent SHALL record the resolved digest and the source reference of each installed extension and SHALL reuse the digest on later loads.

FR-DIST-7. WHEN the user runs the update command, the agent SHALL resolve the extension's ABI line tag to a digest and SHALL prompt for confirmation before it applies any capability the current grant does not already cover.

FR-DIST-8. The agent SHALL load an installed extension by its recorded digest and SHALL NOT resolve a moving tag at ordinary load time.

FR-DIST-9. The agent SHALL fetch an extension from a plain HTTPS-hosted archive as an alternative to an OCI reference, applying the same digest verification and consent flow as an OCI-sourced install.

### Web target

FR-WEB-1. WHERE the agent is built for the web target, the agent SHALL run as a transpiled ES module in a JavaScript host.

FR-WEB-2. WHERE the agent runs in a JavaScript host, extensions SHALL run as sibling modules in the same host engine rather than under a WASM runtime nested inside the agent's own module. See ADR-0018.

FR-WEB-3. WHERE the agent runs in a JavaScript host, the host application SHALL supply the file system and network implementations.

### Concurrency and cancellation

FR-CONC-1. WHEN the user cancels a turn, the agent SHALL interrupt any running WASM extension call through epoch interruption rather than waiting for its fuel budget to be exhausted.

FR-CONC-2. The agent SHALL execute the tool calls within a single turn sequentially rather than concurrently.

FR-CONC-3. WHEN a turn is cancelled, the agent SHALL stop the in-flight provider stream, any running extension call, and any running shell command concurrently rather than in sequence, and SHALL retain whatever session records are already complete.

### Configuration and privacy

FR-CFG-1. The agent SHALL merge configuration from flags, environment, project file, user file, and defaults, in that precedence order.

FR-CFG-2. WHEN the user runs the config command, the agent SHALL print each resolved value and the source that set it.

FR-CFG-3. The agent SHALL NOT send telemetry. Telemetry is out of scope for 1.0.

FR-CFG-4. WHERE telemetry is added after 1.0, the agent SHALL make it opt-in and SHALL send counters and error classes only, and SHALL NOT send prompt text, file content, or file paths.

FR-CFG-5. The agent SHALL NOT write credentials to the session log.

FR-CFG-6. WHILE running interactively, the agent SHALL check for a newer version at most once per day and SHALL NOT block startup on the check; headless mode SHALL make no such request unless the user enables it.

## Non-functional requirements

### Size and performance

NFR-1. The default native binary SHALL NOT exceed 25 MB on x86-64 Linux. This target is provisional. Phase 0 measures the real cost of the WASM runtime and the number becomes fixed after that.

NFR-2. A minimal build with the interpreter backend only SHALL NOT exceed 12 MB on x86-64 Linux.

NFR-3. Cold start to an interactive prompt SHALL NOT exceed 150 ms on a 2020-class laptop with no extensions enabled.

NFR-4. Instantiation of one precompiled WASM extension SHALL NOT exceed 20 ms.

NFR-5. A hook call into a WASM extension SHALL NOT exceed 1 ms of overhead above the extension's own work.

NFR-6. Idle memory use SHALL NOT exceed 80 MB with no extensions enabled.

NFR-7. The continuous integration pipeline SHALL measure binary size and startup time on every merge to the main branch, and SHALL fail the build when a threshold is exceeded.

### Platform support

NFR-8. The agent SHALL support Linux on x86-64 and aarch64 with a statically linked musl build.

NFR-9. The agent SHALL support macOS on x86-64 and aarch64.

NFR-10. The agent SHALL support Windows on x86-64 and aarch64.

NFR-11. The agent SHALL support a `wasm32-wasip2` build for JavaScript hosts.

NFR-12. The continuous integration pipeline SHALL run the test suite on Linux, macOS, and Windows runners.

### Security

NFR-13. The agent SHALL deny by default. An action with no explicit grant does not run.

NFR-14. The agent SHALL store credentials with file permissions that restrict access to the owning user.

NFR-15. The agent SHALL NOT enable executable memory in builds that target environments which forbid it.

NFR-16. The project SHALL run dependency audit and license checks on a schedule and on every merge.

NFR-17. Release artifacts SHALL be reproducible from a tagged commit, and the pipeline SHALL publish checksums for each artifact.

### Compatibility

NFR-18. The extension ABI SHALL follow semantic versioning.

NFR-19. The host SHALL load extensions built against the current ABI minor version and the previous one.

NFR-20. A change that removes or alters an exported function SHALL increment the ABI major version.

NFR-21. The host SHALL NOT change extension behavior when the WASM runtime is upgraded within one ABI version.

### Quality

NFR-22. The core crates SHALL have automated tests that run without network access.

NFR-23. Tests that need live provider credentials SHALL skip when the credentials are absent, and SHALL NOT fail the build.

NFR-24. Every fixed defect that reached a release SHALL get a regression test named after its tracking identifier.

NFR-25. The extension ABI SHALL have a conformance test extension that exercises every host import and every exported function.

NFR-30. Every functional and non-functional requirement in this document SHALL be referenced by at least one test, verified by an automated check run on every pull request; the check is advisory before Phase 8 and a required pipeline gate from Phase 8.

NFR-31. A canonical twenty-turn scripted session with no compaction and no provider change SHALL maintain a cache-hit ratio, measured from turn two onward, above a threshold fixed at the Phase 3 exit test and checked in the same pipeline gate as the binary-size and cold-start measurements.

### Accessibility and usability

NFR-26. The interface SHALL work in a terminal with 80 columns.

NFR-27. The interface SHALL function without mouse input.

NFR-28. Color SHALL NOT be the only signal for state. Every colored state also carries a text or symbol cue.

### Concurrency

NFR-29. Cancellation of a running WASM extension call, once the user triggers it, SHALL take effect within 50 ms under normal load, measured from epoch increment to instance trap.

## Dependencies

The dependency list stays short on purpose. Each entry below states what it does and what replaces it if it fails.

| Crate or tool | Purpose | Fallback |
|---|---|---|
| wasmtime | Extension runtime, Component Model, Pulley interpreter | wasmi for an interpreter-only build |
| wit-bindgen | Guest bindings for extension authors | Hand-written canonical ABI glue |
| wasm-pkg-client | OCI reference resolution and artifact fetch | Direct OCI Distribution API calls over the existing HTTP client |
| tokio | Async runtime | None. This is a structural commitment. |
| hyper with rustls | HTTPS transport | reqwest as a thicker alternative |
| tower-service | The `Service` trait the hyper-util `HttpConnector` accepts as a custom DNS resolver; used to pin the `net` rebinding check's resolved address (ADR-0025) | Hand-written resolver behind a different connector, or accept the TOCTOU race ADR-0025 closes |
| ratatui with crossterm | Terminal rendering and input | A renderer written in the project, over crossterm alone |
| serde and serde_json | Serialization | None |
| clap | Argument parsing | Hand-written parser |
| tracing | Structured logging and diagnostics | log with env_logger |
| sha2 | Digest verification | ring |
| ipnet | CIDR parsing and range membership for `net-local` validation and `net` rebinding checks | Hand-written IPv4/IPv6 range comparison over std's `IpAddr` |
| toml | Configuration and manifest parsing | None |
| jco | Transpiles components to ES modules for the web build | Hand-written JS glue over core modules |
| cargo-zigbuild | Cross-compilation linking for macOS and Linux targets | Native runners per platform |
| cargo-xwin | Cross-compilation for Windows MSVC targets | The `pc-windows-gnu` target, or native Windows runners |
| cargo-nextest | Test runner | `cargo test` |
| proptest | Property-based testing for session log, compaction, and transform-chain invariants | Hand-written table-driven test cases |
| criterion | Benchmark harness for the NFR-1 through NFR-7 and NFR-29 performance gates, and the cache-hit-ratio benchmark | Custom timing harness over `std::time` |
| cargo-deny | License and advisory checks | `cargo audit` |

Build-time tools do not ship in the binary. Only the library crates in the top half of the table become part of the release artifact.

## Folder structure

```
.
├── Cargo.toml                  workspace manifest
├── rust-toolchain.toml         pinned toolchain
├── deny.toml                   license and advisory policy
├── wit/
│   ├── world-provider.wit
│   ├── world-tool.wit
│   ├── world-command.wit
│   ├── world-hooks.wit
│   ├── world-ui.wit
│   ├── world-compaction.wit
│   ├── world-context-transform.wit
│   └── host/                   host import interfaces
├── crates/
│   ├── lca-cli/                the binary
│   ├── lca-core/               agent loop and dispatch
│   ├── lca-protocol/           shared types
│   ├── lca-session/            storage, fork, resume, compaction records; compaction and transform dispatch live in lca-core
│   ├── lca-tools/              built-in tools, behind a swappable backend trait; see ADR-0013
│   ├── lca-permissions/        grants, prompts, approval store
│   ├── lca-provider/           provider trait and stream types
│   ├── lca-ext-abi/            WIT plus generated bindings
│   ├── lca-ext-host/           WASM host and capability enforcement
│   ├── lca-ext-native/         in-binary extension registry, default-feature wiring
│   ├── lca-registry/           OCI and HTTPS-archive resolvers, lockfile
│   ├── lca-config/             configuration merge
│   ├── lca-tui/                renderer, widgets, input editor
│   ├── lca-sdk/                embedding API
│   └── lca-testkit/            fake provider and harness
├── extensions/
│   ├── conformance/            ABI conformance extension
│   ├── hooks-example/          reference hooks implementation
│   ├── openai-compatible/      default provider; native-linked by default
│   ├── antigravity/            reference OAuth provider; WASM by default
│   ├── codex/                  second OAuth provider; WASM by default
│   ├── lmstudio/                local provider; net-local by default
│   ├── ollama/                  local provider; net-local by default
│   ├── skills/                 skills handling; native-linked by default
│   └── compaction-default/     the default compaction extension; native-linked by default
├── web/
│   ├── orchestrator/           JS glue for the browser build
│   └── examples/
├── xtask/                      build, size check, release packaging
├── docs/
│   ├── adr/                    architecture decision records, 0001 through 0018
│   ├── capabilities.md         the capability catalog
│   ├── extension-authoring.md
│   ├── abi-versioning.md
│   ├── session-log-format.md
│   ├── flows.md                sequence diagrams for the runtime paths
│   ├── threat-model.md
│   ├── release-policy.md
│   ├── configuration.md        configuration keys, defaults, and merge sources
│   ├── headless.md             headless output envelope and exit codes
│   ├── testing-plan.md         the software testing plan
│   ├── platform-notes.md
│   ├── glossary.md
│   ├── inspiration.md          provenance: what is taken from Pi and fx, and what is novel
│   └── providers/              one document per first-party provider extension
├── schemas/
│   └── extension-manifest.schema.json
├── tests/
│   ├── regressions/            one file per closed defect, named by tracking identifier
│   └── ...                     workspace-level integration tests
└── .github/workflows/
```

Extensions in `extensions/` build two ways. The workspace builds them as normal crates for the native-linked path. The `xtask` build target compiles them to `wasm32-wasip2` components for the sandboxed path. Both come from the same source. `lca-cli` gates each one behind its own Cargo feature; `bundled-openai-compat`, `bundled-skills`, and `bundled-compaction-default` are on by default, the rest are off by default and installed like any third-party extension. See ADR-0013.

## Coding standards

The project uses stable Rust with a pinned toolchain version in `rust-toolchain.toml`. Upgrades happen deliberately, in their own pull request, with the size and startup measurements attached.

Formatting is `rustfmt` with the default profile. Linting is `clippy` with warnings denied in the pipeline. No pull request merges with a clippy warning.

`unsafe` is forbidden in every crate except where a documented need exists. Each crate declares `#![forbid(unsafe_code)]` unless it carries an exemption comment that names the reason and the reviewer.

Error handling uses `thiserror` for library crates and `anyhow` in the binary only. A library never returns a boxed opaque error. Every error type names its variants.

Public items carry documentation comments. The pipeline runs `cargo doc` with warnings denied, so a missing doc on a public item fails the build.

Naming follows the Rust API guidelines. Crate names use the `lca-` prefix. Module names are singular nouns. Trait names are verbs or capability nouns, not `-er` suffixes where a plain noun reads better.

Dependencies need a reason. A pull request that adds a dependency states in its description what the dependency does, why writing it is worse, and what its transitive footprint is. The reviewer checks the size delta.

Commit messages use a short imperative subject and a body that explains why. Pull requests stay small. A change that touches the ABI comes with a version bump and a note in the ABI changelog.

Every public interface change updates the documentation in the same pull request. Documentation drift is a defect.

## Testing strategy

This is a summary. The full testing plan, with the complete test taxonomy, the fake-provider design, the cache-behavior test scenarios, the requirements-traceability mechanism, and the CI execution strategy, is in `docs/testing-plan.md`. Test-driven development is not optional here: every requirement in this document should have a failing test written against it before the implementation that satisfies it, per the definition of done in the testing plan.

At the summary level: tests run offline by default, with an explicit opt-in for anything that needs the network. `lca-testkit` provides a fake provider, modeled on pi's own faux provider, that scripts realistic multi-turn conversations, including cache-relevant usage numbers per ADR-0017, so the agent loop, compaction, context transformation, retries, cancellation, and tool dispatch are all testable deterministically and at zero cost. The ABI's conformance extension runs in both native and WASM mode on every pipeline run, and any divergence between the two is a defect. Regression tests are named after the defect they close and are required in the same change as the fix. The pipeline runs on Linux, macOS, and Windows on every push, because platform-specific failures are the ones that hide.

## Security model

This is a summary. The full threat model, with the complete actor list, the trust boundaries, and the worked attack scenarios each with a stopped or not-stopped verdict, is in `docs/threat-model.md`. The Phase 8 security review works from that document, not from this summary.

At the summary level: the user is trusted. The model is untrusted and may be manipulated through prompt injection in file content or tool output. An extension is untrusted unless the user installed it knowingly, and even then the capability set limits it.

The model cannot take a sensitive action directly. Every shell command, every write outside the workspace, and every network call from an extension passes through the permission layer. A prompt injection can ask for a dangerous action. It cannot perform one without a user grant or a pre-approved pattern.

An extension holds only what its manifest declared and the user approved. The network allow list is enforced by the host, not by the extension. This matters most for provider extensions, which handle credentials and talk to remote endpoints. A compromised provider extension cannot send tokens to an unlisted host, because the import refuses the request.

Credential isolation is per namespace. One extension cannot read another's tokens. The credential store lives outside the session log and never appears in an export.

The native-linked mode has no sandbox. This is a deliberate hole for first-party code, and the interface labels it. A build that includes a third-party extension natively is out of policy.

A WASM sandbox escape would be a host compromise. The mitigations are: pin the runtime version, track its advisories, keep the host import surface small, and prefer the interpreter backend on any target where executable memory is a concern.

Supply chain controls run in the pipeline. License and advisory checks run on every merge and on a schedule. Release artifacts carry checksums and build from a tagged commit.

## Risks and mitigations

The table lists the risks that can change the design. Each row names the trigger that forces the alternative.

| Risk | Impact | Mitigation | Alternative if it fails |
|---|---|---|---|
| Wasmtime with Cranelift pushes the binary well past the size target | The minimalism goal is lost | Feature-gate the backends. Measure in Phase 0. Ship the interpreter build as the default if the delta is large. | Replace Wasmtime with wasmi for the default build and offer a separate full build |
| Component Model tooling is too young for the provider world | Streaming providers cannot ship on the typed ABI | Keep the WIT surface small. Pin toolchain versions. Spike streaming first in Phase 0. | Fall back to a core-module ABI with hand-written marshaling for the provider world only |
| Streaming across the component boundary is slow or awkward | Perceived latency on every turn | Use a stream resource with host-driven polling. Benchmark in Phase 0. | Invert the flow: the host exports a chunk callback that the extension calls |
| macOS notarization cannot run from a cross-compiled build | Release blocked for macOS | Cross-compile with cargo-zigbuild, then sign and notarize on a macOS runner | Build macOS artifacts entirely on macOS runners |
| jco browser support is not ready for the agent component | The web target slips | Treat the web target as tier 2 and schedule it late | Ship the web build with hand-written JS glue over core modules |
| The extension ecosystem never grows | The extension model adds cost with no payoff | Ship first-party extensions through the same ABI from day one. Dogfood it. | Keep the ABI, reduce the surface to tools and hooks only |
| The capability model blocks a legitimate extension | Authors work around the sandbox or give up | Collect the gaps during Phase 3. Add capabilities deliberately. | Point the case at an external tool protocol such as MCP behind the `process` and `net` capabilities, per ADR-0007. Add a broad capability only if that fails and three or more independent cases confirm a structural gap. |
| Interpreter performance is too slow for a hot extension | The agent feels sluggish | Precompile to a cached AOT artifact per target | Move that extension to the native-linked path |
| Windows terminal behavior differs enough to break the TUI | Windows users get a broken interface | Test on real Windows runners from Phase 1, not at the end | Ship a reduced renderer on Windows |
| OCI registry authentication is harder than expected | Private extension distribution slips | The plain-HTTPS-archive source kind, already part of the 1.0 design per ADR-0010, is available from the start rather than as a fallback; support anonymous public OCI pulls first and treat authenticated registry support as the part that can slip | Extensions distribute via the HTTPS-archive path exclusively until authenticated OCI support lands |
| The `net-local` range grant is wider than users understand, leading to real local-network exposure | An extension reaches a device the user didn't intend to expose it to | Consent text names the reach explicitly, per ADR-0011. Watch install patterns during Phase 3 for evidence users pick ranges wider than they need. | Narrow the vocabulary to single addresses and `.local` names only, dropping CIDR ranges, if evidence shows people grant ranges without meaning to |
| Pulling the `completion` capability into 1.0 for compaction complicates the freeze | The pre-freeze punch list grows, and `completion`'s host-mediated request path is new surface late in the plan | Build it in Phase 4 with its own conformance cases and threat model scenario immediately, not as a late addition bolted on before Phase 8 | Ship the default compaction extension with mechanical strategies only for 1.0, and add `completion` and LLM-based summarization in the first 1.x release instead |
| Two new worlds, `compaction` and `context-transform`, turn out to need a third shape once a real second or third transform extension exists | The two-world split from ADR-0015 doesn't cover a case that shows up in practice | Build the default compaction extension and skills handling as two independent `context-transform` consumers in Phase 4, specifically to stress the ordering and rejection behavior before the freeze | Add an explicit manifest-declared priority field to `context-transform` if installation order alone proves insufficient once real extensions exist |
| A cache-hit-ratio regression ships unnoticed, since a broken prompt cache produces correct answers and only shows up in cost or latency later | Coding-agent economics depend heavily on cache efficiency; a silent regression here is expensive and hard to trace back to its cause | The cache-hit-ratio benchmark in NFR-31 and the request-assembly snapshot test, per `docs/testing-plan.md`, are two independent, complementary detection layers from the start: one verifies the waste-detection logic against scripted usage numbers, the other catches a drift in the assembled request before it ever reaches a real provider | If both still miss a real-world regression class, add a scheduled job that probes a real provider periodically and compares its reported cache-read ratio against the fake-provider benchmark's expectation, closing the gap between simulation and reality |
| The core grows past the minimalism goal | The project becomes the thing it replaced | Write the scope guard into the review checklist. A new core feature needs a written argument for why it cannot be an extension. | Split the extra behavior into a separate optional binary |
| Effort is larger than planned | Release slips or ships incomplete | Phase gates with exit criteria. Cut the web target and the UI world before cutting the sandbox. | Ship the native binary with tools, hooks, and providers only, and defer the rest |

Two alternatives sit outside the table because they are whole-design choices rather than risks. Zig with a Zig-native WASM runtime gives easier cross-compilation and a smaller binary, and costs the Component Model tooling and compile-time memory safety. Extism gives a working plugin system sooner and costs the typed ABI. Neither is the plan. Both are real options if Phase 0 shows that Wasmtime and the Component Model cannot meet the size and streaming targets together.

## Implementation plan

Nine phases. Each phase has an exit test. A phase does not end because its time ran out. It ends when the exit test passes.

### Phase 0: measurement and spikes

Two to four weeks. This phase exists to kill bad assumptions early.

Build a minimal Rust binary that embeds Wasmtime and instantiates a component that implements a two-function world. Measure the binary size with Cranelift, with Pulley only, and with wasmi. Measure cold start and instantiation time. Build a streaming spike where the guest returns a stream of events and the host consumes them, and measure per-event overhead. Run the cross-compilation matrix for all six native targets and record what each one needs.

Exit test: the size, startup, and streaming numbers exist and the team has picked the default backend. NFR-1 and NFR-2 get their final values here.

### Phase 1: the core agent with no extensions

Six to ten weeks. Build the agent that works without any extension at all.

This covers `lca-protocol`, `lca-config`, `lca-session`, `lca-tools`, `lca-permissions`, `lca-tui`, `lca-cli`, and `lca-testkit`. `lca-testkit`'s fake provider and sandboxed test harness, per `docs/testing-plan.md`, are built alongside the very first feature rather than retrofitted once real work has piled up untested, since every other phase's exit test depends on it existing. The OpenAI-compatible provider is built and compiled in directly, behind the `lca-provider` trait, so the loop has something to call from the start; this is also the first proof of the build-time-backend-versus-runtime-extension distinction from ADR-0013 in practice. The permission layer works for shell and out-of-workspace writes. Sessions save, resume, and fork. The TUI renders, streams, and cancels.

Exit test: a user can hold a working coding session on Linux, macOS, and Windows, resume it the next day, and the test suite passes on all three in the pipeline.

### Phase 2: the extension host

Six to eight weeks. Add the sandbox and the first worlds and capabilities that don't depend on network identity.

Write the WIT for `tool`, `command`, and `hooks`. Build `lca-ext-abi`, `lca-ext-host`, and `lca-ext-native`. Implement fuel and memory limits, epoch interruption, trap isolation, and the always-granted log import. Implement the `fs`, `process`, and `pty` capabilities, since these depend only on the local machine, not on network identity, and belong here rather than waiting for Phase 3. Build the conformance extension and run it in both modes. Move at least one built-in behavior into an extension to prove the path.

Exit test: the conformance extension passes in native mode and WASM mode with identical results, a trapping extension disables itself without taking down the session, and a cancelled turn interrupts a running extension call within the latency bound in NFR-29.

### Phase 3: providers, network capabilities, and credentials

Eight to ten weeks. This is the hardest world and it comes before distribution on purpose. It also carries every capability that depends on network identity: `net`, `net-local`, `oauth`, and `credentials`.

Write the `provider` world with streaming, the cache-boundary parameter and the `usage` event's cache token fields per ADR-0017, plus `login`, `logout`, and `usage` per ADR-0012, and the generic dispatch and auto-namespacing commands built on them. Implement the `net`, `net-local`, `oauth`, and `credentials` capabilities in the host, including the resolved-address rebinding check on `net`. Build the loopback listener in the host and expose it as a capability. Build the cache-waste measurement module against the fake provider's scripted usage numbers, following pi's own approach directly, so it exists and is tested before there is a `compaction` record to reset it against. Port the Phase 1 OpenAI-compatible provider from a compiled-in placeholder to its real, dual-mode form under `extensions/openai-compatible/`, then build Antigravity as the OAuth reference provider, proving the loopback flow, `vendor-event`, and credential namespacing all work together on a real vendor. Codex, LM Studio, and Ollama follow the same two proven patterns, OAuth and `net-local`, and are not required for the phase exit test.

Exit test: two provider extensions work, one with an API key and one with an OAuth subscription login, neither one touches a socket or a credential file directly, both respond correctly to the generic `/login`, `/logout`, and `/usage` commands as well as their namespaced forms, and a scripted clean multi-turn conversation against the fake provider reports zero cache waste from the second turn onward.

### Phase 4: compaction, context transform, and skills

Four to six weeks. Two new worlds and the capability one of them needs, proven with two independent first-party consumers rather than one.

Write the `compaction` and `context-transform` worlds. Implement the `completion` capability in the host, routing a granted extension's request to whichever provider is currently active, with its own conformance cases and threat model scenario built alongside it rather than after. Wire the cache-waste module's baseline reset to the `compaction` record and the boundary-narrowing behavior to a `context-transform` extension touching the stable region, completing the ADR-0017 mechanism that Phase 3 built the provider-facing half of. Build the default compaction extension under `extensions/compaction-default/`, using `completion` for real summarization, and wire it into the session log exactly as `docs/session-log-format.md` specifies. Build skills handling under `extensions/skills/` as a `context-transform` consumer, proving the world serves a real, non-hypothetical case beyond compaction. Both ship native-linked by default.

Exit test: usage crossing the configured threshold triggers the default compaction extension, its summary persists across a restart without being recomputed, the cache-waste baseline resets exactly on that turn and reports zero waste on every turn after, skills handling injects matched instructions through the transform chain on an ordinary turn without affecting the cache boundary, and a transform extension that returns a rejection ends the turn with the reason surfaced rather than calling the provider.

### Phase 5: distribution

Four to six weeks. Make installation work without any other tool on the machine, from more than one kind of source.

Build `lca-registry` with OCI reference resolution and the plain-HTTPS-archive resolver from ADR-0010, sharing one lockfile and one digest-verification path. Define the manifest format and the consent screen. Implement `ext install`, `ext update`, `ext remove`, `ext info`, and `ext list`. Publish the reference extensions to a public registry and to a plain HTTPS host, and install from both on a clean machine.

Exit test: a clean machine with only the `lca` binary installs a provider extension from an OCI reference and a second extension from a plain HTTPS archive, sees the capability prompt for each, approves them, and runs a turn.

### Phase 6: the UI world

Four weeks. Give extensions a way to draw without giving them the terminal.

Define the widget vocabulary and the `ui` world. Implement regions for the status line, the footer, a side panel, and a modal. Build a reference extension that adds a status segment and a panel, and extend it, or build a second small one, to exercise `pty` rendering a spawned interactive program's output into a panel, proving the ADR-0016 pattern end to end.

Exit test: an extension renders in all four regions, a hostile extension that returns escape sequences in a text span renders them as literal characters, and a `pty`-backed extension displays a live interactive session inside a panel with no raw terminal access of its own.

### Phase 7: the web target

Six to eight weeks. Tier 2. Cut this first if the schedule slips.

Build the `wasm32-wasip2` target for the agent. Transpile with jco. Write the JS orchestrator that instantiates the agent module and the extension modules as peers and wires the imports, per ADR-0018, rather than embedding a second WASM runtime inside the agent's own module. Supply browser implementations for file system and network through the host application. Confirm, per `docs/platform-notes.md`, that a `process`- or `pty`-holding extension fails with an ordinary permission error in this build rather than a crash, and that `net-local` is documented as not expected to function inside a browser's own private-network restrictions.

Exit test: a browser page runs a session with one extension loaded, and the same extension binary also runs in the native build without a rebuild.

### Phase 8: hardening and ABI freeze

Six weeks. Nothing new ships in this phase.

Fill the test gaps. Turn on the requirements-traceability check from NFR-30 as a required pipeline gate, not just an advisory report, once the backlog of untagged tests from earlier phases is cleared. Run the fuzz targets long enough to matter. Complete the documentation set in `docs/`, including every provider profile under `docs/providers/`. Finalize the ABI versioning policy and freeze `lca:ext` at 1.0, closing out the punch list named in `docs/abi-versioning.md`. Set up reproducible builds and artifact checksums. Run an external review of the capability enforcement code, with particular attention to the `net`/`net-local` boundary and the `completion` capability, both added later in the plan than the rest of the capability set.

Exit test: the ABI is frozen at 1.0, the pipeline publishes signed and checksummed artifacts for all six native targets, the requirements-traceability check passes with zero untagged requirements, and the security review has no open findings above low severity.

## Open questions and decisions

Each question below carries a recommended decision, the options that were considered, and what the decision changes elsewhere in the document. A decision holds until the phase that owns it produces evidence against it. The phase that forces each one is named.

### Streaming shape for the provider world

Phase 3 forces this. The question is what crosses the boundary when a provider extension streams a model response.

Option A is a typed event stream. The WIT defines a variant with cases for text, reasoning, tool call start, tool call argument delta, tool call end, usage, and error. The extension parses the vendor format and emits typed cases.

Option B is an opaque text stream. The extension forwards text and the host parses tool calls out of it. This sounds simpler and is not. Tool call arguments arrive from every major vendor as partial JSON fragments interleaved with text, and each fragment belongs to a specific call index. Carrying that in a text stream needs a framing format, which is a structured protocol written as strings. The host then parses twice: once for the frames, once for the JSON.

Option C is a typed envelope with an opaque payload. Each event has a typed kind and a JSON string body. It keeps the WIT small at the cost of losing type checking on the part that matters.

| Option | Pros | Cons |
|---|---|---|
| A, typed event stream | Type checked at the boundary. No re-parsing. Call indexing is explicit. The interface documents itself. | Adding a variant case is an ABI break. Largest WIT surface of the three. |
| B, opaque text | Smallest WIT. Any future vendor concept fits without an ABI change. | Needs a framing format, so the structure comes back as strings. Two parse passes. Tool call indexing is fragile. No type checking. |
| C, typed envelope | Small WIT. Vendor extras pass through. | The payload is unchecked. Errors move from compile time to run time. |

Decision: option A, with one addition. Include a `vendor-event` case from the first release that carries a kind string and a JSON payload. Adding variant cases later breaks the canonical ABI, so the extension point has to exist before the freeze. Anything the typed cases do not cover travels in `vendor-event` until it earns a typed case in the next major ABI version.

Drive the stream with a pull-based resource, not with WASI 0.3 native async. The resource exports a next function that returns the next event or end of stream, and the host polls it from an async task. WASI 0.3 async is too new to sit under the provider path. The resource shape maps onto native async later without a change to the event variant.

Argument accumulation belongs to the host. The extension emits start, delta, and end for each call. The host accumulates the fragments keyed by call identifier and parses the completed string. This keeps the tool schema out of the extension, which has no reason to know it.

What changes: `lca-protocol` gains a stream event type that mirrors the WIT variant one case per case, and the mapping is generated rather than written by hand. The accumulator lives in `lca-provider`. FR-PROV-7 and FR-PROV-8 hold the two requirements this forces.

### Filesystem scopes

Phase 2 forces this. The first draft gave the `fs` capability two scopes, the workspace root and a private per-extension directory.

The git example in the original question turns out to be a bad example. A git directory sits inside the workspace root, so the workspace scope already covers it. The real gaps are elsewhere: a global config directory holding an existing login from a vendor command line tool, a temporary directory for large intermediate files, and sibling checkouts in a multi-repository layout.

Option A keeps the two fixed scopes. It is the smallest attack surface and it blocks a provider extension from reading credentials that the user already has on disk, which is a common and reasonable thing for a provider extension to want.

Option B defines a fixed vocabulary of named scopes. The manifest names a scope and an access mode. The host resolves the name to a path. The extension never writes a path into the manifest.

Option C lets the manifest request arbitrary paths. It is the most flexible and the worst consent surface. A manifest asking for the home directory and a manifest asking for the root directory read the same way to most people, which is to say they both read as noise, and people click through noise.

Decision: option B, plus one user-driven addition. The first vocabulary is `workspace`, `private`, `home-config`, and `temp`, each with a read or write mode. Beyond that, the user can attach an extra path grant at install time or later through the extension settings. The manifest cannot request it and the consent screen does not offer it. This keeps arbitrary paths available for the rare case while keeping them out of the routine install flow.

Model the scope name as a string in the manifest and validate it against the host vocabulary, rather than as a WIT enum. A string keeps new scope names additive. An enum makes every new scope an ABI break.

Path safety is not negotiable here. The host passes preopened directory handles and the guest resolves paths relative to a handle. The host never joins guest-supplied strings onto a base path, and it refuses any resolution that leaves the scope through a symbolic link or a parent traversal.

What changes: `lca-permissions` owns the vocabulary and the resolution. `lca-ext-host` builds the WASI context preopens from the granted set. FR-PERM-12 holds the requirement this forces. The capability model section under Architecture and the capability catalog both reflect the four-scope vocabulary; the two-scope description that appeared in an earlier draft of this document was the thing this record replaced.

### Permission store location

Phase 1 forces this. The tension is real in both directions. A store in the project directory can be reviewed in a pull request and shared across a team. A store in the project directory also means that pulling a branch can change what the agent is allowed to do, without anyone reading the diff.

Option A puts the store in the user directory, keyed by project path. Nothing leaks into the repository and nothing is shareable. Every developer approves the same commands again.

Option B puts the store in the project directory and commits it. Team sharing works. A pull request that edits the permission file is a privilege escalation that looks like a configuration change, and a fresh clone grants whatever the file says.

Option C splits the two roles. The project file holds proposals. The user store holds grants. Only the user store is consulted when an action runs. Approving a proposal copies it into the user store once, behind a prompt that shows what is being added.

Decision: option C. It is more work than either single-location option and it is the only one that gives sharing without turning a clone into a grant. It also composes with the project trust rule already in FR-PERM-9, which ignores untrusted project configuration.

The mechanics matter more than the layout. The user store records the hash of the proposal set it approved. When the project file changes, the hash stops matching and the agent prompts again, showing only the difference rather than the whole file. Without the hash check, an edit to a proposal inside a large merge lands silently, which is the exact failure the split was built to prevent.

Key the user store by the canonical path of the working copy. Two checkouts of the same repository are two projects, because trust attaches to the directory a person chose to open, not to a remote URL.

What changes: `lca-permissions` reads two sources and enforces from one. FR-PERM-8 already reflects that approval writes to the user store; FR-PERM-10 and FR-PERM-11 hold the two requirements this record adds.

### An out-of-process extension runner

Phase 3 shows whether this is real. The question is whether a second binary should run components with full operating system privileges for cases the sandbox cannot serve.

Listing the cases first makes the answer clearer. What the sandbox cannot give today is raw socket access for protocols other than HTTPS, filesystem watching, platform keychain access, long-lived background processes, and native graphical windows. Three of those five are better solved as narrow host capabilities than as an escape hatch. Keychain access belongs behind the credentials capability anyway, since the host should prefer the platform keychain over a file. Filesystem watching is a small capability with an obvious shape. A narrow outbound TCP capability with a host-enforced allow list covers most of the socket cases without opening a general one.

Option A ships no runner. The native-linked path stays the only unsandboxed tier, and it is reserved for first-party code.

Option B ships a second binary that runs a component unsandboxed and talks to the main process over a pipe. It breaks the single file property, needs process lifecycle management, and needs the whole ABI serialized over IPC. The worst part is not the engineering. It is that a third trust tier is one more than a person can hold in their head while reading an install prompt.

Option C adds capabilities as evidence arrives and points the genuinely unbounded cases at an external tool protocol. An extension that holds the `process` and `net` capabilities can speak a tool protocol such as MCP to a server the user already trusts and installed by other means. The privilege lives in that server, which the user manages, rather than in a new tier of this agent.

Decision: option A for 1.0, with option C as the pressure valve, and option B deferred until Phase 3 produces a case that neither covers. Phase 3 collects the gaps. Each gap becomes either a new named capability or an argument for the runner, and the argument has to be written down.

What changes: the risk table row about a restrictive capability model, below, now names the external tool protocol as its first fallback rather than a broad capability. Nothing in the architecture changes for 1.0.

### Dependencies between extensions

This one can be deferred past 1.0, and deferring it is the recommendation, but the reasoning matters because the word dependency covers three different needs.

The first need is code reuse. An author wants a shared library for vendor authentication across three provider extensions. The second need is service access. A workflow extension wants to ask the active provider for a completion. The third need is version coupling, where one extension refuses to load without another at a given version.

Option A supports none of it. Shared code is vendored into each component at build time. Option B uses Component Model composition, where the author composes dependencies into one component before publishing and the host loads a single artifact. Option C adds a host-side resolver that fetches and links dependencies at load time.

Decision: build-time composition for code reuse, host-mediated capability for service access, and nothing at all for version coupling.

Composition costs the host nothing, which is the point. The work happens in the author's build, and what arrives at the host is one component with one manifest and one capability set. A resolver, by contrast, brings version solving, a lockfile per extension, diamond conflicts, and a capability question with no good answer: when A pulls in B, whose manifest declares the grants.

Service access should never become an extension-to-extension call. An extension that wants a completion asks the host for one through a capability, and the host routes to whatever provider is active. This keeps the graph a star with the host at the center. Direct calls between instances would make it a general graph, and a general graph needs load ordering, cycle detection, and a story for what happens when one node is disabled mid-session.

Version coupling stays unsupported because an extension that cannot function alone is really one half of a composed component.

What changes: nothing in the host. Phase 8 gains a documentation task that shows the composition workflow in the extension authoring guide.

### Updating an extension across an ABI minor version

Phase 5 forces this. The host supports the current ABI minor version and the one before it, per NFR-19, so an author has one minor cycle to publish a rebuilt component.

Option A does nothing automatic and tells the user to reinstall. Option B records a digest in a lockfile, resolves a moving tag at explicit update time, and always loads by digest. Option C updates extensions automatically when the host upgrades.

Option C is wrong for a product with a capability model. An automatic update can change what an extension asks for, and a capability change that nobody approved defeats the consent flow. Any update that widens the capability set has to prompt, which makes it not automatic.

Decision: option B, with the registry tag scheme doing most of the work. Publish each release under an immutable version tag, and also under a moving tag for its ABI line, such as `abi-0.1`. The resolver then has one job: resolve the moving tag to a digest. The digest goes in the lockfile and every later load goes by digest, which preserves FR-DIST-6 and keeps a mutable tag from becoming a supply chain hole.

The failure path needs to be specific, because this is where a user gets stuck. When the host loads an extension whose ABI version it no longer supports, it refuses the load, disables that extension for the session, and reports whether a compatible version exists in the registry. The message names the command that fixes it. The session continues without the extension rather than failing to start, because a broken extension should not cost the user their agent.

What changes: the command line section gains `lca ext update <name>` and `lca ext update --all`, both already reflected there. FR-DIST-6 through FR-DIST-8 hold the digest and source recording and the update prompt behavior; FR-EXT-8 holds the disable-and-continue behavior for an unsupported ABI version encountered at ordinary load time, which is the same behavior the update path falls back to when it cannot find a compatible version.
