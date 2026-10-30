# MCP servers

LCA speaks the Model Context Protocol as a client: an external server offers
tools, resources, or both, and they appear in the session as if they were
built-in — through the same permission checks, the same hooks, and the same
transcript.

This guide covers configuration, the transports, OAuth, how the model reaches a
server's tools, the `/mcp` manager, and the `lca mcp` shell verbs. Every command,
default, and path below is taken from the code, not from a design document; where
behavior is surprising it says why.

MCP is a first-class subsystem (`crates/lca-mcp`), not an extension: it sits below
the agent loop and depends on neither `lca-core` nor `lca-cli`. See
[ADR-0047](adr/0047-mcp-first-class-subsystem.md) for why it stopped being one, and
[ADR-0045](adr/0045-mcp-extension-bridge.md) for the superseded design.

---

## Where the configuration lives

Two files, merged:

| File | Scope | Read when |
|---|---|---|
| `~/.lca/mcp.json` | user, every project | always |
| `<project>/.lca/mcp.json` | one project | the project is **trusted** |

The user file is the one to reach for first. The project file is read only in a
trusted folder (the same rule as every other project-level file —
[ADR-0039](adr/0039-folder-trust-and-rules.md)), because a cloned repository that
could configure a command server would be a repository that runs code on open.

In a trusted project, a **user-level** entry that the project file also names is
overridden, not replaced: the project file writes *knobs only* — `enabled`,
`exposure`, `toolExposure`, `description` — while the command, URL, and
credentials stay where the user put them. So a project can say "`github` is
`hidden` here" without a copy of your token appearing in the repository.

```json
{
  "mcpServers": {
    "github": { "enabled": false }
  }
}
```

Server names hold letters, digits, `_`, and `-`. Two names that normalize to the
same namespace (`my-server` and `my_server` both become `mcp__my_server`) are a
conflict: the first loads, the second is skipped with a warning naming both. A
skipped entry never blocks the rest.

---

## stdio servers

A server that runs as a child process. LCA speaks JSON-RPC over its stdin and
stdout.

```json
{
  "mcpServers": {
    "echo": {
      "command": "python3",
      "args": ["/path/to/echo-mcp-server.py"],
      "env": { "API_KEY": "${GITHUB_TOKEN}", "LOG_LEVEL": "warn" },
      "cwd": "~/projects/thing",
      "description": "Echoes text back"
    }
  }
}
```

| Key | Meaning |
|---|---|
| `command` | One executable. Never a shell string — there is no shell in the path. |
| `args` | Arguments, `{NAME}` expansion applied. |
| `env` | Variables overlaid on the inherited environment. `${NAME}` expands; a name that is not set is an error naming that variable. |
| `cwd` | A directory. A leading `~` expands to your home; anything else resolves against the session's working directory. Omitted means the session's working directory. |
| `description` | One line for the system prompt, so the model knows what the server is for without a tool call. |
| `timeout` | HTTP only; ignored here. |

`env` and `args` both expand `${NAME}` against **LCA's own environment**, which is
what makes a server's credential a variable rather than a literal in a file. Two
consequences worth knowing:

- A value that begins with `!` is **refused**, not executed. Pi runs `!command` in
  a config value; LCA does not, because executing configuration content is a trust
  hole — anyone who can write `mcp.json` would gain command execution. The entry is
  skipped with a message saying so. Use an environment variable instead.
- Nothing is expanded at install or edit time; it is expanded when the entry is
  loaded, so changing a variable takes effect on the next session.

**Every spawn is a permission prompt.** A stdio server is a program on your
machine, so LCA asks before running it, showing the executable, its arguments, and
the resolved working directory. It respects folder trust, your allow/deny rules,
pre-approvals in the grant store, and `--yolo`, and it records the decision like
any other command. Holding `mcp` in a config file is not itself permission to run
anything.

