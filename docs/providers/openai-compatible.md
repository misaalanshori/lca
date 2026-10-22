# OpenAI-compatible provider

Version 0.1, 2026-09-20.

Source: `extensions/openai-compatible/`. Delivery: native-linked, enabled by default via the `bundled-openai-compat` Cargo feature on `lca-cli`, per ADR-0013's build-time-backend-versus-runtime-extension distinction — this is a runtime extension in the sense that mattered for that record, since it is capability-gated and labeled in the extension list, but it ships in the binary rather than requiring an install step.

## What it authenticates against

Any HTTP endpoint that speaks the OpenAI chat completions request and response shape: the request has a model field, a messages array, and a tools array in the now-conventional layout; the response streams the same shape back. This covers OpenAI itself, and every one of the many services, self-hosted or commercial, that expose the same wire format deliberately for compatibility, OpenCode Go among them.

The model's context window is resolved per model rather than once for the
provider, in the order the [Context windows](#context-windows) section
gives; `0`, the default when nothing is known, means unknown and never
compacts (the FR-SESS-4 compaction threshold needs a window to divide
by). Authentication is a bearer token in the request header, read from the `credentials` capability, with an environment variable fallback for a user who prefers not to store it through the agent. Every request also carries `x-opencode-session`, the conversation's session id (from `extras`), which OpenCode Go refuses requests without and every other OpenAI-shaped server ignores; see ADR-0023.

## Manifest

```toml
name = "openai-compatible"
version = "1.0.0"
abi = "0.6"
worlds = ["provider"]
description = "Any OpenAI-compatible chat completions endpoint. Configurable base URL and key."

[capabilities.net]
hosts = ["api.openai.com"]

[capabilities.credentials]
namespace = "openai-compatible"

[capabilities.env]
keys = [
    "OPENAI_BASE_URL",
    "OPENAI_API_KEY",
    "OPENCODE_API_KEY",
    "OPENAI_MODEL",
    "LCA_MODEL",
    "OPENAI_CONTEXT_WINDOW",
    "OPENAI_PROMPT_CACHE_KEY",
    "OPENAI_SUPPORTS_REASONING",
]
```

`[capabilities.env]` is what makes the environment work at all in the sandbox (gh
#461, gh #170). A WASM component is instantiated with an empty `WasiCtx`, so
`std::env::var` inside it answers `NotPresent` unless the manifest declares the name
and the host forwards the value at load. Without this block the sandboxed provider
behaved as if nothing were configured, while the compiled-in one read the host
environment directly — which is why this provider's endpoint discovery once needed a
hardcoded special case in `lca-cli` (`openai_ad_hoc_host`, gh #157) instead of reading
its own variable. The list is the literals this crate reads, and the install consent
names every one of them.

**One limitation, stated rather than discovered: a named profile's key resolves from
its stored credential under the sandbox.** The per-preset key names (`GROQ_API_KEY`,
`OPENROUTER_API_KEY`, `DEEPSEEK_API_KEY`, and the rest of the table's `env` column)
come from the `provider-presets.toml` resource at run time, so the manifest cannot
enumerate them: a fixed list would go stale the moment a preset is added, and a broad
grant is the disclosure the capability model exists to prevent. The native build still
reads them from the process environment. A sandboxed user who wants a preset's key
should store it through `/login` — which is what the provider's own documented
precedence already prefers for a named profile.

The static `net` grant covers the sensible default, OpenAI's own API, since that's the common case and a manifest declaring nothing at all would leave even the default configuration unable to connect. A user pointing this provider at a different OpenAI-compatible endpoint, which is the entire reason this provider exists, adds that host as an ad hoc grant through the same modal that asks for the base URL, per the ad hoc mechanism described in the capability catalog's `net` section. This is different from Antigravity or Codex, both of which talk to exactly one vendor and can name their full host set in the manifest up front; a manifest cannot declare "any host the user later types in," and a bare wildcard is rejected at install time for exactly that reason, so the ad hoc path is the correct mechanism here, not a workaround for one.

## vendor-event usage

None. The OpenAI chat completions shape is what the typed provider stream cases were modeled on in the first place, per ADR-0004, so nothing here needs the escape hatch.

## login, logout, usage

`login` is not OAuth-shaped for this provider. It reads `OPENAI_BASE_URL` (default `https://api.openai.com/v1`) and, when no key is configured, the host prompts for the API key in a masked modal and stores it through this extension's `credentials` namespace; `OPENAI_API_KEY`/`OPENCODE_API_KEY` remain the non-interactive path. A base URL matching an `auth = "none"` preset (Ollama, LM Studio) needs no key at all (gh #211): the login succeeds keyless and stores only the base URL. `logout` purges everything a login wrote — the bare `api_key`/`base_url`, every preset profile's `profile.<id>.api_key`/`base_url`, and the cached `models`/`model`/`preset` settings (gh #293) — so a signed-out provider reports unconfigured and no secret stays on disk. It does not revoke an ad hoc host grant, since a user is more likely to log back in with the same endpoint than to want the grant quietly removed. `usage` returns "not supported," since there is no single usage endpoint this provider can assume exists across arbitrary OpenAI-compatible servers. `/usage` probes *account* quota (credits, limits, spend): the standard OpenAI chat-completions shape standardizes inference, never billing, so a generic endpoint has nothing to answer with. Per-turn token accounting is a different channel and always works: the footer tracks it live and `/session` reports it. Quota endpoints exist only on providers that document one (e.g. OpenRouter's key endpoint via the `openrouter` extension, or the bundled `antigravity` provider).

A base URL whose host is not covered by the manifest's fixed hosts needs an ad hoc `net` grant (FR-PERM-16). A base URL stored through `login` is offered that grant at login, as before. An endpoint configured **only through the environment** has no such moment, so the first request now raises that same consent, naming the exact host: `allow` persists it for the project (FR-PERM-18/19, in force without a restart, so neither the next turn nor the next process asks again) and `deny` refuses the request and records the answer. Headless mode cannot prompt, so it exits **4** with a message naming the host and the fix; `--allow-host <host>` grants it for that one run without touching the grant store. This is deferred work B1, landed on the request path (gh #29).

## Profiles

One credentials namespace can hold several services at once (gh #31).
Each login's choice is a **profile**: its key and endpoint live under
`profile.<name>.api_key` and `profile.<name>.base_url`, so a second
login adds beside the first instead of overwriting it, and a login for
a profile that already exists updates that profile alone. The bare
`api_key`/`base_url` pair - what every install written before profiles
holds - reads as the unnamed **default** profile, unchanged.

The `models` setting carries each model's profile: `id[@profile][=window]`.
An entry without `@` is a default-profile model, and `=window` parses as
it always did, so both directions stay backward tolerant (the same
discipline as gh #34's window growth).

**Routing follows the model.** A request for a model of profile P is
built from P's endpoint and P's key; a model the list does not tag is a
default-profile request. That is what makes the picker's label honest:
every row shows `model (service)` from the model's own `extras`, never
the extension's crate name, so the row says which service and key will
be billed.

**The environment resolves a profile's key in one documented order.**
For the **default profile** that order is: `OPENAI_API_KEY`,
`OPENCODE_API_KEY`, then the active preset's own declared `env` names in
declaration order (gh #292), then what login persisted (the bare
`api_key`). For a **named profile** it is: its preset's own declared `env`
names, then its stored `profile.<id>.api_key`, then the bare `api_key`.
The two universal names are the default profile's, so a named profile
never resolves them — otherwise an `OPENAI_API_KEY` exported for direct
OpenAI use would be sent to Groq and DeepSeek too, which is the same
wrong-secret-to-the-wrong-endpoint mistake as reading another service's
variable.

Only the **active** preset's names are ever read, so a `GROQ_API_KEY` in
the environment cannot authenticate a `deepseek` profile — the endpoint
never receives a secret the user associated with a different service. A
profile whose id is not in the preset table reads no environment at all
and falls straight through to its stored credential.

That makes the table's `env` column load-bearing rather than decorative:
exporting the variable a provider's own documentation names is enough to
authenticate it, without aliasing it to `OPENAI_API_KEY` or typing it into
`/login` (gh #292). The endpoint still comes from the preset or the stored
`base_url`; only the key resolution changed.

**The base URL is unchanged.** `OPENAI_BASE_URL` keeps its precedence for
the default profile and applies to default-profile models only; a named
profile keeps its own endpoint, environment notwithstanding.

**Reasoning support follows the same "the active preset's own row" rule.**
`provider-presets.toml` declares `supports_reasoning` per preset
(`opencode-go` is the one that does), and the request carries a
`reasoning_effort` field exactly when the preset the request routes to
declares it (gh #294). It used to read a single flag fixed at startup from
`OPENAI_SUPPORTS_REASONING`, which cannot know which profile the selected
model routes to: the declaration was inert and every other preset
inherited whatever the environment said. A profile whose id is not in the
preset table still uses `OPENAI_SUPPORTS_REASONING`, because that is the
endpoint-configured-from-the-environment path and it has no declaration to
read. An undeclared preset never inherits the ambient answer.

**Two presets reach a server on this machine, and only those.** `ollama`
and `lmstudio` point at `http://localhost:<port>/v1`, so the manifest
declares `net-local` for `localhost` and `127.0.0.1` (gh #294). It is
those two literals and nothing broader — no CIDR block, no `*.local` — and
that matters because `net` refuses a loopback address outright
(FR-PERM-13's rebinding rule), so before the grant their model discovery
failed before any consent could run. An internet-flavoured preset still
reaches its host through the ad hoc `net` consent at login, as above.

## `ModelInfo.extras` keys

`list-models` entries carry an `extras` map for non-structural per-model data (gh #108: the exact strings are the cross-boundary contract — a WASM guest in any language writes them, the host reads them, and a typo on either side fails silently, so the vocabulary test pins them against the `lca-protocol` constants):

| Key | Value | Read by |
|---|---|---|
| `image.vision` | `"true"` (takes image input) or `"false"`; absent means unknown and bytes pass through | The tool executor's image policy |
| `image.resize` | `WIDTHxHEIGHT:BYTES` vendor-sourced profile | Image downscaling before send |
| `prompt_cache` | `short=SEC,long=SEC` cache lifetimes | The cache-warming eligibility gate (gh #42, #64) |
| `thinking_levels` | Comma-joined canonical reasoning levels; present-but-empty means the model does not reason | The `/thinking` picker (gh #242) |

## Cache behavior
Most OpenAI-shaped endpoints, including OpenAI's own, cache automatically with no explicit marker required, so this provider generally ignores the cache-boundary hint rather than acting on it. It still reports `cache_read` and `cache_write` from the response's usage fields whenever the configured endpoint provides them, since that costs nothing and is what makes the cache-waste measurement in `docs/testing-plan.md` work for whatever server a user pointed this provider at.

## Context windows

`list-models` carries one `context_window` per model. Two numbers divide by it: the footer's `ctx` share, and the compaction threshold (FR-SESS-4) — so a window that is wrong is worse than one that is missing, because a wrong denominator silently mis-triggers compaction. Four sources, in precedence order (gh #34, gh #64):

1. **`OPENAI_CONTEXT_WINDOW`**, as a whole-provider override. It beats everything below.
2. **The user's `~/.lca/models.toml`** (gh #64): per-model fixes without a release — `context_window`, `input` (vision derives from it), `[model.input_limits.images.resize]`, and `[model.prompt_cache]` lifetimes. An optional `provider` scopes an entry to one provider; unknown ids are ignored. It beats the endpoint and the catalog below, never the explicit variable above. `$VAR`/`${VAR}` values read the environment; a leading `!command` never executes (documented divergence from pi, which runs commands from its models file — running arbitrary commands from a config file turns the permission model inside out).
3. **The endpoint's own answer.** `login` keeps each entry's `context_length` from `GET /models` when the response carries one, and it rides the persisted `models` setting as `id=window` so it survives the round trip. OpenRouter reports that field (`openai/gpt-4o` reads 128000 live and in the catalog below, checked 2026-10-03). OpenCode Go's `GET /models` carries no limit field at all — only `id`, `object`, `created` and `owned_by` — so nothing is read there, and its models fall through to the next source.
4. **The curated catalog**, `extensions/openai-compatible/resources/context-windows.toml`: model id → tokens, with **the source named on every number's own line** (models.dev's provider entry and the fetch date — the same catalog pi's own tables are generated from; pi's footer showing `1.0M` for `mimo-v2.6-flash` matches the 1048576 in that file). It is a separate resource from `provider-presets.toml` because the shipped-data guard scans that file for development-harness ids, and several of these models are now live product models on OpenCode Go.

A model that none of the four sources confirms reports `0`, and the footer keeps `ctx ?` — an honest unknown, never a fabricated share. The persisted shape stays backward-tolerant in both directions: a bare `models` entry carries no window (every list written before gh #34 reads that way), an `id=window` entry carries one, and an id containing `=` is only split when the tail is a number.

## Why native-linked by default

This is the provider a fresh install needs before it can do anything at all, per FR-PROV-6's requirement that the agent report plainly when no provider is available. Shipping it enabled removes the otherwise-circular problem of needing to already have a working agent to install the thing that makes the agent work. A user who wants zero providers can disable it, which is a supported, ordinary configuration, not an edge case, per ADR-0013's classification table.
