# ADR-0045: MCP arrives as an extension bridge, not a host service

Status: accepted.

Date: 2026-10-09.

## Context

Pi ships MCP as a built-in client: stdio and streamable-HTTP servers
from `mcp.json`, tools as `mcp__<server>__<tool>`, exposure levels,
`/mcp` management, OAuth sign-in. LCA deliberately excludes MCP from
`lca-core` (gh #53's reframe) to keep the binary minimal, and
ADR-0007 names the replacement outright: "An MCP bridge is itself an
extension implementing the `tool` world, which is a good test of
whether the ABI is expressive enough."

The open question was where the client lives: a host service in the
binary, or an extension behind capabilities. A host service would need
process management, JSON-RPC framing, OAuth, and config ownership in
the binary - the bloat the reframe refused. An extension needs three
things, and all three already exist: a way to run a long-lived child
(`process`: spawn plus stdin/stdout/stderr streams), a world to serve
tools through (`tool-catalog`: suites with pi's exposure vocabulary),
and a permission path (every spawn passes the shared grant-store
prompt).

## Decision

MCP is `extensions/mcp`, a `tool-catalog` extension. One long-lived
child per configured stdio server, newline-delimited JSON-RPC over
its pipes, tools served `direct` under pi's `mcp__<server>__<tool>`
names with the server's annotations carried across.

Permission falls out of the shape, not out of new machinery:

- Spawning a server asks the same prompt as a model-requested
  command. The capability, not the bridge, owns the question: holding
  `process` means the extension may ask, never that commands run
  unapproved. A pre-approved pattern satisfies the prompt silently;
  a refusal returns a permission error and is recorded (engine
  denials plus the journal), and the server never starts.
- Per-call arguments flow to an already-approved server. Prompting
  every call would make stdio unusable, and the server's identity -
  its command line - is what the user approved. Every call still
  runs through the turn's `tool_call`/`tool_result` hooks, so
  permission extensions see MCP calls exactly like built-in ones.
- The host's permission layer never decides on the MCP annotations.
  A `readOnlyHint` is model-visible metadata (the catalog record
  says so); treating it as a bypass would let a server mark itself
  safe.

Capability needs, named: stdio is `process` as documented (spawn,
streams, kill the tree on drop) - no new capability for phase 1.
Remote servers reuse `oauth` + `net` + `credentials` exactly as the
provider extensions do; that is a later phase, not a new shape.
Resources, `/mcp` management, project overrides, and the
system-prompt section are later phases. The sandboxed guest reads
its server list from the `state` key `mcp-servers`; seeding that
from the manifest belongs to the management phase, which also owns
whether `mcp.json` lives beside the config or inside it.

## Alternatives considered

A host service in the binary (pi's shape, ported). It re-centralizes
what the capability model decentralizes: the binary would own child
lifecycles, protocol framing, and credential flows for a protocol
whose whole point is that the user already installed and trusts the
server. It also splits the tool pipeline - host-owned MCP tools
would need their own permission story instead of inheriting the
extension one. Rejected: it contradicts the reframe and ADR-0007 in
the same motion.

Per-call approval for every MCP invocation. The spawn prompt already
asks about the server's identity; asking again per call repeats a
question the user answered, and prompt fatigue is how dangerous
approvals get waved through. The hooks still see every call, so a
permission extension that wants per-call policy can write it.
Rejected for the default; the seam stays open for policy authors.

## Consequences

Phase 1 is additive under the 0.6 freeze: a new extension crate, no
WIT or schema change, old guests valid. The bridge is the first
consumer of `process` for a long-lived child (previous uses were
short probes), which exercises the stream reads past one-shot calls.

A server the user declines is invisible: no tools, no retries, one
recorded denial. Operators debugging "my MCP tools are gone" read
the denial journal first.

The guest twin proves the shape is sandbox-expressible - the
capability set needs no hole for this. If a later MCP need cannot be
built behind `process`/`net`/`oauth`/`credentials`, that need is
evidence for ADR-0007's revisit conditions, written down as such.

## Revisit conditions

A phase-2/3 requirement that the four capabilities cannot serve
(streaming transports beyond stdio pipes, server-sent notifications
the pipe cannot carry, background reconnect the guest cannot own).
One such case scopes a new capability; three structural ones reopen
ADR-0007.
