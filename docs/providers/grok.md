# Grok provider

Source: `extensions/grok/`. Delivery: WASM, installed separately, not bundled by default. The third subscription gateway on the shared subscription kit (`crates/lca-subscription`): the OAuth flow and the Responses protocol are the kit's, parameterized by this gateway's spec table.

## What it authenticates against

A SuperGrok or X Premium subscription, through the loopback OAuth PKCE flow against `auth.x.ai`: client `b1a00492-073a-47ea-816f-4c329264a828`, scope `openid profile email offline_access grok-cli:access api:access`, `referrer` spelled `lca`. The code exchanges at `auth.x.ai/oauth2/token`, and the account id comes from a `GET` on the userinfo endpoint (`sub` field) — unlike Codex, whose account rides the JWT. Headless machines use the paste shape (`lca auth login --provider grok`). A login always re-runs the flow; a 401 purges the tokens.

## Manifest

```toml
name = "grok"
version = "0.1.0"
abi = "0.6"
worlds = ["provider"]
description = "xAI Grok models via SuperGrok / X subscription."

[capabilities.net]
hosts = ["cli-chat-proxy.grok.com", "auth.x.ai"]

[capabilities.oauth]
redirect_path = "/callback"

[capabilities.credentials]
namespace = "grok"
```

Only the two called hosts are declared: the proxy and the IdP. `grok.com` and `x.ai` never see traffic from this extension and are deliberately absent.

## Requests

Responses-protocol bodies over `POST {api_base}/v1/responses` (`store: false` always): the bearer plus the proxy's header block (`X-XAI-Token-Auth: xai-grok-cli`, the client version and `lca` identifier, the `x-grok-model-override` model, the `x-grok-user-id` account). The reasoning effort rides when the caller names one. The request carries user text and images: an image block serializes into the same input message's content array as an `input_image` part with the bytes inlined as a data URL (gh #338), so an image-only message still reaches the model. The terminal `response.completed` object's `usage.input_tokens_details.cached_tokens` maps to `cache_read`, so a prompt-cache hit is reported as a hit rather than billed as fresh input (gh #338).

## login, logout, usage

`login` runs the kit OAuth flow and stores access, refresh, expiry, and account id. `logout` revokes the refresh token at the IdP (RFC 7009 form), then clears the namespace. `usage` is the credential-validity probe: no quota endpoint exists, so an unexpired (or refreshable) token is `Ok` with empty counts, which is what makes `auth check` answer `ready`.

## Limits

The static table names one model (`grok-4.20`, the id fx's gateway tests drive; the window is unpublished, so `0`, never a guess). Subscription turns never run in CI — the suite drives the mock proxy, and the live smoke skips without operator-provided token and account (NFR-23).