If the child dies during the handshake, the failure carries **the tail of its
stderr** — the last 2,000 bytes, on one line. That is usually where the server
explains itself: a Python traceback, a missing token, a bad config. It shows up in
`/mcp`'s detail screen and in `lca mcp list`.

Shutdown closes stdin, then sends `SIGTERM` to the process group, then `SIGKILL`
if the child has not exited. The whole group goes, so a server that spawned
children of its own does not leak them.

---

## HTTP servers

A remote server reached over streamable HTTP.

```json
{
  "mcpServers": {
    "sentry": {
      "url": "https://mcp.sentry.dev/mcp",
      "headers": { "X-Client": "lca" },
      "timeout": 30,
      "oauth": { "scope": "org:read" }
    }
  }
}
```

| Key | Meaning |
|---|---|
| `url` | The endpoint. |
| `headers` | Extra headers. Setting an `Authorization` header **disables OAuth** for that server — a static credential is an explicit choice. |
| `timeout` | Per-request seconds, default 60, capped at 600. |
| `oauth` | See below. Omit for a server that needs no sign-in. |

`type` may be `stdio`, `http`, or `streamable-http`. The legacy `sse` transport is
refused by name rather than silently mis-parsed.

Retries are deliberately narrow. A transient failure — `408`, `429`, or a `5xx`
other than `501` — is retried at 250 ms and then 1 s, giving three attempts in
total, on **idempotent reads only**: `initialize`, `tools/list`, `resources/list`,
`resources/templates/list`, and `resources/read`. A `tools/call` gets exactly one
attempt, because the server may already have performed the side effect and a retry
would perform it twice.

A `401` is not a retry: the token is refreshed once and the request re-sent; a
second `401` is reported with its step-up scopes, which is what moves the server to
`needs-auth`.

A `404` after a session id was established is **not** specially handled today — it
is reported as an error. Reconnect to start a fresh session, which is what
`/mcp`'s `Reconnect` and `/mcp reconnect <server>` do.

---

## OAuth

Most remote servers authenticate with OAuth 2.1. LCA handles the discovery and the
flow; you handle the browser.

```json
{
  "mcpServers": {
    "sentry": {
      "url": "https://mcp.sentry.dev/mcp",
      "oauth": {
        "client_name": "lca",
        "scope": "org:read project:read",
        "client_id": "pre-registered-id",
        "client_secret": "${SENTRY_OAUTH_SECRET}",
        "authServerMetadataUrl": "https://sentry.io/.well-known/oauth-authorization-server"
      }
    }
  }
}
```

Every key is optional:

| Key | Meaning |
|---|---|
| `client_name` | Name sent when registering dynamically. Default `lca`. |
| `scope` | Space-separated scopes to request. A 401's own `scope` merges in (step-up adds, never drops). |
| `auth_server_metadata_url` (also `authServerMetadataUrl`) | Reads this metadata document first, ahead of the challenge pointer and the origin fallback. |
| `client_id` / `client_secret` | A pre-registered client; skips dynamic registration. |

### The flow

1. LCA requests the server. A `401` is a state, not an error: it marks the server
   `needs-auth` and captures the `WWW-Authenticate` challenge — including
   `resource_metadata`, `scope`, and `error`. The challenge is what points at an
   authorization server that is **not** the MCP server's own host.
2. On sign-in, LCA discovers the authorization server, in this order:
   1. the configured `authServerMetadataUrl`, when you set one;
   2. the challenge's `resource_metadata` pointer (RFC 9728), when the server
      supplied one;
   3. otherwise the MCP server's **own origin** as its own issuer, asking it for
      `/.well-known/oauth-authorization-server` (RFC 8414).

   The third step is the last resort, and it is what a server with no RFC 9728
   document and no configuration falls back to. It then registers a client
   (RFC 7591) unless one is configured, and opens the authorization URL in your
   browser.
