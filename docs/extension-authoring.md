# Extension authoring guide

Version 0.1, 2026-09-20. Targets ABI 0.1.

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
wkg get lca:ext@0.1.0 --format wit --output wit/
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
abi = "0.1"
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

## Other worlds

The `command` world adds a slash command. The spec function returns a name, an argument hint, and a completion mode. The invoke function takes the argument string and returns an effect: insert text into the input, submit a prompt, show a widget, or do nothing.

The `hooks` world observes and intercepts the agent loop, one function per hook point: `pre-turn`, `pre-tool-use`, `post-tool-use`, `post-turn-end`, `attention-required`, and `session-close`. The pre-tool hook is the one with power: it returns allow, deny with a reason, or replace the call. A replaced call passes through the permission layer like any other call and is not fed back through the hooks. A hook that denies a call returns a reason the model sees, which is how a policy extension teaches a model what not to do.

The `ui` world renders. It returns a widget tree for a named region: text spans, images with a media type and bytes, boxes, rows, columns, a spinner, a progress bar, and a key-value list. See the capability catalog for the regions and the rendering model. An extension cannot write terminal escape sequences; text spans carry data only.

The `compaction` world replaces an old range of session records with a summary when the host decides usage has crossed a threshold, or when the user runs a manual compact command. It exports a single `compact` function taking the candidate record range and returning a summary; the host writes the result as a durable record, and every later read of the session uses it without recomputing anything. A mechanical strategy, dropping the oldest turns or keeping only errored tool results, needs no capability beyond what it already reads through `fs` or the session state the host passes in. A strategy that summarizes with a model needs `completion`. An extension implementing `compaction` typically also implements `command`, for the manual trigger. See ADR-0015.

The `context-transform` world reshapes the message list on its way out to the model, on every turn, touching nothing durable. It exports one function taking the resolved messages and returning either a transformed list or a rejection. This is where something like pre-send redaction or skills-handling's implicit instruction injection belongs, not a special power added to hooks. The host chains every enabled transform extension in order; a rejection from any of them ends the turn with that reason surfaced. See ADR-0015 for why this is a separate world from `compaction` rather than the same mechanism doing two jobs.

A transform that modifies content early in the message list, rather than only appending near the end, breaks the provider's prompt cache for the rest of the conversation. The host measures this cross-turn, per ADR-0017, by comparing what goes out against the previous turn's stable content: the first diverging turn is narrowed and recorded as an extension event rather than failing, and a transform whose output is byte-stable across turns settles and stops narrowing. A transform whose output drifts, a timestamp or a counter baked into an old message, keeps the cache broken and costs real money and latency every turn. Write a transform so it only touches the most recent messages unless there is a specific reason to reach further back; `docs/testing-plan.md` has the test scenarios that catch this if it happens anyway.

The `provider` world exports model listing, a completion call that returns a stream resource, and the authentication functions, plus `login`, `logout`, and `usage`, each always exported and each returning a defined "not supported" result when a provider doesn't have one. Promoting these three to world-level exports, rather than leaving them as ad hoc commands each author names differently, is what lets the host offer a generic `/login` picker across every installed provider and a `/usage` that follows whichever one is currently active, alongside the automatically namespaced per-provider form. See ADR-0012. It is the largest surface and the one with the most failure modes. Read the next section before attempting one.

## Writing a provider

A provider extension lists models, streams completions, and handles authentication.

Streaming is a pull resource. The host calls a next function repeatedly and gets one typed event each time. The events are text delta, reasoning delta, tool call start, tool call argument delta, tool call end, usage, error, and `vendor-event`.

Three rules keep a provider correct.

Emit a tool call start before any argument delta for that call. A delta with no open start is discarded and recorded as a protocol error.

Do not accumulate argument fragments. Emit them as they arrive with the call identifier. The host joins and parses them, because the host is the side that knows the tool schema.

Use `vendor-event` for anything the typed cases do not cover. It carries a kind string and a JSON payload. Do not encode extra structure inside a text delta, because the host renders text deltas to the screen.

The completion call carries a cache-boundary hint alongside the message list: a count of leading messages the host considers the stable, cacheable prefix, per ADR-0017. If the vendor has an explicit cache-marking mechanism, place it at that boundary rather than guessing from the message shape. If the vendor has no such mechanism, or relies on automatic prefix caching with nothing to mark, ignore the hint; it is advisory, and a provider that doesn't use it is not doing anything wrong. Report `cache_read` and `cache_write` token counts on the `usage` event whenever the vendor's response includes them; this is what makes cache behavior testable at all, per `docs/testing-plan.md`.

For authentication with an API key, read it from the `credentials` capability, and fall back to an environment variable the user can set. For a subscription login, use the `oauth` capability. The extension builds the authorization URL and the code challenge, calls begin to get a redirect URL, waits for the callback, and exchanges the code over the `net` capability. The extension never binds a port.

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

## Publishing

An extension is an OCI artifact. Any registry that implements the OCI distribution specification works, including a personal namespace on a public container registry.

```
wkg oci push ghcr.io/yourname/word-count:0.1.0 word_count.wasm
wkg oci push ghcr.io/yourname/word-count:abi-0.1 word_count.wasm
```

Push two tags. The version tag is immutable and identifies this exact release. The ABI line tag moves and is what the update resolver reads. A user's agent resolves the moving tag to a digest at update time and loads by digest afterward, so the moving tag never decides what runs on an already-installed machine.

Users install with the reference.

```
lca ext install ghcr.io/yourname/word-count:abi-0.1
```

An OCI registry is not the only option. Any HTTPS host works: zip the component and `extension.toml` together with nothing else added, publish two URLs the same way, one for the fixed version and one that always points at the current release, and a user installs with `lca ext install https://yourhost.example/word-count-abi-0.1.zip`. The update mechanics are identical either way; see ADR-0010.

## Versioning

The extension version is yours. The ABI version is not.

The `abi` field names the ABI line the component targets. The host loads the current ABI minor version and the one before it, so a new ABI minor version gives one cycle to rebuild and publish.

When the ABI moves, rebuild against the new WIT package, bump the extension version, and push both tags. Users run `lca ext update`. If a user's host stops supporting an old ABI before a rebuild lands, their agent disables the extension and tells them a rebuild is needed. It does not stop working.

Read the ABI versioning policy for what counts as a breaking change.

## Other languages

The ABI is WIT, so any language with Component Model tooling can implement it. The WIT package and the manifest are identical across languages. What changes is the bindings generator and the build command.

The practical constraint in 0.1 is that Rust has the most complete tooling. Go, C, and Python all have generators at varying maturity. An author using another language should expect to hit rough edges in the toolchain rather than in the ABI.

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
