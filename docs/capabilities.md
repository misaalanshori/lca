# Capability catalog

Version 0.1, 2026-09-20. Targets the current ABI line (0.x development window per ADR-0028; the declared line follows each release).

This is the reference for every capability an extension can hold. It covers what each one grants, how a manifest declares it, what the host imports look like, what the user sees at install time, and what happens on denial.

## How capabilities work

A capability is a named grant attached to an extension instance. The manifest declares what the extension needs. The user approves at install time. The host resolves the approved set into an import table when it instantiates the component.

The declared set resolves once, at instantiation. An extension cannot declare new capabilities during a session. It can gain an ad hoc grant during a session, one specific host or path the user attaches through extension settings or a login flow, and that grant takes effect for subsequent calls without re-instantiation (FR-PERM-18). It can lose a grant, because the user can revoke one; revocation takes effect at the next instantiation, and calls already in flight finish.

The model is deny by default. A capability that is not granted is enforced at the import boundary: the host links every capability interface a world carries, in a denied state, and refuses an ungranted call with a recorded permission error (FR-PERM-3). An interface a world does not import at all is absent from the link, so reaching for it fails at load. This keeps a manifest/code mismatch loud without denying an extension the ability to handle a missing grant at runtime.

Two kinds of failure look different to an extension. An interface the world does not carry is a link failure. A capability that was never granted, or was granted but whose parameters do not cover a specific call, produces a runtime permission error the extension can handle. A network call to an unlisted host is the second kind.

Every denial is recorded. The record holds the extension identity, the capability, the attempted parameter, and the time. `lca ext info <name>` shows the denial count, which is how a user notices an extension trying things it never declared.

## The catalog

### log

Always granted. Never declared in a manifest and never shown at install time.

Import interface: `lca:host/log`. Functions for trace, debug, info, warn, and error, each taking a message string.

Messages go to the agent's diagnostic output under the extension's identity. They never appear in the session log and never reach the model.

The host truncates a message longer than the configured limit. There is no rate limit in 0.1, which is a known gap: a noisy extension can flood the diagnostic output. It cannot flood the interface, because log output is not rendered in the interactive view.

### net

Grants outbound HTTPS to a list of host patterns, each optionally pinning a port. The extension never touches a socket.

```toml
[capabilities.net]
hosts = ["api.example.com", "build.example.com:8443", "*.example-cdn.com"]
```

Import interface: `lca:host/net`. A request function taking a method, a URL, headers, and an optional body, returning a response resource with a streaming body reader.

Pattern rules: an exact hostname matches only itself. A leading `*.` matches one or more labels in that position, so `*.example.com` matches `api.example.com` and `a.b.example.com` but not `example.com`. A bare `*` is rejected at manifest validation. A pattern may pin a port, written `host:port`; a pattern without one grants port 443 only, and HTTPS on any other port needs the pin. A wildcard pattern with a port is rejected at manifest validation, since a wildcard is already the broadest claim in the vocabulary and a port pin is only meaningful on a named host.

HTTPS only. Plain HTTP is refused, including on loopback. Certificate verification is done by the host and cannot be disabled by the extension.

Consent text names every pattern and any pinned port: "Connect to api.example.com and any subdomain of example-cdn.com." "Connect to build.example.com on port 8443."