3. It waits on a loopback listener for the redirect. **On a remote machine the
   loopback can never fire**, so `/mcp`'s sign-in screen and `lca mcp login` also
   accept the redirect URL pasted from the browser you actually used — the SSH
   case. The pasted URL carries a one-time code and is never echoed into the
   transcript or written to a log.
4. The code is exchanged (PKCE S256), the tokens are stored in the `mcp` credential
   namespace (`~/.lca/credentials/`, owner-only), and the server reconnects.

### An authorization server on another host

This is the common corporate case — Auth0, Okta, Sentry, an internal IdP — and it
is the one place MCP reaches a host you did not configure. The rule:

- Three sources may name a host: the MCP server's own base URL, a host you
  **configured** (`authServerMetadataUrl` — configuration *is* the consent), and
  hosts **discovered** from the metadata.
- A discovered host is admitted **for the sign-in flow only**: it is dropped when
  the flow ends, never written to `grants.json`, and never visible to ordinary tool
  traffic. A `tools/call`-time request to that host is denied exactly as before.
- Discovered endpoints are validated before anything is sent: `https://`
  anywhere, or plain `http://` on loopback only (RFC 8252, and how a local
  developer IdP is reached). A discovered `http://` endpoint on any other host is
  refused with the URL and the rule in the message.
