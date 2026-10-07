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

## Output modes (`--mode`)

`--mode text` (default) prints the final reply. `--mode json` prints
the event stream below. `--mode rpc` starts the stdin/stdout command
loop. `--json` stays working as a deprecated alias for `--mode json`
(flags are stable within a major); combining `--json` with a non-json
`--mode` is a usage error, and so is `--mode rpc` with prompt
arguments or a subcommand (drive the session from stdin instead).

## The event stream (`--mode json`, `--mode rpc`)

One JSON object per line, each with a `type` field. Stdout carries
records only; diagnostics go to stderr. The stream is additive: kinds
and fields may be added, never removed or retyped (release policy).

| `type` | Fields | Meaning |
|---|---|---|
| `session-start` | `id` | The session these events belong to. |
| `turn-start` | | One turn started. |
| `message-start` | `role` | A `user` or `assistant` message started. |
| `message-end` | `role` | That message completed. |
| `text-delta` | `delta` | An assistant text chunk; concatenate to rebuild. |
| `thinking-delta` | `delta` | A reasoning chunk. |
| `text` | `content` | Assistant text, streamed as it completes or emitted whole. |
| `tool-call` | `id`, `call_id`, `name`, `arguments`, `parent_call_id` | The model requested a tool call. `parent_call_id` is the calling tool's id for nested calls (gh #77), null for model-issued calls. |
| `tool-update` | `call_id`, `chunk` | Live output from a running tool call. |
| `tool-result` | `id`, `call_id`, `status`, `content`, `truncated`, `exit_code`, `full_output_path`, `nested` | A tool call finished. `status` is `ok`, `error`, `denied`, or `timeout`; the last two fields ride only shell results that spilled (gh #40). |
| `usage` | `input`, `output`, `cache_read`, `cache_write`, `cache_write_1h`, `cost` | The turn's usage, cache counts present when the provider reports them. |
| `extension-event` | `extension`, `event`, `detail` | A load, disable, trap, capability denial, or cache divergence. |
| `queue-queued` | `mode` | A `steer`/`follow_up` message was accepted into the queue. |
| `queue-flushed` | `mode`, `text` | A queued message entered the turn. |
| `compaction-start` | `reason` | Compaction began (`threshold` or `manual`). |
| `compaction-end` | `reason`, `success` | Compaction finished; the record holds the summary. |
| `retry-scheduled` | `attempt`, `max_attempts`, `delay_ms`, `error` | A provider retry was scheduled. |
| `retry-end` | `success` | The retry round finished. |
| `error` | `message`, `class`, `retryable` | An error that did not necessarily end the run. A scheduled retry also emits this row for compatibility; prefer `retry-scheduled`. |
| `turn-end` | `status`, `stop_reason`, `truncated_session` | The turn finished. `status` is `ok` or `error`. |

### Pi-shape mapping

Documented mapping, not byte-clone: pi names on the left, ours on the
right. A pi consumer ports by renaming.

| pi (`docs/json.md`) | ours | Notes |
|---|---|---|
| `session` (header) | `session-start` | Ours carries the id only, not the file shape. Emitted in both json and rpc modes (pi omits it in rpc; our clients need the id and have no `get_state`). |
| `agent_start` / `agent_end` / `agent_settled` | — (absent) | One turn is one run here; `turn-end` closes it. No multi-run settling exists to report. |
| `turn_start` / `turn_end` | `turn-start` / `turn-end` | Ours adds `truncated_session` on `turn-end`. |
| `message_start` / `message_update` (`text_delta`) / `message_end` | `message-start` / `text-delta` (+`thinking-delta`) / `text` + `message-end` | Ours carries roles and deltas, not full message objects; `text` is the authoritative whole. No `contentIndex`: one text stream per message. |
| `tool_execution_start` / `tool_execution_end` | `tool-call` / `tool-result` | Same granularity; ours adds `exit_code`/`full_output_path` on shell results. |
| `tool_execution_update` | `tool-update` | Live tool output chunks (`call_id`, `chunk`). |
| `queue_update` | `queue-queued` / `queue-flushed` | Ours splits accept and drain into two kinds with the ADR-0038 mode. |
| `entry_appended`, `session_info_changed`, `thinking_level_changed` | — (absent) | No `pi.appendEntry` / display-name / level events cross this stream today. |
| `compaction_start` / `compaction_end` | `compaction-start` / `compaction-end` | Ours names `threshold`/`manual` reasons and a bare success. |
| `auto_retry_start` / `auto_retry_end` | `retry-scheduled` / `retry-end` | Same fields, our names. |
| `summarization_retry_*` | — (absent) | Compaction runs once here; no retry taxonomy around it. |
| `bash_execution_update` | — (absent) | No direct-bash RPC command (see below); shell output streams as `tool-update`. |
| `extension_error` | `extension-event` | Different semantics: pi reports a throwing handler, ours reports lifecycle (load/disable/trap/denial). |
| `response` | `response` | Same shape (`command`, `success`, `data`/`error`, echoed `id`); ours adds a `parse` command for malformed lines. |

## The RPC loop (`--mode rpc`)

`lca --mode rpc` (session flags like `--model`, `-c`/`-r` select the
session; `--yolo` and `--allow-host` behave as in `-p`) reads one JSON
object per line on stdin and writes records on stdout: one `response`
per command plus the event stream above. Multi-turn state lives in the
one session across commands. `shutdown` — or closing stdin — persists
the session (`session-end` record, close hooks) and exits 0. Startup
failures exit with the codes below before any record.

Commands (`id` optional, echoed on the response):

| `type` | Fields | Response `data` |
|---|---|---|
| `prompt` | `message`, `streamingBehavior` (`steer`/`followUp`, when streaming), no `images` in v1 | `{"disposition": "started"}` (a run began) or `"queued"` (accepted mid-run) |
| `steer` | `message` | `{"disposition": "queued"}` (drains at the next model-call boundary, ADR-0038) |
| `follow_up` | `message` | `{"disposition": "queued"}` (submits after the run ends, even a cancelled one) |
| `cancel` | | `{}` (fires the turn's cancellation; a no-op when idle) |
| `shutdown` | | `{}` (drains queued follow-ups first, then exits 0) |

Rejections (`success: false`): unknown commands, missing `message`,
a `prompt` with no `streamingBehavior` while streaming, `images` in
v1, prompts after `shutdown`, and malformed lines (`command:
"parse"`, no id). A failure after acceptance surfaces as events, not
as a second response.

The RPC Extension UI sub-protocol is out: extension-UI-over-RPC needs
the ui@0.6.0 world work (#172's train).
## The `auth` commands

`lca auth ...` manages provider credentials without opening the
interface (gh #72, pi's credential commands). `check` probes state
and prints `ready`, `not_ready`, or `invalid` — pi's words, pi's exit
table: 0, 1, or 2. `--json` writes pi's shape
(`{"status","provider","reason?","authType?"}`); `--provider` names
an extension, `--model` resolves through the first provider listing
it, and neither flag is pi's usage error (exit 2, like an
unresolvable model). A `--credentials` flag that emitted the secret
is refused outright, like the printers below.

`auth login --provider <name>` signs in: an API key resolves from
the environment inside the extension, while an OAuth flow prints its
authorization URL and reads the pasted callback from stdin (pi's
remote/headless shape; `docs/flows.md`). The IdP hosts need the same
`--allow-host` this run's turns would. `auth logout --provider <name>`
signs out through the provider's own logout.

`lca` never prints credentials: `auth print-api-key` and `auth
print-bearer-token` fail with the reason instead of the secret, because
tokens in scrollback or history would outlive the command. The
secrets law outranks parity; `--no-refresh` is likewise absent (a
check refreshes like pi's default, and the extension owns the
refresh).

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

For `auth check` the codes are pi's credential table — 0/1/2 is
ready/not_ready/invalid — not the turn rows above.
