# Extension authoring guide

Version 0.1, 2026-09-20. Targets the current ABI line (0.x development window per ADR-0028; examples below show the line current at writing).

This guide builds an extension from nothing to a published artifact. It uses Rust for the examples because the tooling is furthest along there. The ABI is language-neutral, and the section on other languages covers what changes.

## What an extension is

An extension is a WebAssembly component plus a manifest. The component implements one or more WIT worlds. The manifest says which worlds it implements and which capabilities it needs.

The host loads the component, checks the manifest against what the user approved, builds an import table from the approved capabilities, and instantiates. Anything the manifest did not declare is not in the import table, so the extension cannot call it.

An extension is not a script and not a dynamic library. It cannot see the agent's memory, cannot call agent functions that are not in its import table, and cannot reach the operating system except through host imports.

## Before starting

Install a Rust toolchain, add the component target, and install the component tooling.

```
rustup target add wasm32-wasip2
cargo install cargo-component
cargo install wkg
```

`wkg` fetches WIT packages from a registry and publishes artifacts. `cargo-component` builds a Rust crate into a component.

Get the ABI package. It holds the WIT definitions for every world.

```
wkg get lca:ext@0.6.0 --format wit --output wit/
```

## A first extension: a tool

The smallest useful extension implements the `tool` world. It adds one callable tool to the agent.

Create the crate.

```
cargo component new --lib word-count
cd word-count
```

Point the manifest at the world. In `Cargo.toml`:

```toml
[package.metadata.component]
package = "example:word-count"

[package.metadata.component.target]
path = "wit"
world = "tool"
```

Implement the two exports. The schema function describes the tool to the model. The execute function does the work.

```rust
use crate::bindings::exports::lca::ext::tool::{Guest, ToolCall, ToolResult, ToolSchema};

struct Component;

impl Guest for Component {
    fn schema() -> ToolSchema {
        ToolSchema {
            name: "word_count".to_string(),
            description: "Count words in a file in the workspace.".to_string(),
            parameters: r#"{
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Path relative to the workspace root." }
                },
                "required": ["path"]
            }"#
            .to_string(),
        }
    }

    fn execute(call: ToolCall) -> ToolResult {
        let args: serde_json::Value = match serde_json::from_str(&call.arguments) {
            Ok(v) => v,
            Err(e) => return ToolResult::error(format!("bad arguments: {e}")),
        };

        let path = match args["path"].as_str() {
            Some(p) => p,
            None => return ToolResult::error("path is required".to_string()),
        };

        match crate::bindings::lca::host::fs::read("workspace", path) {
            Ok(bytes) => {
                let text = String::from_utf8_lossy(&bytes);
                ToolResult::text(format!("{} words", text.split_whitespace().count()))
            }
            Err(e) => ToolResult::error(format!("cannot read {path}: {e}")),
        }
    }
}

bindings::export!(Component with_types_in bindings);
```

Write the manifest as `extension.toml` next to `Cargo.toml`.

```toml
name = "word-count"
version = "0.1.0"
abi = "0.6"
worlds = ["tool"]
description = "Counts words in a workspace file."
license = "MIT"

[capabilities.fs]
workspace = "read"
```

Build it.

```
cargo component build --release --target wasm32-wasip2
```

Install it from the local path and try it.

```
lca ext install ./target/wasm32-wasip2/release/word_count.wasm --manifest ./extension.toml
```

The agent shows the capability prompt before it writes anything. It says the extension wants to read files in the project. Approve, start a session, and ask the model to count the words in a file.

## The schema and the model

The `parameters` field is a JSON Schema and the model reads it. A vague description produces wrong calls. Two rules matter more than the rest.

Describe when to use the tool and when not to. Models pick tools by description, not by name. A description that says what the tool does and nothing about when to reach for it gets called at the wrong times.

Constrain the parameters. An enum with three values produces better calls than a string with three values named in prose.

The host validates arguments against this schema before it calls `execute`. An argument that fails validation never reaches the extension.

## Tool suites, exposure, and namespaces

One component may register a whole suite: declare the `tool-catalog`
world and return every spec from `get-tools`, each dispatched by name
through `run-tool`. A guest declaring both worlds serves everything
through the catalog; a guest exporting only `get-schema`/`run` keeps
loading exactly as before, wrapped as one `direct` tool with no
namespace.

