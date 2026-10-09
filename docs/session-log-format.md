# Session log format

Version 0.1, 2026-09-20. Format version 1.

This specifies how a session is stored on disk. The format is durability-critical. A session that cannot be read is work a user cannot get back, so the rules here are stricter than they look necessary.

## Layout on disk

Sessions live under the user data directory, grouped by project.

```
<data-dir>/sessions/
  <project-key>/
    index.json
    <session-id>/
      meta.json
      log.jsonl
      attachments/
        <hash>.bin
```

`<project-key>` is a hash of the canonical path of the working copy, prefixed with the directory name for human readability, as in `myproject-3f9a2c`. The prefix helps someone reading a directory listing. The hash does the work.

`<session-id>` is a sortable identifier with a timestamp prefix, so a directory listing is chronological without reading any file.

`index.json` holds a summary of each session in the project: identifier, title, creation time, last modification time, message count, and parent session for a fork. It is a cache. It is rebuilt from the session directories when it is missing or unreadable, and nothing depends on it being correct.

`meta.json` holds session metadata: format version, creation time, the model and provider last used, the working directory, a title, and the fork origin when there is one.

`log.jsonl` is the record log. It is the authority for everything.

`attachments/` holds content too large for the log, stored by content hash. Images, large tool outputs, and pasted files go here. A record references an attachment by hash. The built-in tools truncate their display at `tool.result_limit_bytes` and spill the untruncated text here as `attachments/<sha256>`; the `tool-result` record's `attachment` field carries that hash. A user message's `attachments` list carries image hashes: `/attach` (or headless `--attach`) stages the file here, the message text gets a `[image attachment <hash8>, <media>, <n> bytes]` stub, and assembly sends the bytes as a typed image block to a provider that carries vision (ADR-0029).

## Record framing

The log is JSON Lines. One JSON object per line, terminated by a newline. No trailing commas, no arrays wrapping the file, no pretty printing.

JSON Lines is chosen for three reasons. Appending is a write with no seek. A truncated final line is detectable and discardable without losing anything before it. A person can read the file with ordinary tools during debugging.

Every line has three fields before anything type-specific: `v` for the record schema version, `t` for the record type, and `ts` for the timestamp in milliseconds since the epoch, in UTC.

```json
{"v":1,"t":"user","ts":1758326400123,"id":"01J...","content":"add a test for the parser"}
```

The `v` field is per record, not per file. A file written across a format upgrade holds records at two versions, and a reader handles each by its own version. This is the property that makes migration possible without rewriting old files.

## Record types

`session-start` is the first record. It holds the format version, the agent version, the ABI version, and the working directory. A log without it as the first record is malformed.

`user` holds one user message. Fields: `id`, `content`, and optional `attachments` as a list of hashes. An optional `queue` field marks a message submitted while a turn was running (ADR-0038): `"steer"` joins the turn's input at the next model-call boundary, `"follow-up"` runs when the turn ends. Its position in the log is its injection point; absent for an ordinary message. Assembly copies the marker into the message's `extras["queue"]`, which is how a context-transform extension sees it.

