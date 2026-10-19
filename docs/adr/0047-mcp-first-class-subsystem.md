# ADR-0047: MCP is a first-class internal subsystem

Status: accepted. Supersedes ADR-0045.

Date: 2026-10-19.

## Context

ADR-0045 put the MCP client in `extensions/mcp`, on the reasoning that the
extension boundary was the cheap place to test whether the ABI was expressive
enough (ADR-0007's bet). The bridge shipped through five phases on that shape:
stdio, streamable HTTP, OAuth, resources, management.

By 2026-10 the boundary had stopped describing reality in three ways, each
independently fatal to the fiction:

1. **A hardwired host dependency.** `crates/lca-cli/Cargo.toml` carried
   `mcp = { path = "../../extensions/mcp" }` as an ordinary dependency, and
   `crates/lca-core/Cargo.toml` carried one too. A crate the binary cannot
   build without is not an extension in the sense ADR-0007 meant; it is part of
   the binary wearing a different directory name. (The `lca-core` edge was
   test-only in practice, which is worse: a test-only consumer dressed as a
   runtime edge.)
2. **Host-owned management and prompting.** `crates/lca-cli/src/mcp.rs` owns
   the `mcp.json` files, the trust rules, the grant assembly, and the
   system-prompt section. The host asks the spawn prompt, not the extension.
   ADR-0045 argued the capability model made this fall out "of the shape"; in
   fact the host had to know the shape to ask.
3. **A guest that could not serve its own feature set.** `src/guest.rs` was a
   `tool-catalog` component, and that world exports no `net`, `oauth`, or
   `credentials`. Every phase after stdio was therefore native-only, so
   `mcp.wasm` was published to two release workflows and listed in the catalog
   while being incapable of the features it advertised.

That is the revisit ADR-0045 named: "a phase-2/3 requirement that the four
capabilities cannot serve (streaming transports beyond stdio pipes …)" and "one
such case scopes a new capability". Two fired. The answer turned out not to be a
new capability but a different home.

pi's own layout is the corroboration. `packages/mcp/` declares no dependency on
the agent package at all — its only runtime dependency is `cross-spawn` — and
splits into `protocol/`, `transports/`, `oauth/`, and `auth-provider.ts` beneath
a `client.ts`. The protocol adapter sits below the agent on both sides; LCA's
error was putting it in the extension tree, which is a *sibling* layer, and then
letting the host import it directly.

## Decision

MCP becomes `crates/lca-mcp`, a first-class internal subsystem: a build-time
backend in ADR-0013's taxonomy, symmetric with the ACP adapter (gh #1) pointing
*upward* to other agents while MCP points *downward* to tool servers.

- The wire kit (`crates/lca-wire-mcp`) moves inside it as `src/wire.rs`. It had
  exactly one consumer, and `scripts/docs-consistency.sh`'s one-SSE-parser rule
  is amended to recognize `crates/lca-mcp` alongside `crates/lca-wire-*` —
  MCP's streamable-HTTP envelope is a different protocol from the model
  providers' SSE, so the guard still means "no forked provider engine".
- The WASM guest, its `extension.toml`, and every publish/catalog reference are
  deleted. There is no `mcp.wasm`.
- Dependency edges: `lca-mcp` depends on `lca-protocol`, `lca-ext-abi`,
  `lca-tools`, `lca-permissions`, and `lca-config`, and on **neither `lca-core`
  nor `lca-cli` in any table** — runtime, dev, or build. The direction is
  pinned by `crates/lca-core/tests/architecture.rs`, so re-growing the old edge
  fails a test rather than passing review.
- `lca-core` carries no MCP dependency at run time; its `tests/mcp_bridge.rs`
  keeps a dev-dependency, which is the correct direction (tests may reach down).

### What is preserved, deliberately

The extension-bridge era got four properties right, and they survive the move
unchanged:

- **The star topology** — one long-lived child per configured stdio server,
  newline-delimited JSON-RPC over its pipes, or a single hop per request over
  streamable HTTP. No multiplexing, no shared transport.
- **Per-spawn permission prompts.** Spawning a server asks exactly as a
  model-requested command does; a declined server never starts and the denial
  is recorded. This is now a host call into an internal module rather than a
  capability grant, but the question and the record are identical.
- **The permission layer on every call.** Every invocation runs through the
  turn's `tool_call`/`tool_result` hooks like any other tool, and the host still
  refuses to treat a server's `readOnlyHint` as a bypass.
- **The `!command` refusal**, which keeps shell syntax out of a tool name.

`manifest_grants()` keeps its exact shape and becomes the single declaration of
the subsystem's capability set: with `extension.toml` gone there is one table
rather than two that had to be kept in step.

## Alternatives considered

**Keep the extension shape and add the capabilities the guest needs** (`net`,
`oauth`, `credentials` in the `tool-catalog` world). This was ADR-0045's
implicit plan and it is the wrong trade twice over: it widens the sandbox
surface for every `tool-catalog` guest so that one of them can hold OAuth
tokens, and it still leaves the host owning the files and the prompts. Adding
capabilities to serve a fiction is worse than admitting the crate is internal.

**Port pi's shape wholesale** — host-owned client, `/mcp` modal, its own
enablement model. Rejected for this phase on scope, not on merit: pi is the
long-term reference for *behavior* (later phases C–G), but the consolidation
had to land as a pure move first, with the moved test suite as the bracket.
Absorbing behavior at the same time would make a regression impossible to
attribute.

**Leave it as-is and document the fiction.** The published `mcp.wasm` was
incapable of the feature set it claimed, so "document it" means documenting a
capability the artifact does not have.

## Consequences

The binary's dependency graph is now honest: `lca-core` sits above `lca-mcp`,
and `lca-mcp` cannot see the loop it serves. That is what makes later phases
possible at all — folding host management into `lca-mcp/src/manager.rs`
(phase B) is only coherent if the subsystem does not already depend on the host.

No ABI change: no world, export, manifest key, or `ABI_VERSION` move. The
`tool-catalog` world keeps its meaning and loses a guest.

Removed surface, named: `mcp.wasm` is no longer published (two workflows and
the catalog entry are gone), and the manifest↔grants parity test dies with
`extension.toml`. The concern that test guarded — one capability set with one
source of truth — is preserved by there being exactly one declaration left.

## Revisit conditions

A second internal protocol adapter arriving (another tool-server protocol, or a
second agent adapter) argues for a `crates/lca-adapters` grouping rather than
one crate per protocol. MCP growing a transport the native path cannot express
— a background-reconnect daemon shared across sessions, say — argues for a
long-lived supervisor above this crate, which would be its own record. If a
future phase needs the subsystem to read agent state (a `/mcp` modal that shows
turn context), that is the moment to reconsider whether `lca-mcp` can stay below
`lca-core` or whether the modal belongs in `lca-ui` instead.
