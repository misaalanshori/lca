# ADR-0031: The `provider` world gains a login surface; presets are extension data

Status: accepted.

Date: 2026-09-26.

## Context

pi's `/login` offers "Sign in with an account" and "Sign in with an API
key"; the API-key path is trivial in principle — preset base URLs and a
masked prompt. The question was where preset knowledge lives. Earlier
drafts put a preset table in the host, but that puts provider-shaped data
(the OpenAI dialect's endpoints, flags, model lists) into the core, eroding
the property the architecture is built on: **the core is wire-clean** —
only provider extensions know wire shapes, so switching dialects is
switching extensions and a pure-WASM inference extension stays possible.

The host nonetheless must drive the login UI: the `ui` world deliberately
has no input functions (ADR-0024's conservatism), so no extension can
prompt for a secret.

## Decision

A small **login surface on the `provider` world**: `login-options()` returns
the extension's login options and presets (which the extension loads from
its own `resources/` bag — ADR-0030), and `login-submit(choice, fields)`
consumes the user's answers, stores the secret in the extension's own
`credentials` namespace, and returns opaque `setting: value` pairs.

The host is **UI + courier + consent**: it renders the picker (pi's
two-option wording, plus an always-present "Custom endpoint…"), ferries the
typed secret through the masked prompt, persists the opaque settings it is
handed (it owns the config file *format*, not the settings' *meaning*),
runs the ad hoc `net` grant showing the exact host (FR-PERM-16's deliberate
flow), and nothing more. It never parses provider-shaped data.

Wire identity follows the same split: `prompt_cache_key` and session
affinity are the extension's business (it builds the request body); the
host stamps one generic `User-Agent` at the `net` gate when the caller sets
none — traffic hygiene, not provider semantics.

## Alternatives considered

- **Host-side preset table (both earlier drafts).** Provider knowledge in
  the vendor-agnostic core; the picker outlives the extension it configures
  when the extension is disabled. Rejected.
- **Presets declared in the manifest.** The host still parses
  provider-shaped data, and manifests become mini-databases. Rejected.
- **Input functions on the `ui` world.** Grows the frozen-adjacent UI
  surface into the ABI and fragments the UX across extensions. Rejected —
  the host owns interaction chrome.
- **Extension-only login with no host involvement.** Impossible: no input
  functions exist. Not an option.

## Consequences

`/login` becomes a picker fed by installed provider extensions; disabling an
extension removes its presets with it; a future provider (an
anthropic-shaped dialect, an exotic inference extension) ships its own
login options without host changes. The user override file
(`<config>/provider-presets.toml`) merges as named custom endpoints — host
data, not vendor data. ABI addition on the 0.2 line with the window's
discipline: ADR here, `wit/CHANGELOG.md` line, conformance (options round
trip, submit storing a credential, denial cases) in the same change.
