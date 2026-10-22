# ADR-0049: An inbound HTTP capability that the host owns, Mode A only

Status: accepted.

Date: 2026-10-22.

## Context

Every network capability so far points *outward*. `net` and `net-local` make
requests; the extension never touches a socket, and the host resolves, checks,
and connects on its behalf (ADR-0011). `oauth` is the one exception in shape — it
binds a loopback listener — but it is not a capability in the ordinary sense: the
listener exists for a few hundred milliseconds to receive one redirect, it serves
a fixed page the host wrote, and the extension never sees the request.

gh #363 asks for the general form: an extension that serves content to a browser.
A mirror of the terminal in a browser tab, a webhook receiver, a local
documentation viewer. The issue proposes two operating modes, path-routed
namespaces, a dashboard, WebSocket bridging, and a settings-ladder port.

All of that is one phase too much for one change, and only one part of it is
security-critical. This record covers **Mode A: one loopback listener per
process, path-routed, token-authenticated, prefix-stripped.** Everything else is
named in the consequences as deliberately deferred, because everything deferred
is additive on top of the listener and its auth, and none of it can be built
safely before those exist.

## Decision

**The host owns the listener; the guest answers a request.** An extension holding
`serve-http` never binds a socket and never accepts a connection. The host reads a
request off the wire, routes it, and calls the extension's handler with a
`request` record; the extension returns a `response` record. That is the `net`
shape inverted, and it is what keeps the sandbox promise (ADR-0007): the socket,
the port, the accept loop, and the framing are all host state, and an extension
cannot reach any of them.

### 1. Loopback, chosen by the host, not declared by the manifest

The listener binds `127.0.0.1` and nothing else. There is no `bind` key and no
`0.0.0.0` consent path in this phase: a capability that *can* name a routable
address is a different capability, with a different threat story, and allowing it
"for symmetry" would ship the risky half before the safe half was known to work.
The one place a manifest cannot argue its way out of.

### 2. A per-process token is mandatory

This is the decision the security analysis forces, and it is worth stating
plainly because the alternative looks harmless.

A loopback listener is not private. Three separate things can reach
`http://127.0.0.1:<port>/`:

1. Any process running as any user on the machine, including a sandboxed
   application the user did not write.
2. Any web page the user has open in any browser, because a page may issue a
   cross-origin `POST` with a simple content type, and it may open a WebSocket
   handshake, and neither requires CORS preflight. The page cannot read the
   response — but it does not need to read anything for a state-changing `POST`
   to have happened.
3. Anything that can forge a `Host` header, which is the DNS-rebinding case: an
   attacker-controlled name resolves to `127.0.0.1`, the browser sends the request
   to the listener believing it is talking to the attacker's own origin, and
   loopback-only binding has done nothing to stop it.

The listener serves *extension* content that can act on the user's workspace
through the extension's other grants. So binding loopback is a necessary
condition for this capability, never a sufficient one.

The token answers (1) partly and (3) completely, and it is the only one of the
three the host can address without a browser-side story:

- The host generates a random token per process, once, and requires it on every
  request — accepted either as a `?token=` query parameter or an
  `Authorization: Bearer` header, so a browser navigation (which cannot set a
  header) and a `fetch` (which can) both work.
- A request without the token is refused **before** routing, before the extension
  is called, and before the path is even parsed for a namespace. The refusal names
  no extension and reveals no session id.
- A request whose `Host` header is not `127.0.0.1:<port>` or `localhost:<port>` is
  refused as a rebinding attempt, and recorded *as such*, distinctly from an
  ordinary denial — the same separation `net` makes for a resolved-address
  mismatch (ADR-0011, FR-PERM-13). This is the check that closes (3), and it is
  cheap because the host knows its own port.
- The token is **never logged**, never written to the session record, never
  printed by `ext info`, and never rendered into a log line a diagnostic dump
  would carry. It is displayed once, at session start, in the notice line, which
  is where the user needs it and the only place it belongs. A token that leaks
  into a session log is a token an extension holding `fs` can read back, which
  would make the whole gate theatre.

Item (2) is reduced but not eliminated: a cross-origin `POST` from an open page
does not know the token, so it is refused. That is the honest statement — the
token is what makes the drive-by case fail, and there is no second control in
this phase that would also make it fail if the token leaked. A same-origin-policy
story (`Origin` checking, `SameSite` cookies) belongs with the dashboard in a later
phase, when there is a browser session to hold.

### 3. Routing is path-based, and stripping is the contract

