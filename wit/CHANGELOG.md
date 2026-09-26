# Changelog

## 0.2.0 (in-place development line, 2026-09-26)

- **Breaking (in-place):** `provider-models.list-models` now takes
  `settings: list<extra-pair>`, the same opaque `setting: value` pairs
  `provider-completion` carries in its request `extras` (ADR-0035). A
  provider can now see its own persisted configuration when asked for
  models, instead of only the environment. A component built against the
  previous 0.2 shape fails to link and is refused at load.

Written for extension authors. Each entry names the version, the date, and
every change grouped as added, deprecated, removed, or fixed, with a
migration note for anything breaking (docs/abi-versioning.md).

##0.2 development — open (started 2026-09-25)

The ADR-0028 development window's single **in-place** line. Breaking changes
land here without a minor bump (ADR-0028's annotation): the package stays
`@0.2.0`, every manifest stays `abi = "0.2"`, and this section is the running
record rather than one entry per change. The host loads 0.2, the previous line
(0.1), and the 1.0 freeze line, so nothing installed stops loading. The 1.0
entry below is the freeze's; 1.0 is a snapshot-and-relabel of the final 0.2
with no interface change between them.

### Changed (breaking)

- The `provider` world gains the `provider-login` export: `login-options`
  and `login-submit`, the host-rendered picker over the extension's own
  presets (ADR-0033). A provider component must export it; the two
  first-party providers and the conformance extension are rebuilt in the
  same change. **Migration:** implement `login-options`/`login-submit`,
  returning `login-result::ok` with no options if the provider has none.
- `types.message.content` is now `list<content-block>` instead of a joined
  `string`, and the new `content-block` variant carries `text(string)` or
  `image(tuple<string, list<u8>>)`. A provider can carry a typed image
  instead of text only (ADR-0029). **Migration:** read each block in order;
  the `text` blocks join to the old string, and `image` blocks are new. A
  rebuilt component declares `abi = "0.2"`.

### Added

- The `resources` host import (`lca:host/resources`): `list-resources` and
  `read`, the extension's own read-only `resources/` data bag (ADR-0030).
  Always available (like `log`), package-scoped, own tree only, per-call size
  capped; the same seam serves both delivery modes (ADR-0032). Imported by
  the `tool`, `provider`, `compaction`, `context-transform`, and `ui` worlds.
  Additive: a new host import interface, so an extension built against the
  previous WIT keeps loading; a rebuilt component declares `abi = "0.2"`.
- The `state` host import (`lca:host/state`): `read`, `write`, `delete`, and
  `list-keys`, the extension's own mutable, non-secret data bag (ADR-0030).
  Always available, identity-namespaced, size-capped, wiped on uninstall;
  not secret-grade (secrets go in `credentials`). Imported by the same five
  worlds. Additive, same as `resources`.
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
