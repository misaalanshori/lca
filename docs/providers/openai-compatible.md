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
abi = "0.5"
worlds = ["provider"]
description = "Any OpenAI-compatible chat completions endpoint. Configurable base URL and key."

[capabilities.net]
hosts = ["api.openai.com"]

[capabilities.credentials]
namespace = "openai-compatible"
```

The static `net` grant covers the sensible default, OpenAI's own API, since that's the common case and a manifest declaring nothing at all would leave even the default configuration unable to connect. A user pointing this provider at a different OpenAI-compatible endpoint, which is the entire reason this provider exists, adds that host as an ad hoc grant through the same modal that asks for the base URL, per the ad hoc mechanism described in the capability catalog's `net` section. This is different from Antigravity or Codex, both of which talk to exactly one vendor and can name their full host set in the manifest up front; a manifest cannot declare "any host the user later types in," and a bare wildcard is rejected at install time for exactly that reason, so the ad hoc path is the correct mechanism here, not a workaround for one.

## vendor-event usage

None. The OpenAI chat completions shape is what the typed provider stream cases were modeled on in the first place, per ADR-0004, so nothing here needs the escape hatch.

## login, logout, usage

`login` is not OAuth-shaped for this provider. It reads `OPENAI_BASE_URL` (default `https://api.openai.com/v1`) and, when no key is configured, the host prompts for the API key in a masked modal and stores it through this extension's `credentials` namespace; `OPENAI_API_KEY`/`OPENCODE_API_KEY` remain the non-interactive path. A base URL matching an `auth = "none"` preset (Ollama, LM Studio) needs no key at all (gh #211): the login succeeds keyless and stores only the base URL. `logout` clears the stored key; it does not revoke an ad hoc host grant, since a user is more likely to log back in with the same endpoint than to want the grant quietly removed. `usage` returns "not supported," since there is no single usage endpoint this provider can assume exists across arbitrary OpenAI-compatible servers.

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

**The environment acts on the default profile.** `OPENAI_BASE_URL` and
`OPENAI_API_KEY` keep their precedence (environment over what login
persisted) and apply to default-profile models only; a named profile is
its own, environment notwithstanding.

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
