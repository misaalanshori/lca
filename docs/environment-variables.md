# Environment variables

LCA reads the environment in four places: host configuration,
provider credentials, proxies, and the session context shell
children see. `lca config` prints the merged result with the
winning source per key, so when a variable and a file disagree you
can see which won.

## Host configuration (`LCA_` scheme)

Any configuration key can come from the environment: uppercase it,
prefix `LCA_`, replace dots with underscores. `tool.timeout_seconds`
is `LCA_TOOL_TIMEOUT_SECONDS`; `models.enabled` is a comma list in
`LCA_MODELS_ENABLED` (or `--models`). Precedence is flags, then
environment, then the project file, then the user file, then
defaults ([configuration](configuration.md)).

Three variables stand outside the scheme:

| Variable | Meaning |
|---|---|
| `LCA_HYPERLINKS` | `1` forces OSC 8 hyperlinks on, `0` forces the `text (url)` fallback; unset leaves the capability ladder ([terminal setup](terminal-setup.md)) |
| `LCA_PROGRAM_STATUS` | `1` forces OSC 7501 tab-status reports on, `0` silences them; unset reports only after the terminal answers the support query ([terminal setup](terminal-setup.md#tab-status-gh-213)) |
| `LCA_LOG` | Tracing directives (`tracing-subscriber` filter syntax); then `RUST_LOG`, then `warn` (`debug` under `--verbose`). Logs land in `~/.lca/logs/lca.log`, bounded at 1 MiB with one spare. With the `debug-trace` feature (unstable/dev builds) the same variable drives the debug files instead: `LCA_LOG="lca_core=trace"` masters domains `lca_core`, `lca_session`, `lca_permissions`, `lca_registry` (into `debug.log`) and `lca_net`, `lca_wasm` (into `debug-wire.log`), each 5 MiB with one spare; `--debug` / `--debug-wire` enable them without the variable. Secrets never land: values emit redacted (prefix plus `[REDACTED]`), matched by **normalized key name** — lowercased with `_`/`-` stripped, exact or suffix — so `access_token`, `id_token`, `accessToken`, `Access-Token`, `client_secret`, and the OAuth callback `code` all mask, while usage counters (`max_tokens`, `input_tokens`, `prompt_tokens`) and `token_type` stay visible (not credentials). Log files are owner-only (Unix `0600`, repaired on open even for a pre-existing world-readable file; on Windows the `%USERPROFILE%` profile ACL — user + SYSTEM + Administrators) |
| `LCA_OFFLINE` | `1` silences automatic network for the run (update checks, catalog refreshes, remote installs); model traffic still flows. Same as `--offline` |

## Provider credentials (`OPENAI_` shape)

The bundled `openai-compatible` provider reads its credentials from
the environment, which beats what `/login` persisted — for the
default profile only (a named profile keeps its own endpoint and
key). Full table in [configuration](configuration.md#environment):
`OPENAI_BASE_URL`, `OPENAI_API_KEY` (`OPENCODE_API_KEY` also
accepted), `OPENAI_MODEL` (`LCA_MODEL` also accepted),
`OPENAI_CONTEXT_WINDOW`, `OPENAI_PROMPT_CACHE_KEY=0` to stop sending
the cache-affinity key, `OPENAI_SUPPORTS_REASONING=0` to stop sending
`reasoning_effort`. The last two exist for strict proxies that
reject unknown body fields. Other provider extensions define their
own variables; check their pages under
[providers](providers/openai-compatible.md).

A host the manifest does not cover still needs its grant: set the
URL all you like, the first request offers the ad hoc grant
interactively and headless exits 4. Consent is not configurable
into existence.

## Proxies (standard names, no prefix)

`HTTP_PROXY` / `HTTPS_PROXY` (lowercase honored), `ALL_PROXY` as
fallback, `NO_PROXY` for bypasses (curl tail match, `:port`
qualifiers, `*`). They route the `net` host client's outbound
requests only; loopback targets need an explicit `NO_PROXY` entry
or they route into the proxy too. Read once per client build — a
change needs a restart. A proxy URL carries credentials, so it
lives here by decision and never in a config file.

## What shell children see

Every shell command a turn runs inherits the session context as
`LCA_*`: `LCA_SESSION_ID`, `LCA_SESSION_DIR`, `LCA_PROVIDER`,
`LCA_MODEL`, `LCA_THINKING` (unset when the session sets no
level), `LCA_DATA_DIR`. Identifiers and paths only — nothing
secret, safe to echo:

```sh
printf '%s/%s thinking=%s\n' "$LCA_PROVIDER" "$LCA_MODEL" "${LCA_THINKING:-unset}"
```

## File locations (not variables, but looked for here)

The agent's own files live under `~/.lca` on every platform:
`config.toml`, `themes/`, `ui.json` (the `/fullscreen` choice),
`provider-presets.toml`, `logs/lca.log`. The platform config
directory (`$XDG_CONFIG_HOME`, `~/Library/Application Support`,
`%APPDATA%`) is *not* where LCA's files live — it is what the
`home-config` fs scope resolves to, for reading other tools'
configuration.
