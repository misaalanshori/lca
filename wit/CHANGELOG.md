# `lca:ext` ABI changelog

Written for extension authors. Each entry names the version, the date, and
every change grouped as added, deprecated, removed, or fixed, with a
migration note for anything breaking (docs/abi-versioning.md).

##0.1.0 — unreleased

The initial surface, ahead of the Phase 8 freeze at 1.0.

### Added

- Worlds `tool`, `command`, and `hooks` with the hook points fixed at
  `pre-turn`, `pre-tool-use`, `post-tool-use`, `post-turn-end`,
  `attention-required`, `session-close`.
- The `types` interface: `tool-call`, `tool-result`, `message`, `usage`
  (with `cache-read`, `cache-write`, `cache-write-hour` — the WIT spelling
  of the JSON `cache_write_1h`, since a WIT identifier may not start a
  segment with a digit — and `cost`), and
  `session-record`. Every record carries a reserved `extras` list of
  key-value pairs; WIT has no map type, so the SRDD's "map of string
  pairs" is realized as `list<extra-pair>`.
- Host import `lca:host/log`, always granted, message truncation at the
  configured limit (FR-EXT-10).
- Pending (added by their phases before the freeze, per the punch list in
  docs/abi-versioning.md): worlds `provider`, `ui`, `compaction`,
  `context-transform`; host imports `fs`, `net`, `net-local`, `oauth`,
  `credentials`, `process`, `pty`, `completion`, `ui`.
