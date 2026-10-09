# Anthropic provider

Source: `extensions/anthropic/`. Delivery: WASM, installed separately, not bundled by default. The Messages-API proof: prompt-cache breakpoints, interleaved thinking with signatures (P5A), and the Claude Pro/Max subscription login beside a plain API key.

## What it authenticates against

Either an `sk-ant-...` API key (stored, or `ANTHROPIC_API_KEY` in the native form), or a Claude Pro/Max subscription through pi's OAuth client (`packages/ai/src/auth/oauth/anthropic.ts`): the browser loopback flow or the copy-code paste. The token exchange is JSON (`platform.claude.com/v1/oauth/token`) — form would 400 — and pi keeps no account (opaque tokens), so neither does this extension. Headless machines use the paste shape (`lca auth login --provider anthropic` runs the browser flow; `/login anthropic` offers the copy-code method too). A 401 purges the token trio.

## Manifest

```toml
name = "anthropic"
version = "0.1.0"
abi = "0.6"
worlds = ["provider"]
description = "Anthropic Claude models via API key or Claude Pro/Max subscription."

[capabilities.net]
hosts = ["api.anthropic.com", "claude.ai", "platform.claude.com"]

[capabilities.oauth]
redirect_path = "/callback"

[capabilities.credentials]
namespace = "anthropic"
```

Two notes against the September spec this replaces: `worlds` is `provider` only — identity commands arrive host-namespaced through the provider world (FR-PROV-10), the way the other providers work; and there is no `[capabilities.env]` section (that grant waits on #170 — the native form reads `ANTHROPIC_API_KEY`/`ANTHROPIC_BASE_URL` from the environment the way `openai-compatible` does, and the sandbox never sees them).

`net` needs all three hosts because the authorize page, the token exchange, and the inference traffic land on different domains.

## Requests

Messages bodies over `POST {api_base}/v1/messages` with cache breakpoints on: `x-api-key` (the key or the subscription access token — pi sends both as `x-api-key`), `anthropic-version: 2023-06-01`, and the cache + thinking betas (plus pi's `claude-code-20250219,oauth-2025-04-20` pair on a subscription token). The token budget is the turn's explicit budget, then the model's table ceiling. Thinking signatures and budgets ride the shared wire kit (P5A); the frozen tool list from #201 stays future work (no turn-state store yet — the betas above do not include it).

## login, logout, usage

`login` offers three choices: the API key, the browser subscription, and the copy-code subscription. `logout` clears the namespace (no subscription-token revoke exists). `usage` is the credential-validity probe these gateways get — a live token is `Ok` with empty counts, which is what makes `auth check` answer `ready` for a live login.

## Limits

The static table names the issue's four models (windows and ceilings from Anthropic's documented Claude 3 generation); it grows with livedata, never guesses. Subscription turns never run in CI — the suite drives the mock gateway, and the live smoke skips without an operator-provided key (NFR-23).
