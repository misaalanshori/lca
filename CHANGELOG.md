# Changelog

Notable changes to LCA. Versions follow semantic versioning for the product;
the `lca:ext` ABI version is independent and is printed by `lca --version`.
Dates are UTC.

## [0.3.0] - 2026-09-26

The finish-it release: no new surface. Every feature the last two cycles
added got its face, and what driving found in the way got fixed.

### Added

- **The `/login` picker.** `/login` with no argument lists every enabled
  provider extension's login options - the presets, and always the host's
  own "Custom endpoint..." - in a scrolling list. Choosing a row runs that
  option's field list, one prompt at a time. A key is masked and never
  renders; a base URL or a model id shows as typed (ADR-0033).
- **`/login <provider>`** scopes the picker; **`/login <option-id>`** skips
  it, so the same journey is drivable from a script.
- **Named custom endpoints.** `<config>/provider-presets.toml` merges the
  user's own presets with the extension's (D1's override layer).
- **`GET /models` discovery.** After a login the provider extension queries
  the endpoint's model list; `/model` offers what comes back, and the
  preset's curated short list is the fallback when it cannot be reached
  (D2).

### Fixed

- **The picker had no presets at all.** The bundled native extension's
  `resources` source was never set, so `resource_read` found nothing and
  the 19 presets cycle 4 shipped were dead weight.
- **A picker with many choices truncated its last rows**, making
  "Custom endpoint..." unreachable. It now scrolls with the cursor in view
  and shows `[n/total]`.
- **A bare release build produced no `lca` binary** again: the workspace
  `default-members` fix did not reach every build line. Regression 18.
- **A forked session listed as `0 messages`**; a fork now counts its
  resolved history. Regression 16.
- **A compaction summary read as an untrusted user note.** It now carries
  the agent's own self-describing framing, so a resumed model reads its own
  memory. Regression 17.
- **The nightly fuzz workspace did not build.** Two fuzz targets had
  drifted from the API they fuzz - one would have false-crashed on any
  valid data-only package. All four build again, and a `cargo check` of the
  fuzz workspace now runs in CI so a parser change cannot break the nightly
  silently. Regression 19.
- The unknown-provider message names `lca ext enable` alongside
  `lca ext install`, so the state where the only provider is disabled is
  escapable. Regression 20.
- Exhausted retries say so; a burst of pasted input redraws once.

### Changed

- `/login`'s secret prompt is per-field: only a key is masked.

## [0.2.0] - 2026-09-26

The dogfood release: LCA was driven like a real developer for a full cycle,
and everything found in the way was fixed. Cycle 2's features, which had not
been released, are in here too.

### Added

- **Typed image content.** A message's content is a list of blocks
  (`text` or `image`); `/attach <path>` in the interface and `--attach <path>`
  headless stage an image, stored content-addressed and owner-only, and a
  vision-capable provider receives the bytes (ADR-0029).
- **`lca session gc <id>`**: delete attachments in a session's fork tree that
  no resolved record references.
- **`lca ext enable <name>` / `lca ext disable <name>`**: per-project
  enablement, which the loader already read but no command reached
  (FR-PROV-9).
- **`lca:ext` ABI 0.2.** The window is a single in-place development line;
  the re-freeze is a snapshot-and-relabel to 1.0 (ADR-0028 and its
  annotation).
- **A scheduled live-provider smoke** workflow, the only test that exercises
  an extension's HTTPS path against a real endpoint.

### Changed

- Cancellation reaches every blocking host wait: the OAuth callback and,
  new in this release, `net` requests and streaming body reads. A hung
  request returns within the NFR-21 window instead of waiting out its
  timeout.
- The interface renders a reasoning model's reasoning marked `∴` and set off
  from the answer, and names the tool and its argument in a finished tool
  call instead of the provider's opaque call id.
- A large paste arrives as one bracketed-paste event instead of one key event
  per character.
- The interface says it needs a terminal when run without one, instead of the
  opaque `os error 6`.

### Fixed

- **Extension `https` requests worked again.** The pinned-DNS connector left
  `enforce_http` set, so every `https` `net` request died before TLS with
  "invalid URL, scheme is not http".
- **Capability grants are keyed by the project, not the data directory.**
  A grant attached mid-session was invisible to the engine that had to honor
  it, and shell "allow always" patterns applied to every project.
- **Re-compaction no longer drops the previous summary**, so facts distilled
  early in a long session survive.
- **A corrupt session says so**: the interface kept the reader's truncation
  warning instead of showing a short transcript silently.
- **`lca resume` lists current message counts**, not a snapshot from session
  creation.
- **A non-SSE provider response is an error**, not a silent empty answer.
- A plain conversation no longer logs a false cache-boundary divergence every
  turn.
- `grep` accepts a file path (it failed with "Not a directory").
- The Windows credential file gets an explicit owner-only DACL.

## [0.1.3] - 2026-09-25

Runtime and UX fixes from hands-on testing.

### Fixed

- Extension manifests that declared the `command` world without exporting it
  made the host refuse the installed artifact; both providers ship no slash
  commands, so the world is gone and the components load.
- The interactive TUI could leave the terminal in raw mode on a panic; a
  restore guard plus panic hook always restore, and `ext install` answers
  with a single key.

### Added

- A real input cursor (Left/Right/Home/End/Delete), `/help`, `/exit`, and Tab
  completion that lists multiple matches.
- A Pi-inspired layout: no boxes, a flowing transcript, a single separator,
  a dim status line.

## [0.1.2] - 2026-09-25

- Fixes `lca --version` and the `session-start` record, which reported ABI
  0.1 from a stale constant; the ABI is sourced from the contract crate so it
  cannot drift.
- Publish assets are named from the manifest ABI line.

## [0.1.1] - 2026-09-25

- Post-audit patch: fixes the critical, high, medium, and low findings from a
  full code and test review, extends conformance coverage, and hardens
  traceability.

## [0.1.0] - 2026-09-25

- Phase 5 exit: reference extensions published as OCI artifacts
  (`ghcr.io/misaalanshori/lca/*`) and as a plain HTTPS zip, both installable
  with `lca ext install` (ADR-0010).
