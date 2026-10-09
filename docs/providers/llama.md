# llama-server provider

Source: `extensions/llama/`. Delivery: WASM or native; the management commands need the native delivery (below). The local proof: no subscription, no OAuth, no cloud — an external `llama-server` router on loopback, reached through `net-local` (pi supervises no child process either).

## What it authenticates against

Nothing, by default: the server is yours. An optional bearer rides when configured (stored key, native `LLAMA_API_KEY`); the endpoint is stored, native `LLAMA_BASE_URL`, else `http://127.0.0.1:8080` — normalized like pi (no trailing slash, no `/v1` tail). `/login llama` stores the endpoint and probes it, so a typo fails at login, not mid-turn. `logout` forgets the stored endpoint; the server keeps running — this extension never supervises it.

## Manifest

```toml
name = "llama"
version = "0.1.0"
abi = "0.6"
worlds = ["provider", "command"]
description = "Local llama-server router models via chat completions and load/unload commands."

[capabilities.net-local]
addresses = ["localhost", "127.0.0.1"]

[capabilities.credentials]
namespace = "llama"
```

No hosted `net`: a loopback provider must not reach the internet. The one deliberate split: the `command` world imports no `net` (link-time refusal by design), so the sandboxed command guest declines in text and the *native* twin runs list/load/unload. Provider streaming works in both modes.

## Requests

Chat bodies over `POST {base}/v1/chat/completions` (pi's inference URL) with the model, streamed messages, tools, and the turn's budget when set. The catalog is the router's own `/models`: statuses (`loaded`, `loading`, `unloaded`, `sleeping`) and windows (runtime `n_ctx`, then the launch `--ctx-size`, then trained `n_ctx_train`, pi's precedence — server args arrive as strings, both shapes parse). Decision-only models stay out; there is no curated floor for your own GGUFs — a down server lists nothing and names `llama-server --port 8080`.

## Commands

`/llama list | load <model> | unload <model>`: one-shot router calls answering text. Downloads, progress watches, and classifier fallbacks from pi's interactive UI stay out — the thin command covers what the acceptance names.

## Limits

The suite drives a mock router (statuses, windows, decision filter, management POSTs); the live smoke needs a pointed-at server with a loaded model and skips without both variables (NFR-23).
