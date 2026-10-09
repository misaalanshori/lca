# Kimi Code provider

Source: `extensions/kimi-coding/`. Delivery: WASM, installed separately, not bundled by default. The token-triple proof: Kimi's device flow answers access + refresh + expiry, and the refresh retries rate limits with backoff before declaring the credential dead.

## What it authenticates against

A Kimi Code subscription through the RFC 8628 device flow (pi's `packages/ai/src/auth/oauth/kimi-coding.ts`): the device authorization at `{oauth_host}/api/oauth/device_authorization` (`oauth_host` stored override plays pi's `KIMI_CODE_OAUTH_HOST` role), the *complete* page with its code fragment (the kit prefers `verification_uri_complete`, pi's display choice), the poll to the triple. Refresh retries 429/5xx three times with backoff; 401/403/`invalid_grant` purges with the re-login error — pi's rules.

## Manifest

```toml
name = "kimi-coding"
version = "0.1.0"
abi = "0.6"
worlds = ["provider"]
description = "Moonshot Kimi models via Kimi Code subscription."

[capabilities.net]
hosts = ["auth.kimi.com", "api.kimi.com"]

[capabilities.credentials]
namespace = "kimi-coding"
```

No options, no loopback: `/login kimi-coding` runs the identity flow directly.

## Requests

Chat bodies over `POST {api_base}/chat/completions` (`https://api.kimi.com/coding/v1` default) with the token bearer. The token budget is the turn's explicit budget when set.

## login, logout, usage

`login` runs device → poll → triple. `logout` clears the namespace. `usage` is the credential-validity probe — a live token is `Ok` with empty counts.

## Limits

The static table names the issue's two rows, both window-unknown (0, never compacts) until livedata says otherwise. Device turns never run in CI — the suite drives the mock gateway, and the live smoke skips without an operator-provided token (NFR-23).
