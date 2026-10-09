# Meta provider

Source: `extensions/meta/`. Delivery: WASM, installed separately, not bundled by default. The split-identity proof: Meta's device flow yields an identity token that inference refuses, so the extension mints a day-lived Model API key from it — identity as `refresh`, key as `access`, pi's exact split.

## What it authenticates against

A Meta account / Muse subscription through the RFC 8628 device flow (pi's `packages/ai/src/auth/oauth/meta.ts`): the device authorization at `auth.meta.com`, the code page with its fragment, the poll to an identity token, then the mint at `api.meta.ai/muse-code/key` (version + identity as headers, empty body). Refresh is the same mint; the identity itself never renews, so a 401/403 on mint purges the trio and names `/login meta` — pi's rule verbatim.

## Manifest

```toml
name = "meta"
version = "0.1.0"
abi = "0.6"
worlds = ["provider"]
description = "Meta Llama and Muse models via Meta account subscription."

[capabilities.net]
hosts = ["auth.meta.com", "api.meta.ai"]

[capabilities.credentials]
namespace = "meta"
```

No options, no loopback: `/login meta` runs the identity flow directly.

## Requests

Chat bodies over `POST {api_base}/v1/chat/completions` (`https://api.meta.ai/v1` default) with the minted bearer. The token budget is the turn's explicit budget when set.

## login, logout, usage

`login` runs device → poll → first mint. `logout` clears the namespace. `usage` is the credential-validity probe — a live key is `Ok` with empty counts.

## Limits

The static table names the issue's two rows: Llama 3.3 70B with Meta's published 128k context, Muse Spark window-unknown (0, never compacts) until livedata says otherwise. Device turns never run in CI — the suite drives the mock gateway, and the live smoke skips without an operator-provided key (NFR-23).
