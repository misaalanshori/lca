# OpenAI-compatible provider

Version 0.1, 2026-09-20.

Source: `extensions/openai-compatible/`. Delivery: native-linked, enabled by default via the `bundled-openai-compat` Cargo feature on `lca-cli`, per ADR-0013's build-time-backend-versus-runtime-extension distinction — this is a runtime extension in the sense that mattered for that record, since it is capability-gated and labeled in the extension list, but it ships in the binary rather than requiring an install step.

## What it authenticates against

Any HTTP endpoint that speaks the OpenAI chat completions request and response shape: the request has a model field, a messages array, and a tools array in the now-conventional layout; the response streams the same shape back. This covers OpenAI itself, and every one of the many services, self-hosted or commercial, that expose the same wire format deliberately for compatibility, OpenCode Go among them.

Authentication is a bearer token in the request header, read from the `credentials` capability, with an environment variable fallback for a user who prefers not to store it through the agent. Every request also carries `x-opencode-session`, the conversation's session id (from `extras`), which OpenCode Go refuses requests without and every other OpenAI-shaped server ignores; see ADR-0023.

## Manifest

```toml
name = "openai-compatible"
version = "1.0.0"
abi = "0.1"
worlds = ["provider", "command"]
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

`login` is not OAuth-shaped for this provider; it opens a modal, through the `ui` capability, prompting for a base URL and a key, writes the key through `credentials` on submission, and, if the submitted host differs from the default, prompts for the ad hoc `net` grant that host needs at that same moment rather than asking for it separately. `logout` clears the stored key; it does not revoke the ad hoc host grant, since a user is more likely to log back in with the same endpoint than to want the grant quietly removed. `usage` returns "not supported," since there is no single usage endpoint this provider can assume exists across arbitrary OpenAI-compatible servers.

## Cache behavior

Most OpenAI-shaped endpoints, including OpenAI's own, cache automatically with no explicit marker required, so this provider generally ignores the cache-boundary hint rather than acting on it. It still reports `cache_read` and `cache_write` from the response's usage fields whenever the configured endpoint provides them, since that costs nothing and is what makes the cache-waste measurement in `docs/testing-plan.md` work for whatever server a user pointed this provider at.

## Why native-linked by default

This is the provider a fresh install needs before it can do anything at all, per FR-PROV-6's requirement that the agent report plainly when no provider is available. Shipping it enabled removes the otherwise-circular problem of needing to already have a working agent to install the thing that makes the agent work. A user who wants zero providers can disable it, which is a supported, ordinary configuration, not an edge case, per ADR-0013's classification table.