`/<session-id>/<extension-name>/<rest>` — the host strips the session and
extension prefix and hands the extension `<rest>`. `index.html` and
`./style.css` then work without the extension knowing where it is mounted. The
alternative the issue offers, injecting `X-Forwarded-Prefix` and letting the
extension build its own URLs, requires every extension to implement the same
base-path logic correctly; picking stripping means one implementation in the host
instead of N in extensions. `X-Forwarded-Prefix` is still *sent* (it costs one
header and helps an extension that does use it), but stripping is what the
contract promises.

### 4. No new WIT import for the listener — but one for the handler

Design point 4 of the brief asks whether an existing world function can carry a
request in and a response out. It cannot, and the reason is worth recording
because "just add a function somewhere" would have been the wrong answer:

- The new world is `serve-http`. Its own export must be a handler, and a world's
  exports are its *reason to exist* — there is nowhere else to put it. So the new
  world adds an export, which is expected and is not the thing being argued.
- On the **host** side, the extension calls nothing to serve a request: the host
  calls the extension. So the new world imports nothing new at all beyond the
  existing `log`. The capability is a *grant on an export*, which is unusual in
  this ABI — every other capability is an import the host satisfies — and it is
  the correct shape here, because the direction of initiation is inverted.
- What the extension *does* need is a way to know its own URL. That arrives as a
  string argument to the handler (`base-url`), never by the guest constructing it
  or reading it from the environment. A guest that builds its own URL has to know
  the port, and a guest that knows the port is one that will eventually be handed
  a routable one.

So: one new world, one new host-side interface for a **single already-bundled
import** — `wasi:http/types` would be the WASI-standard way to carry a request
across, and it is the one place this record abandons a plain record in favour of a
look, because the request/response shapes are already standardized. See
"Alternatives considered" for why that was rejected in favour of plain records.

### 5. Bind failures degrade

The default port is `8080`. If it is occupied, try `8081..8089`. If none is free,
the capability reports **unavailable**, the extension's handler is never called,
and the session continues normally. The agent must not fail to start because a
browser UI could not listen — a web view is a convenience, and a hard dependency
on it would make an unrelated port collision into an outage.

The port that was actually bound is what the notice shows. A user told "8080"
while the listener is on 8083 has been told something false, so the notice is
rendered from the bound address, never from the configured default.

### 6. One listener per process, shared by every granted extension

Per process, not per extension and not per session-instance. A second listener
would mean a second port to tell the user about, a second token, and a second
accept loop for no gain: path routing already separates extensions. The
`<session-id>` segment is in the path because a browser tab survives a session
restart and a stale tab should 404 rather than silently reach a different
session's extension.

### 7. Shutdown is clean

The listener stops with the session. A process that exits leaves no bound port,
which is pinned by a row that binds and drops the listener and re-binds the same
port.

## Consequences

**What this enables.** An extension can serve a browser UI, a webhook receiver, or
a documentation page, with the host owning every part of the socket. Combined with
`ui`'s panel region and `pty`, a browser mirror of the terminal becomes buildable
as a genuine WASM extension — the shape #168 needs. The `serve-http` grant is the
prerequisite, and nothing else.

**What is deliberately absent, and why each can wait.** All of these are additive
on top of a listener that exists and is authenticated; none of them can be built
safely before it does, and each is its own phase with its own argument:

- **Mode B (dedicated port, settings-ladder port).** A dedicated port is for
  standard protocols and webhooks, where the extension needs `/` or a fixed port.
  It needs a settings key, a second consent shape, and a story for what a fixed
  port means across two sessions. The listener does not change.
- **WebSocket bridging.** Real work (a framing layer, an upgrade handshake, a
  per-frame marshalling decision) and the fast path that skips WASM marshalling
  is a performance design, not a security one. The `Upgrade` request is refused
  cleanly today, which is the correct behaviour for a phase that does not bridge.
- **The root dashboard** at `/`, listing sessions and mounted extensions. It needs
  a browser-session story (cookie, `SameSite`, CSRF) to be worth having, and
  serving it before that story exists would put an unauthenticated index of the
  user's sessions on a port we already decided is not private.
- **`X-Forwarded-Prefix` as an *alternative* to stripping.** Rejected as an
  alternative rather than deferred: two contracts for the same thing means every
  extension has to pick one and the host has to support both. The header is sent;
  stripping is the promise.
- **The terminal-engine fast path** from #168 that mirrors ANSI directly into the
  terminal widget tree without per-frame WASM marshalling. That is a performance
  optimisation for one consumer, and it should be designed against a working
  naive path, not instead of one.

