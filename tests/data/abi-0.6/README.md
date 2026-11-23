# A frozen 0.6 artifact, kept on purpose

`tool-legacy.wasm` is a component built for **`lca:ext@0.6.0` / `lca:host@0.6.0`**
— the single-tool guest that every "additive for old artifacts" proof used
(gh #77's `tool-catalog`, gh #302's `net`, gh #407's questionnaire). It is
frozen: the crate at `extensions/tool-legacy` has moved on with the tree (its
own fixture is rebuilt against the current line like every other committed
component), and this copy is the **last 0.6 build**, kept because it is the
only witness for what a line move actually does to an installed extension.

What it proves, and why it cannot be rebuilt:

* **The manifest window admits a 0.6 extension** on a 0.7 host
  (`abi_in_window`), so the loader reaches the link step rather than refusing
  the manifest.
* **The link then refuses**, because opening a line renames the WIT package
  (`lca:host@0.6.0` → `lca:host@0.7.0`) and a component's type names the
  versions it imports and exports. `LoadError::link` turns wasmtime's sentence
  into the one thing a user can do (rebuild against this host's `wit/`).
* **Therefore "additive for old artifacts" is a within-line property.** It held
  for the `tools`, `net` and `questionnaire` additions because the package
  version did not move; a line move rebuilds every component, which is exactly
  why a released line is frozen (docs/abi-versioning.md).

Rebuilding this file would destroy its only reason to exist, so it is
deliberately **outside** `extensions/*/fixtures/`: `scripts/wasm-check.sh`
compares every committed fixture against a fresh build by interface, and a
by-definition-stale artifact would either redden that gate or force an
exclusion list into it. The rows that use it live in
`tests/regressions/302-tool-world-net-import.rs` and
`tests/regressions/407-questionnaire-abi.rs`; if the line moves again, this file
stays as the 0.6 witness and a second directory (for 0.7) is what a new one
would be.
