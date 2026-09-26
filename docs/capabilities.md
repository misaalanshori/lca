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

The host resolves every hostname before connecting and checks the resolved address, not just the pattern that matched, against the canonical local ranges listed under `net-local` below. A hostname that matches a granted pattern but resolves to one of those ranges is refused and recorded as a rebinding attempt specifically, distinct from an ordinary denial, because it can indicate DNS manipulation rather than a simple misconfiguration. An extension that legitimately needs a local address declares `net-local` instead; see ADR-0011 for the full reasoning. `net` never connects to an address in the canonical local ranges, regardless of what pattern was granted.

On denial: the request function returns a permission error naming the host that was refused. The extension can handle it. The denial is recorded.

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

Resolution goes through preopened directory handles. The host never joins a guest string onto a base path. A path that leaves its scope through a parent traversal or a symbolic link is refused, including when the link is created after the grant. A path that enters the agent's own state directory, where sessions, the extension tree, and the credential store live, is refused the same way, under every scope and every ad hoc grant.

Consent text names the scope in plain words: "Read files in this project. Read and write its own private data directory. Read your configuration directory."

On denial: the call returns a permission error. A path escape attempt is recorded separately from an ordinary denial, because it indicates either a defect or an attack.

Beyond the four scopes, a user can attach an additional path as an ad hoc grant, outside the vocabulary entirely, at install time or later through the extension's settings. The manifest cannot request this; it is something the user adds, deliberately, when a specific extension needs a specific path the fixed vocabulary doesn't name. The consent screen for an ad hoc grant names the exact path and mode, never a pattern, since there is no vocabulary entry to summarize it under.

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

Import interface: `lca:host/completion`. A request function taking a message list and returning a response, using the same typed shape the `provider` world's own streaming events resolve into. The host records the usage and cost of each request on the session record that caused it, a `compaction` record for a summarization call, so the spend shows up in session cost.

This keeps the extension graph a star with the host at the center: an extension holding `completion` never calls another extension directly, it asks the host, and the host routes to the active provider. This is the reasoning already established in ADR-0008 for why extension-to-extension calls are not supported.

The `reason` field is required. Consent text uses it directly: the reason string, followed by "This lets it ask the current model for a response."

On denial: the request function returns a permission error.

## Capabilities deliberately absent from 0.1

These come up in design discussion and are not in the first release. Each needs evidence from Phase 3 before it is added, following the pattern that already justified adding `net-local`, `pty`, and `completion` during design: a real, motivated case, not a speculative one.

Filesystem watching. Small and obvious in shape, and nothing in the 1.0 extension set needs it.

Outbound TCP to non-HTTPS endpoints on the public internet, with a host allow list. The main case is database and version control protocols. `net-local` covers the same transport gap for private-network addresses specifically; this entry covers the public-internet equivalent, which remains unadded because nothing in the 1.0 extension set needs it.

Clipboard access. Frequently requested in similar systems and rarely necessary, since a command can print text the user copies.

Long-lived background tasks that outlive a turn. This needs a lifecycle model that 0.1 does not have.

## Adding a capability

A new capability needs an ADR, an entry in this catalog, a section in the manifest schema, consent text, a denial behavior, an entry in the conformance extension, and a threat model update. The conformance entry has to cover the granted case, the denied case, and the parameter-mismatch case.

A capability that cannot be explained in one sentence of consent text is too broad. Split it or narrow it.
