# Headless mode and the scripting contract

Version 0.1, 2026-09-20.

`lca -p "prompt"` runs headless turns without an interactive interface and writes the result to standard output (FR-CORE-3, pi's `-p`/`--print` shape per #109). The invocation contract:

- `-p [<text>]` / `--print [<text>]`: print mode. A bare `-p` runs the positional messages; `-p <text>` prepends one message (an empty `-p ""` behaves like bare `-p`: the empty string is the documented missing-value edge and contributes no message).
- `--prompt <text>`: the legacy spelling, still headless, still one value.
- Positional `messages...`: with print mode they run as successive turns in one session, in order; without it the TUI opens with the first submitted (#109). `@file` expansion is not supported here (it is #71's scope).
- `--model <pattern>`: the turn runs on that model, resolved the way the interface resolves it, and the model lands on the `assistant` records and `meta.json` (#111).
- `-c` continues the project's most recent session, `-r <id>` resumes that session; both append to the existing `log.jsonl` (#111). `-c` with no session yet exits 6; `-c` with `-r`, or messages with a subcommand, exits 2 with the rule named.
- Print mode with no message at all exits 2. The first failed turn stops a multi-message run; its outcome maps the exit code.

`--attach <path>` adds an image to that turn (repeat for several): the file is content-addressed into the session's attachment store, the message text gains a stub naming it, and a provider that carries vision receives the bytes (ADR-0029). A file that is not a recognized image is refused with exit code 2. Everything a script depends on is stable within a major version: the `--json` envelope may gain fields, but fields are never removed or retyped, and exit codes never change meaning (release policy).

`lca --list-models [search]` is the other half of a script's model workflow (gh #8): it prints `id  provider  context` per offered model and exits 0, with the optional pattern filtering through the same matcher `models.enabled` uses (an empty match says so and still exits 0). The endpoint's consent rule holds for a listing as it does for a turn: an env-configured host no grant covers is refused before anything is reached for - the same message, exit 4, and `--allow-host <host>` allows it for this run. That grant attaches to the process's session set only, because a listing creates no session to record a `permission` answer in.

Headless mode makes no network request a script did not ask for: the update check is off unless enabled (FR-CFG-6), and there is no telemetry (FR-CFG-3). The provider call itself is, of course, the request the script asked for. Two flags a script reaches for when the endpoint is a custom one: `--allow-host <host>` (repeat for several) grants an endpoint host for this run only - the pattern is validated at parse time (a bad one is exit 2), attached to the process rather than to the grant store, and recorded as a `permission` answer with `decision: once`; a later run asks - or exits 4 - again. The interactive alternative is to run `lca` once without `-p` and approve the prompt.

## The `--json` envelope

One JSON object per line, each with a `type` field.

| `type` | Fields | Meaning |
|---|---|---|
| `text` | `content` | Assistant text, streamed as it completes or emitted whole. |
| `tool-call` | `id`, `call_id`, `name`, `arguments` | The model requested a tool call. |
| `tool-result` | `id`, `call_id`, `status`, `content`, `truncated` | A tool call finished. `status` is `ok`, `error`, `denied`, or `timeout`. |
| `usage` | `input`, `output`, `cache_read`, `cache_write`, `cache_write_1h`, `cost` | The turn's usage, cache counts present when the provider reports them. |
| `extension-event` | `extension`, `event`, `detail` | A load, disable, trap, capability denial, or cache divergence. |
| `error` | `message`, `class`, `retryable` | An error that did not necessarily end the run. |
| `turn-end` | `status`, `stop_reason` | The turn finished. `status` is `ok` or `error`. |

## Exit codes

| Code | Meaning |
|---|---|
| 0 | The turn completed. A session that loaded with a truncation warning still exits 0 and reports it on stderr and as a `turn-end` flag. |
| 1 | Unexpected internal error. |
| 2 | Usage error: bad flags or arguments. |
| 3 | Provider error after the retry limit (FR-CORE-6, FR-CORE-7). |
| 4 | Permission denied: an action needed approval and headless mode cannot prompt (FR-TOOL-3). For an endpoint host the environment configured and no grant covers, the message names that host and both fixes - run interactively once, or `--allow-host <host>` for this run (gh #29). |
| 5 | Turn aborted: tool timeout, iteration limit (FR-CORE-9), or a rejection from a hook or `context-transform` extension (FR-CTX-3). |
| 6 | Session error: the named session is missing, malformed, or belongs to another project. |

A run that ends non-zero still writes its session records; whatever completed before the failure is durable (FR-CONC-3).
