# Codex provider

Source: `extensions/codex/`. Delivery: WASM, installed separately, not bundled by default. The second subscription-gateway proof after Antigravity — and the first built on the shared subscription kit (`crates/lca-subscription`), so the OAuth flow and the Responses protocol are spec tables here, not a second implementation.

## What it authenticates against

A ChatGPT Plus/Pro/Business/Enterprise/Edu subscription, through the loopback OAuth PKCE flow: the extension builds the authorization request (client `app_EMoamEEZ73f0CkXaXp7hrann`, the `codex_cli_simplified_flow` flag, `originator` spelled `lca`), the host runs the loopback listener behind `oauth.begin`/`oauth.await`, the code exchanges at `auth.openai.com/oauth/token`, and the ChatGPT account id is decoded from the access token's JWT claim. Headless machines use the paste shape (`lca auth login --provider codex`). A login always re-runs the flow; a 401 purges the tokens.

## Manifest

```toml
name = "codex"
version = "0.1.0"
abi = "0.5"
worlds = ["provider"]
description = "OpenAI Codex models via ChatGPT subscription login."

[capabilities.net]
hosts = ["chatgpt.com", "auth.openai.com"]

[capabilities.oauth]
redirect_path = "/callback"

[capabilities.credentials]
namespace = "codex"
```

Two notes against the September spec this replaces: `worlds` is `provider` only — identity commands arrive host-namespaced through the provider world (FR-PROV-10), the way the other providers work; and the API host is `chatgpt.com` (the `/backend-api/codex/responses` gateway), not `api.openai.com`, which serves a different API. `net` needs both hosts because the token exchange and the inference traffic land on different domains.

## Requests

Responses-protocol bodies over `POST {api_base}/codex/responses` (`store: false` always): the bearer, the `chatgpt-account-id` header decoded from the JWT, `originator: lca`, and `OpenAI-Beta: responses=experimental`. The reasoning effort rides when the caller names one (`minimal` maps to the wire's `low`).

## login, logout, usage

`login` runs the kit OAuth flow and stores access, refresh, expiry, and account id. `logout` clears the namespace (the gateway exposes no subscription-token revoke). `usage` is the credential-validity probe these gateways get — no quota endpoint exists, so an unexpired (or refreshable) token is `Ok` with empty counts, which is what makes `auth check` answer `ready` for a live login.

## Limits

The static table names one model (`gpt-5.4-mini`, window 272000 from fx's catalog fixture); it grows with livedata, never guesses. Subscription turns never run in CI — the suite drives the mock gateway, and the live smoke skips without an operator-provided token (NFR-23).