`exposure` controls how the model reaches a tool. Only `direct`
tools are declared to the model. `model-only` tools never are and
never run nested either: host-side tools the model neither sees nor
calls. `codemode` tools are callable whenever registered and listed
for codemode-style callers. `deferred` tools are found through
discovery instead: the model calls the built-in `tool_search` with a
keyword, the hits activate, and the next request declares the newly
active `direct` ones. `hidden` tools are registered but unreachable:
not declared, not callable, not searchable. Re-register `hidden` to
withdraw a tool. An unknown exposure refuses the whole registration,
naming the value.

`namespace` (`name`, `description`, `instructions`) groups related
tools the way MCP servers do: discovery lists a namespace under one
heading with its description, and `instructions` holds the longer
guidance discovery surfaces but declarations never carry.
`annotations` (the four MCP hints) travel on the spec for the model;
the host's permission layer never decides on them — a hint is not a
bypass.

The active set is dynamic: `setActiveTools`-style replacement takes
only registered names (unknown names are ignored and reported), and
the host records the change in the transcript before the next model
request. Orchestrator tools call other tools through the `tools`
import (`execute-tool`, `list-tools`), declared via
`[capabilities.tools]` with a reason. The host assigns `<parent
id>/<n>` call ids, so events carry the linkage without the guest
threading it; the parent's result keeps a bounded record of the
nested calls (the first entries win). Nested calls run the same
validation, hooks, and permission checks as model-issued calls, at
most eight levels deep, and never reject: unknown tools, blocks, and
failures all arrive as error results.

## Bridging an MCP server

An external MCP server joins as a `tool-catalog` extension
(ADR-0045, `extensions/mcp`): spawn it through `process`, speak
newline-delimited JSON-RPC over its pipes (`initialize`, the
`notifications/initialized` handshake, `tools/list`, `tools/call`),
and serve each tool under pi's `mcp__<server>__<tool>` name with
the server's annotations carried across. One long-lived child per
server; killing the bridge kills the tree. The spawn passes the
same permission prompt as a model-requested command, so a declined
server never starts and the denial is recorded - per-call arguments
flow to an already-approved server, and every call runs through the
turn's hooks like any other extension tool. The sandboxed twin reads
its server list from the `state` key `mcp-servers`.

A remote server rides streamable HTTP over `net`: JSON or SSE
envelopes, the `Mcp-Session-Id` round-trip, pi's transient retry on
the idempotent reads (calls never retry), and the configured
per-request timeout bounding the whole exchange. OAuth is the
provider pattern: dynamic registration (or a configured client),
the PKCE loopback flow through the `oauth` capability, tokens in
the `credentials` namespace keyed by name and URL, proactive
refresh near expiry, one reactive refresh on 401, purge on
`invalid_grant`, and step-up scopes remembered into the next
sign-in. A static `Authorization` header disables OAuth, exactly
like pi.

### Configuration

