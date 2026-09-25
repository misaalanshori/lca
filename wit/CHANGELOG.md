# `lca:ext` ABI changelog

Written for extension authors. Each entry names the version, the date, and
every change grouped as added, deprecated, removed, or fixed, with a
migration note for anything breaking (docs/abi-versioning.md).

##0.2.0 — 2026-09-25

The first line of the ADR-0028 development window, and the window's first
**breaking** change. The host loads this line, the previous one (0.1), and
the 1.0 freeze line, so nothing installed stops loading.

### Changed

- `types.message.content` is now `list<content-block>` instead of a joined
  `string`, and the new `content-block` variant carries `text(string)` or
  `image(tuple<string, list<u8>>)`. A provider can carry a typed image
  instead of text only (ADR-0029). **Migration:** read each block in order;
  the `text` blocks join to the old string, and `image` blocks are new. A
  rebuilt component must declare `abi = "0.2"`.

### Added

- The `content-block` variant in the `types` interface.

##1.0.0 — 2026-09-23

The freeze release: the ABI is now stable under semver, no breaking
change ships without2.0, and the pre-freeze punch list is closed out in
this one line - `provider` gains the `login`/`logout`/`usage` identity
exports (ADR-0012), the `compaction` and `context-transform` worlds
join the surface (ADR-0015), the `completion`, `net-local`, and `pty`
capabilities join the catalog (ADR-0015/0011/0016), `usage` carries
`cost`, every record carries `extras`, the widget vocabulary gains
`image` with its reserved `vendor` case, and the hook points are fixed
at `pre-turn`, `pre-tool-use`, `post-tool-use`, `post-turn-end`,
`attention-required`, `session-close`.

A host at1.0 loads extensions built for0.1 unchanged: that line is
the freeze grandfather (docs/abi-versioning.md).

##0.1.0 — the pre-freeze line (superseded by1.0.0)

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