Outbound requests honor the standard proxy variables (gh #145, pi's `HTTP_PROXY` row): `HTTP_PROXY`/`HTTPS_PROXY` (lowercase honored, uppercase wins), `ALL_PROXY` as the fallback, and `NO_PROXY` for the bypass list. Plain-http targets dial the proxy and send the absolute form; https targets tunnel `CONNECT` (pi's `proxyTunnel` shape, credentials from the proxy URL's userinfo). Read once per client build, so a proxy change needs a restart. The grant and rebinding checks still run first and still resolve the target through direct DNS — a network without direct DNS stays refused, and loopback targets need an explicit `NO_PROXY` entry (`localhost,127.0.0.1`) or they route into the proxy like everything else.

The host resolves every hostname before connecting and checks the resolved address, not just the pattern that matched, against the canonical local ranges listed under `net-local` below. A hostname that matches a granted pattern but resolves to one of those ranges is refused and recorded as a rebinding attempt specifically, distinct from an ordinary denial, because it can indicate DNS manipulation rather than a simple misconfiguration. An extension that legitimately needs a local address declares `net-local` instead; see ADR-0011 for the full reasoning. `net` never connects to an address in the canonical local ranges, regardless of what pattern was granted.

On denial: the request function returns a permission error naming the host that was refused. The extension can handle it. The denial is recorded.

An empty host list is a declaration, not an absence (gh #302). `hosts = []` is the shape an extension ships when every host it may reach arrives by consent — `webfetch` is the worked example — and the host distinguishes it from a manifest that never declared `net` at all. A manifest with no `net` table keeps the flat "not declared" refusal: the declaration gate never becomes a way to ask for a capability the sandbox does not carry.

Where the deny happens, a **user-initiated** call may ask instead of failing (gh #302). The tool path the model or the user deliberately invoked — `webfetch`'s own fetch — offers the ad hoc grant of [FR-PERM-16](#net): the same modal, grant store, deny-rule precedence and yolo answer every other approval uses, with Allow once (this session), Allow always (persisted per project, so later turns and later processes skip the prompt), or Deny. The capability layer's own check still runs first and still refuses a call nobody approved: an approval is what lets a request out, never the absence of a prompt, so a grant-less call never goes out because no prompt was reachable.

The ask belongs to the call, not to the capability. A convenience request — a provider probing an endpoint for its model list, a login flow's discovery — keeps the plain deny and falls back, because it runs on a background thread with no user action behind it and the flow it belongs to has its own consent already. Prompting there would ask one question twice and leave the interface waiting behind a modal nobody triggered.

Headless runs are unchanged: with no prompt installed the ask answers denied, so the error copy names the host and the two fixes (`lca` interactively once, or `--allow-host <host>` for one run) exactly as before.

### OAuth sign-in reaches a second host — for the flow only (gh #418 Phase D)

An MCP server's authorization server is routinely not on the server's own host: the 401's `WWW-Authenticate: resource_metadata` (RFC 9728) or the RFC 8414 issuer names Auth0, Okta, Sentry, or an internal IdP. Those fetches and the token exchange go through the same `net` check as everything else, so what a sign-in may reach is stated here rather than assumed.

Three sources, and only these: the MCP server's own base-URL host; any host the user **configured** in `mcp.json` (`oauth.authServerMetadataUrl`) — configuration is the consent, the same rule the server URL already follows; and hosts **discovered** from the metadata, which are admitted as **session-scoped, OAuth-flow-only** grants. A discovered host is attachable while the flow runs and dropped when it ends, never written to the grant store, and never visible to ordinary tool traffic: the flow-scoped set is consulted only by the sign-in path's own requests, so a `tools/call`-time request to an IdP host is denied exactly as before.

Discovered endpoints are validated before any request leaves: `https://` anywhere, or plain `http://` on loopback only (RFC 8252, and how a local IdP is reached on a developer machine). A discovered `http://` endpoint on any other host is refused with the URL and the rule in the message, and nothing is sent.

The sign-in screen names an outside host: `via <server>: authorization host <host>`. The user sees where the credential is about to go before it goes there. A server whose authorization server is its own host says nothing extra.

Scope: a 401's `scope` member still merges into the next sign-in's request (step-up adds scopes, never drops them), and `resource_metadata` is remembered alongside it so discovery has the hint the server just gave.

A manifest cannot declare a wildcard covering every host; a bare `*` is rejected at install time, and correctly so, since a consent screen that reads "connect anywhere" tells a user nothing they can evaluate. The real case this rejects is a provider extension whose actual host isn't known until the user configures it, such as an extension speaking to whatever OpenAI-compatible endpoint a user points it at. That case is handled the same way an out-of-vocabulary `fs` path is: the manifest declares whatever fixed hosts it has a sensible default for, if any, and the user adds the specific host they actually want as an ad hoc grant, at the point they configure it, usually the same login or setup flow that asks for the base URL in the first place. The consent shown at that moment names the exact host being added, not a pattern standing in for "anywhere."

### net-local

Grants HTTP or HTTPS, any port, to `localhost` or a loopback address, to a private-use, link-local, unique-local, or carrier-grade NAT range, or to an mDNS-style `.local` hostname. Separate from `net` because the risk shape, the pattern syntax, and the port and scheme rules all differ; see ADR-0011.

```toml
[capabilities.net-local]
addresses = ["127.0.0.1", "192.168.0.0/16", "100.64.0.0/10", "*.local"]
```

Import interface: the same `lca:host/net` request function `net` uses. Dispatch between the two grants is stated once here and applies everywhere: if the request's host matches a `net` pattern, `net` rules govern the call, and a local resolved address is refused as a rebinding case even when a `net-local` grant would also cover it (FR-PERM-13). `net-local` is consulted only when no `net` pattern matches the request's host, and a non-`.local` hostname is checked against a CIDR entry by resolved address.

Pattern rules: a literal address, a CIDR range, `localhost`, or a `.local` hostname. A hostname outside `.local` matches a CIDR entry by resolved address at request time. The host validates every entry against the canonical local ranges at manifest install time and refuses an entry that names an address outside them (FR-PERM-14); `net-local` cannot be used to smuggle general internet access.

The canonical local ranges, the one normative list for `net-local` grants and for `net`'s rebinding check: IPv4 loopback `127.0.0.0/8`; IPv6 loopback `::1/128`; IPv4 private use `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`; IPv4 link-local `169.254.0.0/16`; IPv6 link-local `fe80::/10`; IPv6 unique-local `fc00::/7`; IPv4 carrier-grade NAT `100.64.0.0/10`, which is the tailnet case, a model server reached over WireGuard or Tailscale rather than over the local wire. IPv4-mapped IPv6 addresses are normalized to IPv4 before any range check (FR-PERM-17).

This capability reaches further than the machine the agent runs on. A private network can include other people's devices, particularly on a shared or public network with local routing enabled, and the carrier-grade NAT range reaches a tailnet. Consent text says so rather than implying "this machine only": "Connect to a device on your local network, your own machine, or your private tailnet."

On denial: the request function returns a permission error. An address outside the canonical local ranges and not covered by a `.local` grant is refused the same way an unmatched `net` pattern is, since `net-local` is not a general escape from `net`'s host restrictions.

### fs

Grants access to named filesystem scopes. See ADR-0005 for why the vocabulary is fixed.

```toml
[capabilities.fs]
workspace = "read"
private = "read-write"
home-config = "read"
```

Import interface: `lca:host/fs`. Open, read, write, list, and stat functions taking a scope name and a path relative to that scope.

| Scope | Resolves to | Typical use |
|---|---|---|
| `workspace` | The project root the agent was opened in | Reading source files, writing generated output |
| `private` | A per-extension directory under the user data directory | Caches, indexes, extension state |
| `home-config` | The platform configuration directory | Reading a login an existing command line tool already wrote |
| `temp` | A per-session temporary directory, removed at exit | Large scratch files that should not appear in the source tree |

Modes are `read` or `read-write`. A scope absent from the manifest is absent from the grant. Each scope resolves to the per-platform path listed in `docs/platform-notes.md`.

`private` and `temp` both resolve *under* the user data directory (`<data>/private/<name>` and `<data>/tmp/<session>`), which the state-directory exclusion otherwise refuses wholesale. They are the two sanctioned exceptions, each confined to its own root: neither can reach `sessions/`, `credentials/`, or the extension tree, and a path that escapes its root is still refused (gh #344 fixed `temp`, which had never been given the exemption and was therefore unusable on a real installation).

A file staged for the model to read — an extension's large binary payload — goes through the `temp` scope rather than a shared location: `fs_write_new` creates it `create_new` (so an entry planted at the path is an error, never followed) with owner-only permissions on both the file and its directory, and the session's temp directory is removed at exit. The directory is created owner-only and symlink-refusing as well: `ensure_session_temp` sets `0700` on both `<data>/tmp` and the session directory at creation, and refuses a path that already exists and is not a directory, so a link planted before the first stage fails loudly instead of silently becoming the session's temp root. (pi applies the same `0700` discipline to its extension temp folder but does not check for a planted link; LCA's guard is deliberately stricter, because the scope engine's own confinement only fires once something resolves a path.)

Resolution goes through preopened directory handles. The host never joins a guest string onto a base path. A path that leaves its scope through a parent traversal or a symbolic link is refused, including when the link is created after the grant. A path that enters the agent's own state directory, where sessions, the extension tree, and the credential store live, is refused the same way, under every scope and every ad hoc grant - except `private` and `temp`, whose own roots sit inside it and whose confinement to those roots is what keeps the refusal meaningful everywhere else.

Consent text names the scope in plain words: "Read files in this project. Read and write its own private data directory. Read your configuration directory."

Scope grants are unchanged by the sensitive-read escalation (gh #215):
that escalation lives in the model tool-call permission path, where
credential filenames (`.env`, private keys, tokens) prompt even
in-workspace. Extension scope reads carry the install-time grant as
before.

On denial: the call returns a permission error. A path escape attempt is recorded separately from an ordinary denial, because it indicates either a defect or an attack.

Beyond the four scopes, a user can attach an additional path as an ad hoc grant, outside the vocabulary entirely, at install time or later through the extension's settings. The manifest cannot request this; it is something the user adds, deliberately, when a specific extension needs a specific path the fixed vocabulary doesn't name. The consent screen for an ad hoc grant names the exact path and mode, never a pattern, since there is no vocabulary entry to summarize it under.

### env

Grants read access to **named** host environment variables. The manifest lists the names; the host copies the values that are present into the extension's sandbox at instantiation.

```toml
[capabilities.env]
keys = ["DEEPSEEK_API_KEY", "LCA_LOG_LEVEL"]
reason = "Reads the keys its own documentation tells users to export."
```

Import interface: none of its own. A component reads the variables through `wasi:cli/environment`, which WASI already provides and which answers `NotPresent` for anything the context does not hold. That is deliberate: the grant is enforced when the host builds the `WasiCtx`, so there is no capability call an extension could reach around, and no new interface for the sandbox to carry.

Exact names only. A wildcard (`FOO_*`, `*`) is refused at manifest validation, because the names *are* the consent: "Read `DEEPSEEK_API_KEY` from your environment" is a sentence a user can evaluate, while "read any variable" says nothing. This is the same rejection `net`'s bare `*` gets, for the same reason.

Never the ambient environment. The host forwards only what the manifest declared plus what the user approved; not even `PATH` is included. Forwarding the environment wholesale would be both an information leak and a behavioural difference between a compiled-in extension (which sees the real environment) and a sandboxed one.

**Absence is not emptiness.** A declared variable that is unset on the host is *absent* inside the extension, never an empty string. A set-but-empty variable is present and reads as empty. The distinction matters because an empty API key that looks present is how a provider sends a blank credential instead of failing honestly.

**Declared is not granted.** The manifest declares the names; the user approves them at install time, and a declared name outside the approved set is absent — exactly as with every other capability. Until the install flow collects per-key approval, the declared set is the granted set, the same phase-5 note that applies to `fs` and `net`.

Declared in the manifest with `keys = []` is a declaration that grants nothing, the shape an extension ships when every variable arrives by consent (the `net hosts = []` precedent, gh #302).

Consent text names every key: "Read DEEPSEEK_API_KEY from your environment."

On denial: the variable is simply absent. There is no error, because a component cannot distinguish "not granted" from "not set" — and should not need to.

Values never enter the session log, never appear in a session export, are never written to the diagnostic log, and are not shown by `ext info` beyond their names. A granted variable is a *granted disclosure*, the same rule `credentials` follows: the extension was given the secret, so the host must not also write it down.

This capability exists because a sandboxed component gets an empty environment while a native-linked extension reads the process directly (gh #170). Before it, the same provider worked compiled-in and broke as a `.wasm`, and `lca-cli` carried a hardcoded workaround for one provider's base-URL variable. See [ADR-0048](adr/0048-capability-gated-environment.md).

### credentials

Grants read and write access to one namespace in the credential store. The namespace is the extension's own identity and cannot be another extension's.

```toml
[capabilities.credentials]
namespace = "example-provider"
```

Import interface: `lca:host/credentials`. Get, set, and delete functions taking a key within the namespace.

The store prefers the platform keychain where one exists and falls back to a file with owner-only permissions. The extension cannot tell which backend is in use and cannot choose.

Values never enter the session log, never appear in a session export, and are not readable through any other capability. The `fs` capability cannot reach the credential file even with `home-config` granted, because the store lives in the agent's own state directory, which the host excludes from every `fs` resolution.

Manifest validation rejects a namespace that does not match the extension name. There is no cross-namespace read at any privilege level.

Consent text: "Store and read its own saved credentials."

On denial: get returns an empty result rather than an error, so an extension checking for an existing login does not need to distinguish denial from absence. Set and delete return a permission error.

### resources

Always granted. Never declared in a manifest and never shown at install time (like `log`). This is the extension's own read-only package data bag (ADR-0030), not an `fs` grant: the path resolves inside the calling extension's own `resources/` tree only.

Import interface: `lca:host/resources`. `list-resources(prefix)` returns `(path, size)` entries; `read(path)` returns the file's bytes.

Gating: no traversal (`..`, absolute, NUL), no symlink escape, no cross-extension read. A path that leaves the tree is a recorded permission error. A per-call read cap (1 MB) keeps a hostile package from pulling the host into memory games. Both delivery modes serve the same bytes from the same engine seam: an installed package reads its files, a compiled-in extension serves its embedded `include_bytes!` table (ADR-0032).

Conventions (ADR-0030): the host also reads some resource kinds for host-side features - `resources/skills/<name>/SKILL.md` is the standard Claude skill format, merged into the prompt with attribution. The manifest declares kinds (`resources = ["skills", "provider-presets"]`); the installer refuses an undeclared kind and shows the counts in consent.

On denial: a traversal, absolute path, or missing file returns a permission or not-found error; the attempt is recorded.

### state

Always granted. Never declared in a manifest and never shown at install time (like `log`). The extension's own mutable, non-secret data bag (ADR-0030) - caches, last-used values, counters.

Import interface: `lca:host/state`. `read(key)` returns `option<list<u8>>` (absence is `none`, like `credentials.get`); `write(key, value)`; `delete(key)`; `list-keys()`.

Gating: the namespace is the extension's own identity, never guest input, so a cross-extension read has no address to take (FR-PERM-6/7). Keys are restricted to a safe filename charset. A per-value cap (4 MB) and a per-namespace cap (16 MB) bound it. State is wiped on uninstall, shown in `ext info`, and cleared with `lca ext state clear <name>`.

State is **not secret-grade**: secrets go in `credentials`. It never enters a session log or an export.

On denial: a bad key or an over-cap write returns an invalid error; a read of an absent key returns `none`.

### oauth

Grants the loopback authorization flow. The extension never binds a port.

```toml
[capabilities.oauth]
redirect_path = "/callback"
```

Import interface: `lca:host/oauth`. Three functions: `begin`, returning a redirect URL and a flow handle; `open`, which opens a URL in the user's browser; and `await`, taking a handle and returning the callback parameters or a timeout.

The host picks the port, binds on the loopback interface only, serves a minimal response page, and stops the listener when the flow completes or times out. The default timeout is 300 seconds.

`await` is a host wait, so it is cancellation-aware: the host's interrupt flags the capability engine and the wait polls that flag in short slices, returning promptly instead of sitting out the callback window. This is the general rule for a host import that can wait longer than the cancellation budget (NFR-21's 50 milliseconds): poll the flag, never block for the whole window, because an epoch bump only fires at a guest code point and cannot reach host code that is already blocked. The `net` request paths follow the same rule: a request blocked waiting for a response head, and each streaming body read, return promptly on a cancel instead of sitting out their 300-second window.

The extension builds the authorization URL itself, including the code challenge, and opens it through `open`, which is why opening a browser needs no separate capability. It receives the parsed query parameters from the callback. Token exchange happens over the `net` capability, so an extension using `oauth` needs `net` as well.

Consent text: "Open a browser sign-in and receive the response on a local port."

On denial: begin returns a permission error.

### process

Grants the right to ask the host to run a command. The command passes through the same permission prompt as a command the model requests.

```toml
[capabilities.process]
reason = "Runs an external tool server over stdio."
```

Import interface: `lca:host/process`. A spawn function taking a program, arguments, and a working directory scope, returning a handle with stdin, stdout, and stderr streams.

The extension cannot bypass the user prompt. Holding `process` means the extension may ask; it does not mean commands run without approval. A pre-approved pattern in the user grant store satisfies the prompt the same way it does for a model-requested command.

The working directory is a scope name from the `fs` vocabulary, so an extension cannot run a command in a directory it cannot otherwise see.

The `reason` field is required and shown verbatim at install time, because a bare "can run commands" prompt gives a user nothing to evaluate.

Consent text: the reason string, followed by "Each command still asks for your approval."

On denial: spawn returns a permission error. A user declining an individual command prompt also returns a permission error, and the extension cannot tell the two apart.

### pty

Grants allocation of a pseudo-terminal for an interactively spawned program, rather than the plain pipe-backed streams `process` provides. See ADR-0016.

```toml
[capabilities.pty]
reason = "Runs an interactive session inside a terminal panel."
```

Import interface: `lca:host/pty`. A spawn function taking a program, arguments, a working directory scope, and initial dimensions, returning a handle with a byte stream carrying the terminal's output and a function to forward keystrokes and resize events. The PTY itself is allocated host-side; the extension's code never leaves the sandbox to get it.

This is the capability that makes something tmux-shaped buildable as a genuine WASM extension: combined with `ui`'s panel region, the extension relays keystrokes in, renders output through the widget tree, and only the spawned program itself runs unsandboxed, the same as any command run through `process` already does.

The `reason` field is required for the same reason it is on `process`: a bare "allocate a terminal" grant gives a user nothing to evaluate.

Consent text: the reason string, followed by "This gives it an interactive terminal session."

On denial: spawn returns a permission error.

### ui

Grants the right to render in named regions. See ADR-0003 for the rendering model.

```toml
[capabilities.ui]
regions = ["status-line", "panel"]
```

Import interface: `lca:host/ui`. Functions to request a redraw and to close a modal the extension opened. Rendering itself is an export on the extension, not an import.

Host dialogs (`lca:host/ui-dialogs`: `confirm`, `select`, `input`, `notify`) need no `ui` grant: every question is its own consent on host chrome. The widget vocabulary is `text`, `styled-text`, `markdown`, `button`, `table`, `scroll-container`, `image`, `boxed` (title, border role, background tint), `row`, `column`, `spinner`, `progress`, `keyvalue`. Clicks map on the modal and panel regions; footer and status-line stay display-only.

| Region | Constraint |
|---|---|
| `status-line` | One segment, single line, width-limited |
| `footer` | Up to three lines above the input editor |
| `panel` | A side column, shown when the user opens it |
| `modal` | Full-screen dialog, one at a time, user-dismissible |

An extension cannot open a modal during a running turn without the user having invoked it. This stops an extension from interrupting work.

Text spans carry data. The host strips control characters before drawing, so an extension that returns escape sequences sees them rendered as literal text.

Consent text names the regions: "Show a segment in the status line and content in the side panel."

On denial: the extension's render export is never called.

### completion

Grants the right to ask the host for a response from whichever provider is currently active, rather than the extension making its own model call. Added in 1.0 by ADR-0015, having been deliberately deferred pending a real forcing case; the default compaction strategy's need to summarize well is that case.

```toml
[capabilities.completion]
reason = "Summarizes older parts of the conversation when compacting."
```

Import interface: `lca:host/completion`. A request function taking a message list and returning a response, using the same typed shape the `provider` world's own streaming events resolve into. The host records the usage and cost of each request on the session record that caused it, a `compaction` record for a summarization call, so the spend shows up in session cost. Tool-driven completions attribute the same way: the turn installs its own backend into handles that carry none, stamps each call's spend onto its result (`extras.usage`), and folds it into the turn totals the stats sum (#49). Every request the host makes through this capability carries an explicit generation budget (`4096` tokens, riding the request's non-structural extras as `max-tokens`), so an endpoint's own default never decides when a summarization is cut; the strategy pairs it with a bounded, structured summary prompt, and a generation that still ends on the cap surfaces as a failure naming the number (gh #169).

This keeps the extension graph a star with the host at the center: an extension holding `completion` never calls another extension directly, it asks the host, and the host routes to the active provider. This is the reasoning already established in ADR-0008 for why extension-to-extension calls are not supported.

The `reason` field is required. Consent text uses it directly: the reason string, followed by "This lets it ask the current model for a response."

On denial: the request function returns a permission error.

### tools

Grants the right for one tool to call another through the host (gh
#77): orchestrator suites that coordinate other tools, the one case
that justified revisiting ADR-0008's no-extension-to-extension rule.
The star keeps the host at the center — the extension never calls
another extension directly, it asks the host, and the host runs the
nested call through the same validation, hooks, and permission
checks as a model-issued call.

```toml
[capabilities.tools]
reason = "Coordinates the suite's helper tools through nested calls."
```

Import interface: `lca:host/tools`. `execute-tool(name, arguments)`
runs one callable tool now (direct tools while active, codemode and
deferred tools whenever registered; never hidden or model-only
tools) and answers its result, never a rejection: unknown tools,
blocks, and failures all arrive as error results. The host assigns
`<parent id>/<n>` call ids, at most eight levels deep. `list-tools`
lists the callable tools; `set-active-tools` replaces the session's
active set (registered names apply, unknown names are ignored and
reported); `get-active-tools` reads it back.

The `reason` field is required. Consent text uses it directly: the
reason string, followed by "This lets it list, activate, and call other
tools in the session."

On denial: every function answers the refusal. `execute-tool`
returns it as an error result (a guest cannot distinguish a denied
call from a failed one, by design); the refusal is recorded
(FR-EXT-9) either way.

## Resource hints

A manifest may also carry a `[limits]` table, which is **not** a
capability and grants nothing: it hints at resource ceilings the host may
lower. `memory_mb` is mebibytes (1..512, default 64) and `fuel_per_call`
is the execution budget for one call. The table applies whether or not the
manifest declares `[capabilities]`. See the authoring guide's "Capabilities
in practice" for the practical advice.

## Capabilities deliberately absent from 0.1

These come up in design discussion and are not in the first release. Each needs evidence from Phase 3 before it is added, following the pattern that already justified adding `net-local`, `pty`, and `completion` during design: a real, motivated case, not a speculative one.

Filesystem watching. Small and obvious in shape, and nothing in the 1.0 extension set needs it.

Outbound TCP to non-HTTPS endpoints on the public internet, with a host allow list. The main case is database and version control protocols. `net-local` covers the same transport gap for private-network addresses specifically; this entry covers the public-internet equivalent, which remains unadded because nothing in the 1.0 extension set needs it.

Clipboard access. Frequently requested in similar systems and rarely necessary, since a command can print text the user copies.

Long-lived background tasks that outlive a turn. This needs a lifecycle model that 0.1 does not have.

## Adding a capability

A new capability needs an ADR, an entry in this catalog, a section in the manifest schema, consent text, a denial behavior, an entry in the conformance extension, and a threat model update. The conformance entry has to cover the granted case, the denied case, and the parameter-mismatch case.

A capability that cannot be explained in one sentence of consent text is too broad. Split it or narrow it.
