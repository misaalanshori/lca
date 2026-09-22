# Headless mode and the scripting contract

Version 0.1, 2026-09-20.

`lca -p "prompt"` runs one turn without an interactive interface and writes the result to standard output (FR-CORE-3). Everything a script depends on is stable within a major version: the `--json` envelope may gain fields, but fields are never removed or retyped, and exit codes never change meaning (release policy).

Headless mode makes no network request a script did not ask for: the update check is off unless enabled (FR-CFG-6), and there is no telemetry (FR-CFG-3). The provider call itself is, of course, the request the script asked for.

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
| 4 | Permission denied: an action needed approval and headless mode cannot prompt (FR-TOOL-3). |
| 5 | Turn aborted: tool timeout, iteration limit (FR-CORE-9), or a rejection from a hook or `context-transform` extension (FR-CTX-3). |
| 6 | Session error: the named session is missing, malformed, or belongs to another project. |

A run that ends non-zero still writes its session records; whatever completed before the failure is durable (FR-CONC-3).