`mcp.json` lives beside the config: user-level next to the data
dir, project-level at `<cwd>/.lca/mcp.json` (read only when the
project is trusted, the config file's own rule). A project entry
with a transport adds or replaces the server; one without only
overrides `enabled`, `exposure`, `toolExposure`, and `description`
of a user-level server. Invalid entries are reported at startup
and skipped, never blocking the rest.

```json
{
  "mcpServers": {
    "echo": {
      "command": "python3",
      "args": ["echo-server.py"],
      "exposure": "direct",
      "description": "Echo."
    },
    "docs": {
      "url": "https://example.com/mcp",
      "headers": { "Authorization": "Bearer ${DOCS_TOKEN}" },
      "timeout": 30
    }
  }
}
```

`${NAME}` expands from the process environment in commands, args,
headers, and secrets. `!command` values are refused (executing
config content is a trust hole), `env` waits for the 0.7
process-env seam (entries carrying it skip loudly), the legacy SSE
transport is rejected, and `cwd` names an `fs` scope (default
`workspace`).

### Managing servers

`/mcp` prints every server with state, tools, and source; its
verbs act: `reconnect`, `enable`/`disable`, `exposure` (set or
cycle direct → codemode → deferred → hidden), `login` (background
OAuth; the browser opens where one exists), `logout`. Enable and
exposure edits persist to the defining file - a user-level server
edited under a trusted project writes a knob-only project
override, and later edits stay there. `/reload` re-reads the files.
Shell management (`lca mcp ...`) waits for the CLI-delegation
mechanism (#171, 0.7); until then the files stay hand-editable.

### Exposure and resources

The server default is `codemode`; `toolExposure` overrides per
tool (exact names win, then `*` patterns). Deferred tools surface
through `tool_search` and declare on activation; hidden tools never
register. Servers offering resources add `list_mcp_resources`,
`list_mcp_resource_templates`, and `read_mcp_resource` at the
widest such server's exposure. Text reads as text, images ride the
image lane, other bytes stage to a file whose path the model
receives. Reachable non-direct servers list under `## MCP servers`
in the system prompt with one line on how their tools are reached.

The sandboxed twin serves server tools over stdio; remote servers
and the resource trio stay native until server URLs have a managed
path to the guest (a `net` import on the `tool-catalog` world is a
WIT change, deferred under the freeze).

## Other worlds

The `command` world adds a slash command. The spec function returns a name, an argument hint, and a completion mode. The invoke function takes the argument string and returns an effect: insert text into the input, submit a prompt, show a widget, or do nothing.

The `hooks` world observes and intercepts the agent loop, one function per hook point: `pre-turn`, `pre-tool-use`, `post-tool-use`, `post-turn-end`, `attention-required`, and `session-close`. The pre-tool hook is the one with power: it returns allow, deny with a reason, or replace the call. A replaced call passes through the permission layer like any other call and is not fed back through the hooks. A hook that denies a call returns a reason the model sees, which is how a policy extension teaches a model what not to do. These six points are frozen: they keep their shapes.

Eight more worlds opt in per point, each declared in the manifest:
`hooks-message` observes finalized assistant and tool-result messages
and may replace their text (the host writes an append-only edit, so
a redaction hook's whole job is returning the rewritten text);
`hooks-tool-call` mutates calls compositionally in registration order
(each handler sees the previous arguments; a block vetoes with its
reason) before the `pre-tool-use` verdict runs; `hooks-tool-result`
composes results the same way before the log keeps them;
`hooks-stream` observes normalized provider events in order after the
stream closes (observation never steers a live stream);
`hooks-settle` (`turn_end`, then `agent_before_settle`) may append
context entries and continue exactly one more provider request per
turn; `hooks-compaction` vetoes a compaction with its reason and
observes failures; `hooks-cache` votes on prompt-cache warming
(any decline skips it); `hooks-trust` votes yes/no/undecided on
project trust before the operator is asked. A broken hook is skipped
with a warning, never a silent veto — except `pre-tool-use`, whose
deny is the verdict. The `context` and `context_with_system` points
are the `context-transform` world, which already sees the full
transcript including the system prompt: no second surface.

The `ui` world renders. It returns a widget tree for a named region: text spans, styled text (independent foreground/background as a role or `#RRGGBB`, plus bold/dim/italic/underline), markdown (the host's own engine, highlighted fences included), buttons (an id plus a label), tables (headers plus rows, auto-aligned), scroll containers (a viewport height over child nodes, with a scrollbar thumb), images with a media type and bytes, boxes (an optional title plus a border role and background tint), rows, columns, a spinner, a progress bar, and a key-value list. See the capability catalog for the regions and the rendering model. An extension cannot write terminal escape sequences; text spans carry data only.

Interaction arrives through the `interaction` world: keys, submissions, and dismissals as before, plus mouse - `click-widget(id)` for a declared button, `click(col, row)` relative to the region's content origin, and `scroll(+1/-1)` for the wheel. Clicks map on the modal and panel regions; the footer and status-line regions stay display-only (their rows pack native and extension statuses together, so a cell cannot name one owner). Input of every kind goes to the first extension registered for the region.

For questions, do not build a modal: call `lca:host/ui-dialogs` (`confirm`, `select`, `input`, `notify`) from a tool or a hook and the host asks on its native chrome. No capability grant is needed - each question is its own consent, answered on host chrome the extension cannot forge. `render`, `interaction`, and command `invoke` run on the interface thread, so a dialog there is refused with a message instead of deadlocking; ask from tools and hooks. Headless answers the denied values (`false`/`None`, notify dropped) without asking.

The `compaction` world replaces an old range of session records with a summary when the host decides usage has crossed a threshold, or when the user runs a manual compact command. It exports a single `compact` function taking the candidate record range and returning a summary; the host writes the result as a durable record, and every later read of the session uses it without recomputing anything. A mechanical strategy, dropping the oldest turns or keeping only errored tool results, needs no capability beyond what it already reads through `fs` or the session state the host passes in. A strategy that summarizes with a model needs `completion`. An extension implementing `compaction` typically also implements `command`, for the manual trigger. See ADR-0015.

The `context-transform` world reshapes the message list on its way out to the model, on every turn, touching nothing durable. It exports one function taking the resolved messages and returning either a transformed list or a rejection. This is where something like pre-send redaction or skills-handling's implicit instruction injection belongs, not a special power added to hooks. The host chains every enabled transform extension in order; a rejection from any of them ends the turn with that reason surfaced. See ADR-0015 for why this is a separate world from `compaction` rather than the same mechanism doing two jobs.

A transform that modifies content early in the message list, rather than only appending near the end, breaks the provider's prompt cache for the rest of the conversation. The host measures this cross-turn, per ADR-0017, by comparing what goes out against the previous turn's stable content: the first diverging turn is narrowed and recorded as an extension event rather than failing, and a transform whose output is byte-stable across turns settles and stops narrowing. A transform whose output drifts, a timestamp or a counter baked into an old message, keeps the cache broken and costs real money and latency every turn. Write a transform so it only touches the most recent messages unless there is a specific reason to reach further back; `docs/testing-plan.md` has the test scenarios that catch this if it happens anyway.

The `provider` world exports model listing, a completion call that returns a stream resource, and the authentication functions, plus `login`, `logout`, and `usage`, each always exported and each returning a defined "not supported" result when a provider doesn't have one. Promoting these three to world-level exports, rather than leaving them as ad hoc commands each author names differently, is what lets the host offer a generic `/login` picker across every installed provider and a `/usage` that follows whichever one is currently active, alongside the automatically namespaced per-provider form. See ADR-0012. It is the largest surface and the one with the most failure modes. Read the next section before attempting one.

## Resources and state

An extension package may carry a `resources/` folder: its own read-only data (presets, `SKILL.md` docs, templates, any bytes). The extension reads it through `lca:host/resources` - `list-resources(prefix)` and `read(path)` - and sees only its own tree, never the filesystem. Declare the kinds in the manifest:

```toml
resources = ["provider-presets", "skills"]
```

The installer refuses a kind the manifest does not declare and shows the counts at consent. A kind is the first segment of the resource's path inside the bag: a folder's name (`skills` for `resources/skills/<name>/SKILL.md`), and for a file sitting directly in `resources/` its whole file name — `resources/provider-presets.toml` declares `"provider-presets.toml"`. The host reads `resources/skills/<name>/SKILL.md` (standard Claude format) into the prompt with attribution; other kinds are the extension's own business.

For mutable, non-secret data, use `lca:host/state` - `read`/`write`/`delete`/`list-keys`, keyed by your own identity, size-capped, wiped on uninstall. Secrets go in `credentials`, never `state`.

A state key is 1-200 characters of ASCII letters, digits, `.`, `_`, or `-`, because the host stores one file per key under your own namespace and a path separator would be a path surface. A key outside that set is refused with an `invalid` error naming the rule, not silently sanitized. The value is opaque bytes: up to 4 MB per key, 16 MB per namespace.

```rust
let bytes = lca::host::resources::read("provider-presets.toml")?;
let preset_text = String::from_utf8_lossy(&bytes);
lca::host::state::write("last-model", b"gpt-4o")?;
```

A data-only extension declares `worlds = []` and ships only a manifest and a `resources/` bag; it installs through the same pipeline (a skill pack needs no component).

## Markdown transforms

The markdown pipeline takes an ordered list of pre-parse transforms (gh #12): pi's `registerMarkdownTransformer`. Each transform sees the raw source and a small context (whose markdown it is, whether the message is streaming, the available width) and returns the source the parser sees next. Transforms run in registration order; a transform that panics behaves as identity, so a hostile transform cannot break the render.

What it is not: this is not a WIT world and there is no WASM-facing export yet. A native (Rust) extension provides one by implementing the optional `ExtensionDispatch::markdown_transformer` method (the default is absent); the host collects native transforms into the transcript's pipeline in registration order. The WASM export ships with the first third-party-shaped consumer. Reasoning runs render as plain wrapped lines rather than parsed markdown, so transforms see user and assistant sources only.

## Writing a provider

A provider extension lists models, streams completions, and handles authentication.

Streaming is a pull resource. The host calls a next function repeatedly and gets one typed event each time. The events are text delta, reasoning delta, tool call start, tool call argument delta, tool call end, usage, error, and `vendor-event`.

Three rules keep a provider correct.

Emit a tool call start before any argument delta for that call. A delta with no open start is discarded and recorded as a protocol error.

Do not accumulate argument fragments. Emit them as they arrive with the call identifier. The host joins and parses them, because the host is the side that knows the tool schema.

Use `vendor-event` for anything the typed cases do not cover. It carries a kind string and a JSON payload. Do not encode extra structure inside a text delta, because the host renders text deltas to the screen.

The completion call carries a cache-boundary hint alongside the message list: a count of leading messages the host considers the stable, cacheable prefix, per ADR-0017. If the vendor has an explicit cache-marking mechanism, place it at that boundary rather than guessing from the message shape. If the vendor has no such mechanism, or relies on automatic prefix caching with nothing to mark, ignore the hint; it is advisory, and a provider that doesn't use it is not doing anything wrong. Report `cache_read` and `cache_write` token counts on the `usage` event whenever the vendor's response includes them; this is what makes cache behavior testable at all, per `docs/testing-plan.md`.

For authentication with an API key, read it from the `credentials` capability, and fall back to an environment variable the user can set. For a subscription login, use the `oauth` capability. The extension builds the authorization URL and the code challenge, calls begin to get a redirect URL, waits for the callback, and exchanges the code over the `net` capability. The extension never binds a port.

Declare the environment override your endpoint reads (gh #157) so the host's ad hoc `net` grant path consults it without provider-specific code:

```toml
[login]
env_base_url = "OPENAI_BASE_URL"
```

With no `[login]` table the host skips the environment and reads stored credentials only. The key is optional and additive: old manifests parse on new hosts, and new manifests parse on old hosts, which ignore the table.

A provider may also export `provider-login` (ADR-0033): `login-options` returns the picker choices the host renders (load them from your own `resources/provider-presets.toml` through `lca:host/resources`), and `login-submit` consumes the chosen id and the field values, stores the secret in your `credentials` namespace, and returns opaque `setting: value` pairs for the host to persist. The host never parses provider-shaped data; it renders the picker, masks the secret, persists the settings, and runs the ad hoc `net` grant when the chosen host is outside the manifest's vocabulary. A provider whose login is self-contained (an OAuth flow) exports `provider-login` returning no options and keeps its flow in `login`.

To offer a custom endpoint of your own (gh #188), declare it as a preset with no `base_url` and explicit `fields`, `kind`, and `default_profile`:

```toml
[[preset]]
id = "custom"
name = "Custom endpoint…"
fields = ["base-url", "api-key", "model"]
kind = "custom"
default_profile = true
```

The host prompts for the declared fields in order and submits the answers to your `login-submit` like any preset. `kind = "custom"` keeps the host's preset-less treatment (no `preset` pair is stored; the extension name stands in), and `default_profile = true` stores bare default-profile keys, like a direct setup. Any field ids are allowed - the host prompts generically and masks `api-key`-shaped ids - and the option rides the same consent and discovery path as every preset.

Build on the shared wire kits (gh #189) instead of writing your own SSE parser. An OpenAI-family provider (`/v1/chat/completions` and/or `/v1/responses`) depends on `lca-wire-openai`: `chat_completions::{SseDecoder, parse_sse, to_wire, tools_wire, map_usage}` for the Completions shape, `responses::{build_responses_body, ResponsesStream, responses_usage, ResponseStreamDriver}` for the Responses shape, and the shared `StreamFailure` vocabulary for every failure. The Responses driver takes an `on_unauthorized` callback for the 401 purge - credential lifecycle stays the caller's (`lca_subscription::purge_tokens`). An Anthropic-family provider (`/v1/messages`) depends on `lca-wire-anthropic`: `build_messages_body` (with `cache_breakpoints` for pi's ephemeral markers) and `AnthropicStream` (thinking signatures emit as `thinking-signature` vendor events until #41). Both kits build for native targets and `wasm32-wasip2`; a second copy of any of these engines outside the kits fails `scripts/docs-consistency.sh`.

Refresh expired tokens before the next call, not on failure. Waiting for a 401 costs a round trip and produces a confusing error if the refresh also fails.

## Capabilities in practice

Ask for the smallest set that works. A user comparing two extensions that do the same thing picks the one asking for less, and a reviewer will say so publicly if the set looks wide.

Declare `net` with exact hostnames where possible. A wildcard is sometimes necessary for a content delivery network and it always reads as broader. A pattern without a port grants 443 only; pin one, `host:8443`, when the service really is on a non-standard port, and the consent text will show it. `net` never reaches a loopback or private-network address, even if a hostname happens to resolve there; if the extension genuinely needs a local address, such as a provider talking to a model server on the user's own machine or network, declare `net-local` instead, which has its own, honest consent text for that broader-than-this-machine reach. See the capability catalog and ADR-0011.

Do not ask for `home-config` unless reading an existing tool's login is the actual goal. It is the scope most likely to make someone stop and think.

The `process` and `pty` capabilities both need a reason string that is shown verbatim at install time. Write it for a person who does not know what the extension does. Reach for `pty` specifically when the spawned program checks whether it has a real terminal and behaves differently without one; `process`'s plain pipe-backed streams are enough for anything that doesn't.

If an extension needs a response from the current model as an input to its own logic, rather than only producing output the model reads, that is `completion`, not a call to another extension. There is no extension-to-extension calling in this design; see ADR-0008.

Test the denied path. A user can revoke a grant, and an extension that panics when a capability is missing fails badly at the worst moment. Every capability call returns a result. Handle it.

## Testing

`lca-testkit` provides a fake host. It instantiates a component with a configured capability set and asserts on the calls it makes. For a provider extension specifically, script realistic usage numbers, including cache read and write counts, on every scripted turn; `docs/testing-plan.md` has the full cache-behavior test scenarios this makes possible, and they apply to a third-party provider the same way they apply to the first-party ones.

```rust
#[test]
fn counts_words_in_a_workspace_file() {
    let host = FakeHost::builder()
        .grant_fs("workspace", Mode::Read)
        .file("notes.md", "one two three")
        .build();

    let result = host.call_tool("word_count", r#"{"path":"notes.md"}"#);
    assert_eq!(result.text(), "3 words");
}

#[test]
fn reports_an_error_when_the_grant_is_missing() {
    let host = FakeHost::builder().build();
    let result = host.call_tool("word_count", r#"{"path":"notes.md"}"#);
    assert!(result.is_error());
}
```

The second test is the one authors skip and should not. It covers the revoked-grant path.

Tests run offline. The fake host refuses network calls unless a test opts in with a recorded response.

## Building both ways

The same source compiles two ways. The component build is what third parties publish. The native build is for extensions that ship inside the agent binary, where there is no marshaling and no sandbox.

The trait that `wit-bindgen` generates is an ordinary Rust trait. An implementation of it compiles for a native target as easily as for `wasm32-wasip2`. To make an extension work both ways, keep the implementation free of anything specific to the component build, and put host access behind the generated import functions rather than calling the operating system directly.

Native mode is for first-party code only. A third-party extension compiled into the binary has no capability boundary at all, and the agent labels it unsandboxed in the extension list.

Native code that can be cancelled must implement two dispatch methods as a pair. `interrupt` flags the capability engine (`cap.cancel()`) so a blocked host wait returns promptly, because no epoch bump can reach code sharing the caller's thread. `turn_started` clears that flag again (`cap.reset_cancellation()`); the host calls it once at every turn boundary. Implementing only `interrupt` latches the flag: after one cancelled turn, every later call in the session fails with "request cancelled by the user". The WASM host does both sides itself; a native implementation must do its own half. The conformance extension carries the pair, and `openai-compatible`'s `turn_started_clears_the_interrupt_left_behind` is the regression row.

## Publishing

An extension is an OCI artifact. Any registry that implements the OCI distribution specification works, including a personal namespace on a public container registry.

```
wkg oci push ghcr.io/yourname/word-count:0.1.0 word_count.wasm
wkg oci push ghcr.io/yourname/word-count:abi-0.2 word_count.wasm
```

Push two tags. The version tag is immutable and identifies this exact release. The ABI line tag moves and is what the update resolver reads.

`lca ext install` reads `extension.toml` from the artifact's OCI config blob, with the second layer as a fallback, and the component from layer0 (`application/wasm`). `scripts/publish-oci.sh` publishes exactly that layout, and the repository's release workflow runs it for the first-party extensions - a bare `wkg oci push` of the component alone produces an artifact with no manifest in it, so pack the manifest into the config blob the way the script does. A user's agent resolves the moving tag to a digest at update time and loads by digest afterward, so the moving tag never decides what runs on an already-installed machine.

Users install with the reference.

```
lca ext install ghcr.io/yourname/word-count:abi-0.2
```

An OCI registry is not the only option. Any HTTPS host works: zip the component and `extension.toml` together with nothing else added, publish two URLs the same way, one for the fixed version and one that always points at the current release, and a user installs with `lca ext install https://yourhost.example/word-count-abi-0.2.zip`. The update mechanics are identical either way; see ADR-0010.

## Project-local extensions

A repo can ship its own tools without publishing (gh #138): drop one directory per extension into `.lca/extensions/`, each holding an `extension.toml` and a `component.wasm` — the same two files an install lays down, minus the digest (the repo is the record). They load when the project is trusted (persistent or session trust; an untrusted project ignores the directory, and its presence triggers the trust prompt like a `.lca/config.toml` does). Grants still prompt exactly as installed extensions do, and `lca ext disable <name>` applies. Most-specific scope wins: a project-local extension shadows a same-named installed one. `--no-extensions` skips them with everything else.

## Versioning

The extension version is yours. The ABI version is not.

The `abi` field names the ABI line the component targets. The host loads the current ABI minor version and the one before it, so a new ABI minor version gives one cycle to rebuild and publish.

When the ABI moves, rebuild against the new WIT package, bump the extension version, and push both tags. Users run `lca ext update`. If a user's host stops supporting an old ABI before a rebuild lands, their agent disables the extension and tells them a rebuild is needed. It does not stop working.

Read the ABI versioning policy for what counts as a breaking change.

## Other languages

The ABI is WIT, so any language with Component Model tooling can implement it. The WIT package and the manifest are identical across languages. What changes is the bindings generator and the build command.

The practical constraint in 0.1 is that Rust has the most complete tooling. Go, C, and Python all have generators at varying maturity. An author using another language should expect to hit rough edges in the toolchain rather than in the ABI.

## Per-extension knobs are config keys, not CLI flags

An extension cannot add a `--flag` or a keybinding (ADR-0044, gh #79): the CLI surface and the keymap are static, so `--help`, completions, and startup order never depend on what is installed. A knob the extension needs is a typed config key through the normal settings process — documented in `docs/configuration.md`, visible in `/settings`, file over env over flag. An action it needs is a slash command (the `command` world), and later an `lca <ext>` subcommand (gh #171). Ask for the key, not the flag.

## Common mistakes

Asking for `read-write` on the workspace when the extension only reads. Reviewers notice.

Assuming a capability is present. Every host call returns a result, and a user can revoke a grant between sessions.

Accumulating tool call arguments inside a provider. The host does this, and doing it twice produces duplicated JSON.

Putting structure inside text deltas. Text deltas go to the screen.

Declaring `oauth` without `net`. The manifest schema rejects it, because a code exchange needs an outbound request.

Writing a `process` reason string like "runs commands". It is shown to a user who has no other information.

Forgetting the ABI line tag when publishing. Users can install the version tag, and the update path will not find anything.

Declaring `net` for an address that turns out to be local. It will be refused at connection time regardless of what pattern matched, since the host checks the resolved address, not just the pattern. Use `net-local`.

Treating `completion` as a way to call another extension. It asks the host for a response from the active provider; there is no path from one extension to another's instance.
