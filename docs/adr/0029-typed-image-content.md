# ADR-0029: Typed image content for provider messages

Status: accepted.

Date: 2026-09-25.

## Context

A coding agent reads screenshots, diagrams, and design mockups as often as it
reads text. LCA's provider message ABI carried `content: string` only, so an
image had nowhere to travel except a side channel.

Under the 1.0 freeze the choice was bad either way. The `extras` map on every
ABI-crossing record exists for *non-structural* data; smuggling image bytes
through it would make a structural feature depend on an untyped string map
with no schema and no validation. The alternative was a 2.0 that also carried
everything else the interface had learned it needed. ADR-0028 reopened the
interface into a development window precisely so a structural feature could be
designed as one, and named the typed image as the window's first decision.

## Decision

`message.content` becomes `list<content-block>`, and the new `content-block`
variant carries `text(string)` or `image(tuple<string, list<u8>>)`. The
protocol type is `ContentBlock::Image { media_type, bytes }`.

The bytes are the raw image; the media type is sniffed from magic bytes, never
taken from a user-controlled file name (the attach path's D8 rule). Assembly
turns a `user` record's image attachment hash into an `Image` block after the
message's text, so a provider that carries vision receives the image and a
provider that does not still reads the `[image attachment …]` stub the attach
path put in the text. `openai-compatible` maps the block to a base64
`image_url` data URI and `antigravity` to an `inlineData` part.

This is a breaking change, made as the ADR-0028 window's first breaking change on the 0.2 line: the WIT
packages, every first-party manifest, and `ABI_VERSION` move to `0.2`, the
conformance extension is updated in the same change, and the support window
keeps the previous line loading.

## Alternatives considered

- **Carry images in `extras`.** Withdrawn. `extras` is reserved for
  non-structural data (docs/abi-versioning.md's record-growth rule); a
  structural feature there has no schema, no validation, and no way to be
  removed. Rejected.
- **Text stub only, no bytes.** A model that cannot see the image cannot
  answer "what is wrong with this screenshot", which is the use case. The stub
  remains as the fallback for a provider without vision, not as the feature.
- **Wait for a 2.0.** Puts the feature behind an unspecified future release
  for a project with no published third-party ecosystem; ADR-0028 exists to
  avoid exactly this. Rejected.

## Consequences

Every world that carries a message (provider, compaction, context-transform)
sees the new content shape, and a component built against 0.1 or 1.0 keeps
loading during the window. The native twin and the WASM component are diffed
byte for byte, so the two delivery modes cannot drift. The terminal keeps
rendering the image as a labelled placeholder (D7); the data path is typed
now, the render protocol decision is deferred. `wit/CHANGELOG.md` carries the
migration note.

## Revisit conditions

If a second content kind earns its place (audio, a file reference), the same
variant grows a case — which is breaking, so it belongs to the same window. If
the terminal render protocol is chosen, it changes the `ui` world's `image`
widget, not this message shape.
