# `extensions/antigravity` — Google subscription models for LCA

A first-party provider extension: Google's subscription-backed models
(Antigravity backend) over a loopback OAuth flow with PKCE. It is the
reference implementation for OAuth providers in this tree (login,
refresh, re-auth) and the most protocol-sensitive extension here —
read the risks section before changing any request shape.

Dual-mode like every extension under `extensions/`: the native and
WASM halves share `src/` and run over the `net`, `oauth`, and
`credentials` capability surface, so neither mode binds a port, opens
a socket, or reads a credential file on its own. User docs live in
`docs/providers/antigravity.md`.

## How it works

- **Auth**: loopback OAuth (PKCE, `S256`). Tokens live in the
  `credentials` capability namespace (`access`, `refresh`,
  `expires`). Login always runs the flow — a stored token never
  short-circuits it, because a stored token may be expired or
  revoked. A `401` purges the stored tokens so the next call
  re-authenticates instead of retrying a dead token.
- **Requests**: Cloud Code Assist endpoint with exactly three
  identifying headers (Bearer token, content type, and the
  `antigravity/cli/...` User-Agent). The backend fingerprints
  clients, so these values are load-bearing, not decorative.
- **Models**: there is no model-list API — the catalog is a hardcoded
  table with a parity test pinning it. Preview IDs that 404 fall back
  to their mapped backend model (`fallback_runtime_model`).
- **Labels**: every request carries deterministic execution labels
  (`last_execution_id` derived from trajectory and step), which the
  backend uses to correlate multi-turn conversations and tool use.
- **Tool schemas**: model-class specific. Gemini models receive
  `parametersJsonSchema`; Claude/GPT-OSS go through an allowlisted
  normalization (`type`, `description`, `properties`, `required`,
  `items`, `enum`) because the bridge strictly rejects Draft-7 /
  2020-12 keywords (`nullable`, `anyOf`, `format`, `$ref`).

## Risks (read before touching request shapes)

Google treats anomalous clients as abuse. Concretely:

1. **Wrong or missing fingerprint headers** (User-Agent, auth scheme)
   can lead to rejected requests or, in the worst case, action
   against the Google account behind the token. Never "simplify" the
   headers; never send a request shape you have not asserted at the
   wire level in the mock tests.
2. **Re-auth loops**: retrying a revoked token looks like credential
   abuse. The 401-purge path exists for this reason — do not add
   retries around authentication failures.
3. **Schema 400s**: unnormalized tool schemas fail loudly per call
   (safe), but do not work around a 400 by resending variants —
   fix the normalization.
4. **Model table drift**: the hardcoded catalog goes stale when
   Google ships tiers. Update the table AND the parity pin together;
   a model ID the backend does not know returns 404 into the
   fallback path, not an error you can ignore.

## Tests

`tests/` covers the OAuth flow (login, refresh, 401-purge), the wire
shapes (headers, labels, fallback routing), and schema normalization
per model class — all against a mock backend. Live turns need a real
Google subscription and never run in CI.
