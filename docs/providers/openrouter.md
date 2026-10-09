# OpenRouter provider

Source: `extensions/openrouter/`. Delivery: WASM, installed separately, not bundled by default. The one-click OAuth proof: a browser loopback flow whose exchange provisions a permanent API key, then chat completions on the shared OpenAI wire kit.

## What it authenticates against

An OpenRouter sign-in through pi's flow (`packages/ai/src/auth/oauth/openrouter.ts`): PKCE, `openrouter.ai/auth` with `callback_url` (not `redirect_uri` — no client id, no scope, no state, which is OpenRouter's shape), and the JSON exchange at `openrouter.ai/api/v1/auth/keys` that answers a permanent key instead of a token pair. The key lands as the access token with no refresh and a never-expiry. Remote sessions paste the redirect URL (the host's callback parser yields the same pairs). Keys stay manual in the `openai-compatible` preset — this extension never takes one, and that preset stays strictly key-only (pinned by its own guard).

## Manifest

```toml
name = "openrouter"
version = "0.1.0"
abi = "0.6"
worlds = ["provider"]
description = "OpenRouter multi-model provider with one-click browser OAuth sign-in."

[capabilities.net]
hosts = ["openrouter.ai"]

[capabilities.oauth]
redirect_path = "/callback"

[capabilities.credentials]
namespace = "openrouter"
```

One note: pi binds a random callback path since no state crosses; this extension takes the fixed `/callback` every other provider uses (single-flight login, the flow handle still gates delivery).

## Requests

Chat bodies over `POST {api_base}/chat/completions`: the bearer (provisioned or `OPENROUTER_API_KEY` in the native form), the model, streamed messages with usage requested, tools when the turn carries them, and gh #169's generation budget. No reasoning mapping (OpenRouter's per-model reasoning shapes are not one field — diverged openly, not guessed).

## login, logout, usage

`login` runs the identity flow directly (no picker presets — the host routes an option-less provider to it, like codex). `logout` clears the namespace (keys are revoked on the site). `usage` is the credential-validity probe — the key never expires, so a present key is `Ok` with empty counts.

## Models

The gateway's own `/models` when it answers (every entry's `context_length` rides — the field OpenRouter publishes), the preset's three curated rows otherwise, never an empty picker.

## Limits

OAuth and keyed turns never run live in CI — the suite drives the mock gateway, and the live smoke skips without an operator-provided key (NFR-23).
