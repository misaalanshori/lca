//! Host-side bindings for every world in `wit/` (gh #551: split out of `lib.rs`).
//!
//! All seventeen are generated here (sixteen `world-*.wit` files, with
//! `world-tool.wit` carrying both `tool` and `tool-catalog`). A world is added to
//! this module in the same change that adds its WIT file, so the list has no
//! "not yet" state to describe. `#[allow(missing_docs)]` is deliberate: the
//! `bindgen!` expansions' documentation lives in the `wit/` files.

#![allow(missing_docs)] // generated bindings: the WIT files carry the docs

//! Host-side bindings for every world in `wit/`: all seventeen are generated here
//! (sixteen `world-*.wit` files, with `world-tool.wit` carrying both `tool` and
//! `tool-catalog`). A world is added to this module in the same change that adds
//! its WIT file, so the list has no "not yet" state to describe.

/// The `tool` world.
pub mod tool {
    wasmtime::component::bindgen!({ path: "../../wit", world: "tool" });
}

/// The `command` world.
pub mod command {
    wasmtime::component::bindgen!({ path: "../../wit", world: "command" });
}

/// The `hooks` world.
pub mod hooks {
    wasmtime::component::bindgen!({ path: "../../wit", world: "hooks" });
}

/// The `provider` world (streaming, identity, model listing).
pub mod provider {
    wasmtime::component::bindgen!({ path: "../../wit", world: "provider" });
}

/// The `compaction` world.
pub mod compaction {
    wasmtime::component::bindgen!({ path: "../../wit", world: "compaction" });
}

/// The `context-transform` world.
pub mod context_transform {
    wasmtime::component::bindgen!({ path: "../../wit", world: "context-transform" });
}

/// The `ui` world.
pub mod ui {
    wasmtime::component::bindgen!({ path: "../../wit", world: "ui" });
}

/// The `tool-catalog` world (gh #77): multi-tool registration.
/// The `tool` module above stays the single-tool world.
pub mod tool_catalog {
    wasmtime::component::bindgen!({ path: "../../wit", world: "tool-catalog" });
}

/// The `hooks-message` world (gh #45): `message_end` replace.
pub mod hooks_message {
    wasmtime::component::bindgen!({ path: "../../wit", world: "hooks-message" });
}

/// The `hooks-tool-call` world (gh #45): composable mutation.
pub mod hooks_tool_call {
    wasmtime::component::bindgen!({ path: "../../wit", world: "hooks-tool-call" });
}

/// The `hooks-tool-result` world (gh #45): composable results.
pub mod hooks_tool_result {
    wasmtime::component::bindgen!({ path: "../../wit", world: "hooks-tool-result" });
}

/// The `hooks-stream` world (gh #45): stream observation.
pub mod hooks_stream {
    wasmtime::component::bindgen!({ path: "../../wit", world: "hooks-stream" });
}

/// The `hooks-settle` world (gh #45): actionable settle.
pub mod hooks_settle {
    wasmtime::component::bindgen!({ path: "../../wit", world: "hooks-settle" });
}

/// The `hooks-compaction` world (gh #45): compact veto.
pub mod hooks_compaction {
    wasmtime::component::bindgen!({ path: "../../wit", world: "hooks-compaction" });
}

/// The `hooks-cache` world (gh #45): cache-warming votes.
pub mod hooks_cache {
    wasmtime::component::bindgen!({ path: "../../wit", world: "hooks-cache" });
}

/// The `hooks-trust` world (gh #45): project-trust votes.
pub mod hooks_trust {
    wasmtime::component::bindgen!({ path: "../../wit", world: "hooks-trust" });
}

/// The `serve-http` world (gh #363): the host's listener calls in.
pub mod serve_http {
    wasmtime::component::bindgen!({ path: "../../wit", world: "serve-http" });
}