- The metadata document is treated as **untrusted input**, because it is the word
  of the server you are signing into. Three refusals follow from that, each
  reported rather than silently narrowed:
  - a **wildcard** authorization server (`https://*.example.com`) is refused — a
    sign-in goes to a host the document states exactly (gh #290);
  - a **loopback** authorization server is refused *when the MCP server is not
    itself loopback*. That is the machine talking to itself on a remote server's
    instruction, which is `net-local`'s business and needs its own consent. A
    local MCP server naming a local IdP is the ordinary development case and is
    allowed;
  - one sign-in may reach at most **8** distinct hosts. A document listing more
    is refused outright rather than truncated, so a listing spree shows up as an
    error instead of a mysteriously failing request later.

The sign-in screen names the outside host — `via <server>: authorization host
<host>` — so you see where the credential is about to go before it goes there.

### The challenge is consumed, not kept

A `resource_metadata` pointer is the server telling you *now* where its
authorization server is, so LCA reads it once. The persisted challenge — the
pointer, the `scope`, the whole `WWW-Authenticate` — is deleted when the sign-in
attempt finishes, **whether it succeeded or failed**. A pointer left behind after a
refusal would steer the next attempt without the server having said anything, and
the pointer is attacker-influenced by construction, so remembering it forever is a
standing instruction from whoever sent the first `401`. A step-up `scope` rides the
same lifecycle.

### Tokens expire

A stored token set is refreshed proactively and reactively. The refresh is
guarded by a cross-process lockfile, so two LCA processes sharing one credential
do not race their refresh and invalidate each other's token. A refresh that the
server rejects outright clears the stored set, which sends the server back to
`needs-auth` rather than looping.

---

## How the model reaches a server's tools

Every tool is namespaced `mcp__<server>__<tool>`, with any character outside
`[A-Za-z0-9_]` replaced by `_`. Two tools on one server that sanitize to the same
name (`do.thing` and `do_thing`), or a name over 64 characters, get a deterministic
8-character hash suffix derived from the server and tool names — so both stay
callable and the suffix is stable across restarts.

**Exposure** decides how much of the model's context a server costs:

| Exposure | What happens | When to use it |
|---|---|---|
| `codemode` | **Default.** The tools are not declared to the model. The server appears in the prompt as one line, and the model reaches its tools through the codemode tool surface. | Most servers. It is the cheapest option that still works. |
| `deferred` | Not declared. The model finds them with `tool_search`, then calls them directly for the rest of the session. | A large server you use occasionally. |
| `direct` | Declared to the model exactly like a built-in tool. | A server you use constantly, whose tool list is small. |
| `hidden` | Registered but unreachable — no declaration, no call path. | Temporarily silencing a server without deleting its config. |

`toolExposure` overrides exposure per tool, with exact names first and then
patterns in order:

```json
{
  "mcpServers": {
    "big": {
      "exposure": "codemode",
      "toolExposure": { "search_*": "direct", "dangerous_delete": "hidden" }
    }
  }
}
```

`*` matches any run of characters. An exact name always wins over a pattern.

`enabled: false` is stronger than any exposure: the server is not connected at
all, so nothing spawns and nothing waits.

### The prompt section

Enabled servers whose exposure is not `direct` (and not `hidden`) get one line each
in the system prompt saying how to reach them, plus their `description` when one is
set. That is the whole surface: `direct` servers do not need a line because their
tools are already declared, and `hidden` and disabled servers have nothing to say.

---

## Resources

A server that offers resources gets three tools, registered whenever a connected
server offers any:

| Tool | What it does |
|---|---|
| `list_mcp_resources` | Lists resources as JSON. With a `server`, one page; without, the first page of every server. `cursor` continues a page. |
| `list_mcp_resource_templates` | Lists URI templates. With a `server`, one server's; without, every server's. |
| `read_mcp_resource` | Reads one resource by URI. |

They register at the **widest** exposure among the servers offering resources, so a
single `direct` server's presence is not lost because a `codemode` server also
offers resources. A server exposed as `hidden` contributes no resources.

Binary resources are staged into the per-session owner-only temp directory and
handed to the model as an attachment, the same path an image from a built-in tool
takes.

MCP App UI resources (`ui://` URIs and anything with `profile=mcp-app`) and icons
are filtered out: they are for a host that renders MCP apps, which this one is not.

---

## The `/mcp` manager

Type `/mcp` in the interactive interface and the manager opens. Three screens.

### The server list

Sorted by what needs your attention, not by config order:

`needs-auth` → `failed` → `disconnected` → `connecting` → `connected` → `disabled`

Alphabetical within each group, so the order is stable between opens. Each row is
`<name> — <state> · <exposure> · <scope>`, where scope is `user`, `project`, or
`user, project override`. Configuration errors appear above the list, because a
warning is usually the reason a server you configured is missing from it.

`Enter` opens one server. `Esc` closes.

### A server's detail

The state, the command line or URL, the file that defines it, the project override
path when one exists, the effective exposure (with the configured one beside it
when they differ), and — the reason this screen exists — **the failure and the
tail of the server's stderr**. A traceback is split back into readable lines and
bounded to 12 rows with a marker saying how many earlier lines were omitted, so a
server in a retry loop cannot scroll the actions away.

The actions depend on the server's state, and an action that would do nothing is
absent rather than greyed:

| Action | When |
|---|---|
| `Sign in` | the server needs a sign-in |
| `Tools (N)` | the server is connected |
| `Reconnect` | always — the safe way to pick up a config change or a crashed child |
| `Sign out` | connected, with OAuth configured |
| `Exposure: <level>` | always — opens the picker |
| `Enable` / `Disable` | always |
| `Enable in this project` | a user-level server in a trusted project |
| `Back` | always |

`Enter` runs the highlighted action, `Esc` goes back to the list with the cursor
still on this server.

### The tool inspector

`Tools` lists every tool the server offers, its **effective** exposure (after the
server default and any per-tool override), and its description wrapped beneath.
This is the only place a per-tool override is visible.

### The exposure picker

Four levels, `●` on the one currently effective. `Enter` applies and persists:
writing to the file that defines the entry, or, for a user-level server in a
trusted project, writing a project override. The picker opens on the effective
value, and the header refreshes so you see the new one rather than the one the
screen opened with.

### The sign-in screen

The authorization URL, unwrapped, ready to read or copy — because you may be about
to open it in a browser on another machine. Below it, a field for the redirect URL
that browser lands on: paste it and press `Enter` to finish a flow whose loopback
callback could not fire. `Esc` cancels, which stops the listener immediately rather
than abandoning it to its timeout.

While the manager is open, background connections keep landing: a server that
finishes connecting updates its row, and the cursor stays on the server it was on —
by name, not by position, so a rename in the sort order does not move your
selection.

---

## The direct verbs

The manager is an addition, not a replacement. These keep working, from the prompt
or a script:

```
/mcp                                               the manager
/mcp list                                          the manager
/mcp reconnect [server]                            reconnect one server, or all
/mcp enable <server>                               switch on
/mcp disable <server>                              switch off
/mcp exposure <server> [direct|codemode|deferred|hidden]
                                                   set exposure; omit the level to cycle
/mcp login <server>                                start a browser sign-in
/mcp logout <server>                               forget stored tokens
```

Each answers with a one-line notice and nothing else, which is what makes them
usable from a recorded session or a scripted pane.

---

## The `lca mcp` shell verbs

For configuration and CI, without opening the interface.

```sh
lca mcp add <server> [options] -- <command> [args...]
lca mcp add <server> [options] --url <url>
lca mcp remove <server> [-l|--local]
lca mcp list [--json]
lca mcp login <server> [--timeout <seconds>]
lca mcp logout <server>
```

### `add`

Writes to the user file, or the project file with `-l`. Adding a **user-level**
server from an untrusted project prints a note that the project file is ignored
until the folder is trusted.

| Flag | Applies to |
|---|---|
| `-l`, `--local` | both — write `<project>/.lca/mcp.json` |
| `--url <url>` | HTTP. Mutually exclusive with `-- <command>`. |
| `--env KEY=VALUE` (repeatable) | stdio |
| `--cwd <dir>` | stdio |
| `--header KEY=VALUE` (repeatable) | HTTP |
| `--bearer-token-env-var NAME` | HTTP — sets `Authorization: Bearer ${NAME}` |
| `--oauth-client-id`, `--oauth-client-secret`, `--oauth-client-name` | HTTP |
| `--oauth-callback-port <port>` | HTTP |
| `--exposure <mode>` | both |
| `--description <text>` | both |

A secret given with `--oauth-client-secret` is stored in the credential namespace,
**not** in `mcp.json`, because the credential store is where a secret belongs and
`mcp.json` is a configuration file you are expected to edit and diff. Note that
`mcp.json` does hold secrets when you put them there yourself — a server's `env`
and `headers` carry tokens — so it is written **owner-only (`0600`)** and
**atomically**: a temp file beside it, restricted before it is published, then
renamed over the target. An interrupted write leaves the previous file intact
rather than truncating your server configs and credentials. On Windows the file
gets the same owner-only ACL the credential store applies.

### `remove`

Removes the server from the target file. If it is not in the requested scope but
*is* in the other one, the message says so and the exit code is `1`, so a typo in
`--local` does not look like success.

### `list`

Connects to every enabled server **in parallel**, collects state, tool names,
per-tool overrides, resource availability, and errors (with the stderr tail), then
closes the connections. Those connections live for exactly one invocation.

Exits `0` when every enabled server connected, `1` when any config error exists or
any enabled server did not. `--json` prints the same report as JSON.

### `login` / `logout`

`login` performs the same flow the `/mcp` screen does, from the shell: it captures
the 401 challenge, discovers, opens the browser, waits for the redirect (or a
pasted one), exchanges, and prints the tool count. `--timeout` bounds the whole
flow.

`logout` deletes the stored credentials for one server.

None of these verbs are gated by `--no-mcp` or the enablement record: a user
debugging a disabled setup needs `list` and `login` to work.

---

## Disabling MCP

Three ways, in increasing persistence:

| How | Scope |
|---|---|
| `--no-mcp` on the command line | one run. Nothing is written. |
| `enabled: false` on a server | that server, until you change it |
| `/ext` or the grant store's `mcp` enablement | the subsystem, this project |

All three are checked **before** any connection is attempted, so a disabled server
does not spawn, does not connect, and does not hold up the turn.

That ordering is the whole contract, and it is enforced at two levels (gh #266).
The decision is read once, early, and **the entries are marked disabled before the
manager is constructed** — so a disabled server cannot reach the code that spawns
it, rather than being filtered afterwards. `/mcp` still lists it (as `disabled`),
because knowing what is configured and switched off is the point of the list.

Anything registered into a **live** registry after startup — the compaction
strategy, on every `/reload` — goes through `registry::register_gated`, which
registers and then applies the enablement decision to that one name. The order
matters because there is no later sweep to rely on: registering first and sweeping
later is exactly how a disabled extension came back to life after a reload. A
disabled extension is inert, not deleted, so re-enabling it takes effect on the
next launch without re-installing anything.

## Startup and the first turn

Server connections run on background workers. `registry::assemble` returns
immediately, so a slow or unreachable server cannot delay the interface (NFR-3's
150 ms cold-start budget). On the **first turn**, LCA waits up to 10 seconds for
enabled servers whose tools are `direct`, because those tools are declared to the
model and a missing declaration would change the answer. `codemode`, `deferred`,
and resource tools are waited for on demand instead. After the first turn, nothing
waits.

While a server is still connecting, `/mcp` says `connecting` rather than guessing:
reporting "failed" for a server that simply has not answered yet would be a lie
that costs a debugging session.

---

## Where things are

| Path | What |
|---|---|
| `~/.lca/mcp.json` | user configuration — written **owner-only (`0600`)** and **atomically**; see below |
| `<project>/.lca/mcp.json` | project configuration and overrides (trusted folders only) |
| `~/.lca/credentials/` | OAuth tokens, owner-only, `mcp` namespace |
| `~/.lca/logs/lca.log` | the agent's own diagnostics (not per-server) |

**Both `mcp.json` files are written atomically and owner-only.** They carry a
server's `env` and `headers` by design, so the process umask must not decide who
can read them: the write goes to a temp file beside the target, is restricted
(`0600` on Unix, an owner-only ACL on Windows — the same call the credential store
uses), and is then renamed over the target. An interrupted write therefore leaves
the **previous file intact** rather than truncating your server configs and
tokens, and there is no moment at which the file is world-readable.

## What is not here

Worth stating, because a guide that only lists what works is a guide that makes
you debug the rest:

- **Servers cannot call back into the agent.** No `sampling`, no
  `roots/list` handling, no `ping` reply. Server-initiated requests are not
  implemented; a server that requires them will not work. `roots` is a client
  capability that is not advertised, so a conforming server will not ask.
- **`notifications/tools/list_changed` does not re-list automatically.** The
  notification is tolerated (it does not desync the stream) but the tool list is
  read at connect time, so a server that changes its tools mid-session needs a
  `Reconnect`.
- **`notifications/message` is tolerated, not surfaced.** A server's log
  notifications are read past so they cannot desync the stream, but they are
  neither displayed nor written to a file. A server that reports its problems only
  through `notifications/message` will look silent; its stderr tail is the better
  diagnostic.
- **No protocol-version negotiation.** LCA sends `2025-11-25` and accepts whatever
  version the server answers with, as long as it answers with one. A server that
  implements only an older revision is not rejected for it — which usually works,
  but is not a parley.
- **The legacy `sse` transport** is refused by name.
- **`!command` values** are refused by design (see above).

## See also

- [`docs/capabilities.md`](capabilities.md) — the capability model an extension
  holds; MCP servers are a built-in backend rather than an extension.
- [`docs/extension-authoring.md`](extension-authoring.md) — the extension path.
- [`docs/flows.md`](flows.md) — installed-extension lifecycle and consent.
- [`docs/threat-model.md`](threat-model.md) — what the permission model stops.