**A `serve-http` extension is a local server, and the consent says so.** The
install line reads: "Serve a web interface on this machine, reachable by any
program on it, at http://127.0.0.1:8080/." A user approving that should understand
that "on this machine" is not "private to LCA". A capability whose consent text
implies more privacy than it delivers is worse than no capability.

**The token is a secret with a display path and no storage path.** It is generated
per process, held in memory, shown in the notice, and dropped at exit. It is not
persisted, so a restart means a new URL, which is the right answer: a persisted
token is a token in a file, and a file is readable by whatever the user's next
`fs`-holding extension is.

**The native twin has no listener.** A compiled-in extension implementing
`serve-http` answers the same handler through the same host, so this is one
implementation with two delivery modes rather than two listeners — the same
arrangement `net` has, and the reason the conformance diff can compare them at all.

**Untested by this phase, and named rather than implied.** TLS on the listener
(there is none, and `http://` is the honest scheme for loopback), rate limiting
(a hostile local process can hammer the port; the extension's own fuel budget is
what bounds each call), and concurrent request handling beyond a bounded number of
in-flight connections. These are the places a later phase should look first.

## Alternatives considered

**Let the extension bind its own socket.** It is what every other runtime does, and
it is the wrong answer here for the reason ADR-0007 gives about escape hatches: a
capability that hands out a listening socket hands out all of the above problems
at once, with no place for the host to enforce anything. The extension could bind
a routable address, could skip auth, could ignore `Host`. Rejected.

**Bind loopback and skip the token.** The tempting simplification, and it is wrong
because of the web-page case: a page the user already has open can `POST` to
`127.0.0.1` without a preflight. Loopback is an address, not an authorization
boundary. Rejected.

**Use `wasi:http/incoming-handler` instead of a new world.** This is the
standard-shaped answer and it deserves a real argument, not a dismissal. It was
rejected for three reasons: it puts the request body behind `wasi:http/types`'
own resource machinery, which means the host must implement a second body type
with its own limits and its own streaming semantics alongside the one `net`
already has; it carries a large amount of surface (outgoing bodies, trailers, and
a `future-incoming-response` type) that this capability will never use; and it
makes the capability *invisible* — a component that exports
`wasi:http/incoming-handler` looks like any other WASI HTTP component, so nothing
in the manifest or the install screen would say "this listens on your machine".
The last point is decisive for the same reason `net`'s bare `*` is refused: the
consent screen must be able to say what is being approved. A named world
`serve-http` with a manifest key of the same name makes the capability legible.
Rejected, with the note that a future WASI-native path should be revisited if
`wasi:http` ever grows the "this component wants to listen" declaration it
currently lacks.

**A single `mode` field with `"shared"` and `"dedicated"` from day one.** That is
the issue's shape, and it means shipping a validated-but-unimplemented value: a
manifest could declare `mode = "dedicated"` and get shared behaviour, or an error.
Two modes are two consents and two port policies. Phase A ships one shape and no
`mode` key at all; adding the key later is additive, and adding a meaning to a key
that already exists is not. Rejected as premature.

**Per-extension tokens instead of one per process.** A token per extension would
narrow the blast radius if one leaked, at the cost of N strings in one notice line
and N ways for a user to copy the wrong one. One listener, one token, one URL to
show. Revisit if a real case needs per-extension revocation.

## Guards

- `tests/regressions/363-inbound-http-capability.rs` — the NFR-24 guard: the
  loopback-only bind, the mandatory token, the `Host` rebinding refusal, the
  prefix stripping, the port fallback, the no-free-port degradation, the denial
  path, and clean shutdown.
- Conformance in `extensions/conformance` — granted, denied, and
  path-mismatch in both delivery modes (NFR-25).
- `crates/lca-ext-host`'s world-declaration test, which refuses a manifest naming a
  world the vocabulary does not hold.
- The catalog's `every_schema_capability_has_a_consent_line` row, which fails if
  `serve-http` reaches the schema without a consent sentence.

## Revisit conditions

Mode B gains a real, motivated consumer (an extension that genuinely needs `/` or
a fixed port) — that is the evidence bar ADR-0007 sets, and it is what would move
the settings-ladder port from deferred to scheduled. A WebSocket consumer that
cannot be served acceptably by polling moves WebSocket bridging up. Evidence that
the `Origin`/cookie story is needed for the dashboard before the dashboard exists
would mean the token was not sufficient in practice, which would be a reason to
revisit decision 2 rather than to add the dashboard early.
