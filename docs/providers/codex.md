# Codex provider

Version 0.1, 2026-09-20.

Source: `extensions/codex/`. Delivery: WASM, installed separately, not bundled by default. Same shape as Antigravity; documented separately mainly because a second, independent implementation of the OAuth pattern is what actually proves it generalizes rather than being specific to one vendor's flow.

## What it authenticates against

The ChatGPT subscription login, through the same loopback OAuth mechanism Antigravity uses: the extension builds the authorization request and PKCE challenge, the host runs the actual loopback listener behind `oauth.begin` and `oauth.await`, and the token exchange goes over `net`.

## Manifest

```toml
name = "codex"
version = "1.0.0"
abi = "0.1"
worlds = ["provider", "command"]
description = "OpenAI Codex models via ChatGPT subscription login."

[capabilities.net]
hosts = ["api.openai.com", "auth.openai.com"]

[capabilities.oauth]
redirect_path = "/callback"

[capabilities.credentials]
namespace = "codex"
```

The two-host `net` grant, an API host and a separate auth host, is worth calling out as a small but real variation from Antigravity's single-host-plus-wildcard shape: not every OAuth provider's token exchange and API traffic land on the same domain, and the manifest schema's array-of-patterns design already accommodates this without any special casing.

## vendor-event usage

Carries reasoning-effort and response-format fields specific to OpenAI's API shape where they don't map onto the typed reasoning-delta case cleanly, following the same principle as Antigravity's use of the same escape hatch for a different vendor's specifics.

## login, logout, usage

Same three-function shape as every provider under ADR-0012. `usage` here reports against whatever quota or rate-limit information the API surfaces for the authenticated subscription.

## Cache behavior

OpenAI's API caches automatically with no explicit marker, the same as the OpenAI-compatible provider's usual case, so this provider ignores the cache-boundary hint for marker placement but still reports `cache_read` and `cache_write` from the response's usage fields, which is real, vendor-reported signal for the cache-waste measurement in `docs/testing-plan.md` even though nothing on this provider's side had to act on the hint to produce it.

## Why WASM by default

Same reasoning as Antigravity: nothing about this provider needs first-party trust, and the sandbox costs it nothing it needs to do its job.