`assistant` holds one model message. Fields: `id`, `content`, optional `reasoning`, optional `reasoning_signature` (the thinking signature resent verbatim on replay, gh #41), optional `provider_thinking_level` (the level the provider ran at), `model`, `provider`, and `usage` with input tokens, output tokens, cache-read, cache-write, and extended-cache-write tokens, and cost. Reasoning content blocks may carry `signature` beside `reasoning`; old lines without the new fields load with absent values (unknown-fields rule).

`tool-call` holds one call the model requested. Fields: `id`, `call_id`, `name`, `arguments` as a JSON string, and `source` naming whether the tool is built in or comes from an extension.

`tool-result` holds the outcome. Fields: `id`, `call_id` matching the call (a nested call carries `<parent id>/<n>` with `parent_call_id` on the call), `status` of ok, error, denied, or timeout, `content` or an attachment hash, `truncated` as a boolean, and (gh #40) `exit_code` plus `full_output_path` when the tool ran a command and spilled: the process exit code and the session-attachments path holding the untruncated output. Non-shell tools leave both absent. `nested` (gh #77) is the bounded
nested-call record: one entry per directly nested call (`name`,
`status`, `content_head` capped at 500 characters), the first twenty
winning; calls that nested nothing carry none. Nested calls never
appear as their own records - the parent's result is where they
persist. `truncated = true` with an `attachment` means the display shown to the model was cut and the full text is in that attachment; `truncated = true` with no attachment means the content was dropped (no session was attached, as in one-shot use).

`permission` records a grant decision made during the session. Fields: `action`, `decision` of once, always, or denied, and `pattern` when the decision was always. This is a record of what happened, not the grant store itself.

`model-change` records a model switch (gh #8). Fields: `to` (the model now in use), `provider` (the provider extension that serves it), optional `from` (the model it left; absent when the session had no model yet), and optional `profile` when the model belongs to a named profile (gh #31: routing follows the model, so this record says which endpoint the next request bills). Every switch appends one - a `/model` pick, the picker, or a `Ctrl+P` cycle - and it rides alongside the `meta.model` write rather than replacing it. The type is additive: a reader that does not know `model-change` skips the line (see reading and error handling below), which is exactly why a new record type needs no version bump.

`extension-event` records a load, a disable, a trap, or a capability denial. Fields: `extension`, `event`, and `detail`.

`compaction` marks a compaction. Fields: `replaced_from` and `replaced_to` as record identifiers, `first_kept_id` naming the first record kept verbatim past the cut (gh #36 phase 1; empty on older records; the record's own id on a retain-none `/compact`, gh #36 phase 3, so the next plan starts after the entry), `summary` as the replacement content, `strategy` naming the extension that ran, optional `usage` with the input, output, and cost of the summarization call when the strategy asked the model for one, cumulative `read_files` / `modified_files` (gh #36 phase 3, each capped at 200 sorted entries), and optional `system_prompt` checkpointing the prompt at compaction time (gh #36 phase 3; a later move appends a `custom` record of type `system-prompt-change`, the detection, not a migration).

`fork-point` appears in a forked session and names the parent session and the record identifier the fork was taken at.

`branch-point` navigates within one log (gh #37): it names the record the branch continues from (`target_id`). Later records chain through it; the ancestry walk treats it as transparent (jumps to the target, never follows a parent of its own - it carries none). Skipped in display reads, carried in audit and exports.

`branch-summary` records what an abandoned path learned (gh #37, pi's `branch_summary` shape): `parent` names the navigation point explicitly, `from_id` the abandoned tip (`None` when the old path was empty), `summary` the lesson. Assembly injects the summary as context where the new branch continues, like a compaction summary. Carried in display reads, audit, and exports.

`session-end` is written on a clean exit. Its absence means the session ended without one, which is normal after a crash and is not an error.

`thinking-level-change` records a thinking-level switch (gh #47: pi's `thinking_level_change` semantics under this log's framing). Fields: `id`, `level` (the level the next request runs at, or `default` when the session runs the provider's choice - no level in the picker's set is named that). Writer: the `/thinking` picker's host seam (which the `/settings` thinking row shares), only when the effective level actually moved - re-picking the active level writes nothing, the same rule `model-change` follows. Readers: assembly ignores it - the request itself carries the level.

`usage` records model-attributed usage that is not an assistant message and does not enter model context (gh #47: pi's `usage` semantics - cache warms, compaction calls, nested model work an extension reports). Fields: `id`, `kind` (an arbitrary string naming the operation, e.g. `cache_warm`), optional `provider` and `model` naming what did the work, and `usage` with the same input/output/cache/cost shape an `assistant` record carries. Unknown `kind` values are normal usage, never rejected. Writer: the host, on behalf of whatever did the work. Readers: assembly ignores it content-wise; turn totals stay live-measured (a future tree/compaction epic may sum them from the log - the records are there for it). Audit-only on export (see below).

`label` records a user bookmark on an entry (gh #47: pi's `label` semantics). Fields: `id`, `target_id` (the labeled record), `label` (absent clears the bookmark). Writer: the `/label` verb (gh #37: latest message by default, `[n]` for the nth), appending through the host; `/labels` lists, `/jump` branches at the mark. Readers: assembly ignores it; the `/resume` and session-title surfaces will read it later.

`session-info` records the session display name (gh #47: pi's `session_info` semantics): the name the session selector shows instead of the first message. Fields: `id`, `name`. Writer: `store.rename`, shared by `/rename`, `lca rename`, `--name`, and clone titling (gh #37) - the entry trails before the `meta.json` write, so the log witnesses every rename. Readers: `/resume` keeps showing the meta title; `session_name` reads the latest entry back. Readers: assembly ignores it. Kept minimal (name + set) on purpose.

`custom` persists extension state (gh #47: pi's `custom` semantics). Fields: `id`, `custom_type` (which extension owns the entry - readers use it to find their own entries on reload), `data` (the extension's JSON). Writer: the host, appending on the extension's behalf under the capability model - an extension never touches the log file (guest-side imports for this are a minor-version decision for the tree/compaction epics; no `wit/` change in this cycle). Readers: assembly ignores it - it never enters model context. Audit-only on export (see below).
The host's own `custom_type` values: `tool-set-change` (gh #77 -
the active tool set after a mid-turn change, `data.active` naming
every active tool; written before the next model request, so the log
shows what each request declared) and `previous-summary` (gh #36).

`custom-message` is extension context injection (gh #47: pi's `custom_message` semantics). Fields: `id`, `custom_type`, `content` (the injected text - a string; content blocks ride a later record version if an extension needs them), `display` (whether the interface shows it with distinct styling), optional `details` (extension metadata, never sent to the model). Writer: the host, like `custom` - on an extension's behalf, or on
its own for `settle-append` (gh #45: a settle handler's appended
entries, injected for the continued request). Readers: assembly injects it as a user message carrying `custom-type` and `display` in its extras; `display = false` hides it from the transcript, never from the model.

`context-edit` appends an omission or replacement of one earlier context-producing entry (gh #47: pi's `context_edit` semantics). Fields: `id`, `target_id` (a `user`, `assistant`, `tool-result`, or `custom-message` record), `replacement` (absent omits the target from future model context; present swaps its text, keeping role and tool linkage - an assistant replacement keeps its tool calls, a tool-result keeps its `call_id`). Writer: the interface's context-surgery surfaces, and `message_end`
hooks (gh #45) - a replacement lands here, never as a rewrite. Readers: assembly applies the latest edit per target; the target record itself stays unchanged in raw history, display, exports, and accounting. Omitting an assistant message whose tool calls have results leaves orphan `tool` messages a provider may reject - the surgery is explicit, so the log keeps what was asked.

## Ordering and identity

Records are append-only. Nothing is rewritten in place. Nothing is deleted.

Record identifiers are unique within a session and sortable by creation order. A `tool-result` references its `tool-call` by `call_id`, which is the identifier the model used, not the record identifier.

Every record except `session-start`, `fork-point`, and `branch-point` carries an optional `parent`: the previous record's id, stamped by the store on append when the writer left it empty. Records written before linkage have none, and old logs load unchanged. The ancestry walk starts at the log tip, follows `parent` links newest-first, jumps through `branch-point` records to their targets, and contributes everything before a parentless record in log order (pre-linkage history is linear by construction). Display reads, the transcript, and model context see the resulting chain only; audit reads, label resolution, gc reachability, and exports see the whole file (gc additionally honors compaction suppression, so compacted-away bytes are still collected).

The log order is the authority for conversation order. Timestamps are for display and diagnostics. A reader that sorts by timestamp is wrong, because two records written in the same millisecond have no defined timestamp order.

## Compaction

Compaction does not remove records. It appends a `compaction` record naming the range it replaces and carrying the summary.

Reading a session for display walks the log and skips any record inside a replaced range, using the summary in its place. Reading a session for audit walks everything.

This costs disk space and buys two things. A compaction that produced a bad summary can be inspected and, in a future version, undone. A user who wants to know what was actually said can find out.

Nested compaction is allowed. A second compaction may replace a range that includes an earlier `compaction` record.

## Fork

A fork creates a new session directory. Its `meta.json` names the parent and the record identifier. Its log starts with a `session-start` and a `fork-point`, then continues with new records.

The parent's records are not copied. A reader follows the fork chain backward to assemble full history. This makes a fork cheap and makes the parent immutable from the child's side.

Attachments follow the same rule. A forked session's records reference the content the home session wrote, so resolution walks the fork chain to find `attachments/<hash>`, and an export names the ancestor's path (`../<owner>/attachments/<hash>`) rather than copying the bytes. `lca session gc <id>` is the mark-and-sweep: it walks the session's whole fork tree, marks every hash any member's resolved record list references, and deletes the rest. Compaction only ever drops references, so an orphan is collected and a referenced attachment is never deleted; the tree-wide scope is what keeps a sibling branch's content safe when one member is swept.

A parent that is deleted leaves the child with a broken chain. The reader reports a truncated session and shows what it has, which is the same behavior as a corrupt record.

## Durability

Appends are written with a single write call per record where the record fits, then flushed. The file is opened in append mode, so concurrent appends from two processes do not interleave partially on the platforms LCA targets.

`meta.json` and `index.json` are written atomically: write to a temporary file in the same directory, then rename over the target. Neither is ever appended to.

A crash mid-write leaves a partial final line. The reader discards it. That is the whole recovery story for the common case, and it is why the framing is one record per line.

The agent does not call fsync on every record. A crash can lose the last few records. The alternative is a synchronous write per token, which is not a reasonable cost for the failure it prevents.

## Reading and error handling

A reader loads records until it hits one it cannot parse. It keeps everything before the failure, stops, and reports a truncated session. It does not attempt to skip the bad record and continue, because a corrupt record usually means corruption from that point on.

This matches the requirement already in the design: IF a session log contains a record that fails to parse, THEN the agent SHALL load the records before the failure and report a truncated session.

A record with an unknown `t` value is skipped rather than treated as corrupt. This lets a newer agent write record types an older one ignores, which is what makes format version 1 extensible without a version bump for every addition. A record with a known `t` but unknown fields is read with those fields ignored, so new data can join an existing record type without a version bump.

A record with a `v` higher than the reader understands is skipped with a warning.

## What never goes in the log

Credentials, tokens, and API keys. The credential store is separate and no record references it.

Environment variables. Tool results carry command output, and the agent redacts values matching known credential patterns before writing a `tool-result`.

The content of a file read outside the workspace, unless the user's action put it there. A path is recorded; the content goes to an attachment only when it is part of the conversation.

## Export

A session export produces a single file containing the metadata, the resolved record list with forks followed, and the attachments inline as base64 or as a sidecar directory.

Export applies the same redaction as the log, and additionally strips audit-only records unless an audit flag is passed, because an export is usually shared. Audit-only: `permission`, `extension-event` (who was allowed to do what), and the v2 vocabulary's `usage` (operational accounting) and `custom` (extension internals). Everything else survives a default export: the conversation (`user`, `assistant`, `tool-call`, `tool-result`, `custom-message`), its shaping (`compaction`, `context-edit`, `label`, `model-change`, `thinking-level-change`, `fork-point`, `session-info`, `session-start`, `session-end`). An export carries `context-edit` records as written - a consumer replays them latest-wins per target exactly like assembly, rather than receiving pre-edited text.

## Migration

A format version change is a change to the `v` field on new records. Old records keep their version and old readers keep working on the parts they understand.

A change that makes existing records unreadable needs a migration tool that rewrites session directories, run once, with the original preserved in a backup directory until the user removes it. This is a last resort. The first question for any format change is whether a new record type solves it, since unknown record types are already skipped.

The format version is recorded in `meta.json` and in the `session-start` record. Both are checked on load, and a mismatch between them is a corruption signal.

## Migration note (gh #37, #98 touchpoint - recorded, not built)

Collapsing fork directories into single-file trees would need: a
one-shot rewriter that splices each child's records into its parent's
log at the fork point (re-chaining `parent` links across the splice),
re-homing attachments into one directory, reworking gc's mark phase
from directory walks to reference walks, and rebuilding every
`index.json` from the merged files - with the originals preserved in
a backup directory until the user removes them, per the rule above.
Nothing about the current format blocks it (ids are unique per
session today and would need family-scoped uniqueness), and nothing
about current usage demands it: forks stay cheap and the entry tree
already navigates across them via `/resume`. Revisit only on the
ADR-0046 signal (a real fork-chain defect class).
