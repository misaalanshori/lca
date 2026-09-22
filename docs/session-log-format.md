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

`attachments/` holds content too large for the log, stored by content hash. Images, large tool outputs, and pasted files go here. A record references an attachment by hash.

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

`user` holds one user message. Fields: `id`, `content`, and optional `attachments` as a list of hashes.

`assistant` holds one model message. Fields: `id`, `content`, optional `reasoning`, `model`, `provider`, and `usage` with input tokens, output tokens, and cost.

`tool-call` holds one call the model requested. Fields: `id`, `call_id`, `name`, `arguments` as a JSON string, and `source` naming whether the tool is built in or comes from an extension.

`tool-result` holds the outcome. Fields: `id`, `call_id` matching the call, `status` of ok, error, denied, or timeout, `content` or an attachment hash, and `truncated` as a boolean.

`permission` records a grant decision made during the session. Fields: `action`, `decision` of once, always, or denied, and `pattern` when the decision was always. This is a record of what happened, not the grant store itself.

`extension-event` records a load, a disable, a trap, or a capability denial. Fields: `extension`, `event`, and `detail`.

`compaction` marks a compaction. Fields: `replaced_from` and `replaced_to` as record identifiers, `summary` as the replacement content, and `strategy` naming the built-in compactor or the extension that ran.

`fork-point` appears in a forked session and names the parent session and the record identifier the fork was taken at.

`session-end` is written on a clean exit. Its absence means the session ended without one, which is normal after a crash and is not an error.

## Ordering and identity

Records are append-only. Nothing is rewritten in place. Nothing is deleted.

Record identifiers are unique within a session and sortable by creation order. A `tool-result` references its `tool-call` by `call_id`, which is the identifier the model used, not the record identifier.

The log order is the authority for conversation order. Timestamps are for display and diagnostics. A reader that sorts by timestamp is wrong, because two records written in the same millisecond have no defined timestamp order.

## Compaction

Compaction does not remove records. It appends a `compaction` record naming the range it replaces and carrying the summary.

Reading a session for display walks the log and skips any record inside a replaced range, using the summary in its place. Reading a session for audit walks everything.

This costs disk space and buys two things. A compaction that produced a bad summary can be inspected and, in a future version, undone. A user who wants to know what was actually said can find out.

Nested compaction is allowed. A second compaction may replace a range that includes an earlier `compaction` record.

## Fork

A fork creates a new session directory. Its `meta.json` names the parent and the record identifier. Its log starts with a `session-start` and a `fork-point`, then continues with new records.

The parent's records are not copied. A reader follows the fork chain backward to assemble full history. This makes a fork cheap and makes the parent immutable from the child's side.

A parent that is deleted leaves the child with a broken chain. The reader reports a truncated session and shows what it has, which is the same behavior as a corrupt record.

## Durability

Appends are written with a single write call per record where the record fits, then flushed. The file is opened in append mode, so concurrent appends from two processes do not interleave partially on the platforms LCA targets.

`meta.json` and `index.json` are written atomically: write to a temporary file in the same directory, then rename over the target. Neither is ever appended to.

A crash mid-write leaves a partial final line. The reader discards it. That is the whole recovery story for the common case, and it is why the framing is one record per line.

The agent does not call fsync on every record. A crash can lose the last few records. The alternative is a synchronous write per token, which is not a reasonable cost for the failure it prevents.

## Reading and error handling

A reader loads records until it hits one it cannot parse. It keeps everything before the failure, stops, and reports a truncated session. It does not attempt to skip the bad record and continue, because a corrupt record usually means corruption from that point on.

This matches the requirement already in the design: IF a session log contains a record that fails to parse, THEN the agent SHALL load the records before the failure and report a truncated session.

A record with an unknown `t` value is skipped rather than treated as corrupt. This lets a newer agent write record types an older one ignores, which is what makes format version 1 extensible without a version bump for every addition.

A record with a `v` higher than the reader understands is skipped with a warning.

## What never goes in the log

Credentials, tokens, and API keys. The credential store is separate and no record references it.

Environment variables. Tool results carry command output, and the agent redacts values matching known credential patterns before writing a `tool-result`.

The content of a file read outside the workspace, unless the user's action put it there. A path is recorded; the content goes to an attachment only when it is part of the conversation.

## Export

A session export produces a single file containing the metadata, the resolved record list with forks followed, and the attachments inline as base64 or as a sidecar directory.

Export applies the same redaction as the log, and additionally strips `permission` and `extension-event` records unless an audit flag is passed, because an export is usually shared.

## Migration

A format version change is a change to the `v` field on new records. Old records keep their version and old readers keep working on the parts they understand.

A change that makes existing records unreadable needs a migration tool that rewrites session directories, run once, with the original preserved in a backup directory until the user removes it. This is a last resort. The first question for any format change is whether a new record type solves it, since unknown record types are already skipped.

The format version is recorded in `meta.json` and in the `session-start` record. Both are checked on load, and a mismatch between them is a corruption signal.
