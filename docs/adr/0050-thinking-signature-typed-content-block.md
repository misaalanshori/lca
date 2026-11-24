# 0050. A reasoning block crosses the boundary typed, with its replay signature

Status: accepted (2026-11-25).

## Context

Gemini's `generateContent` API requires a `thoughtSignature` echoed on replayed
`functionCall` parts; a call without it is HTTP 400 and the turn dies. The signature is
opaque bytes the vendor issued for one thinking run, and it has to survive the whole
round trip: the stream that produced it, the session record that persisted it, and the
next request that replays it.

Three seams carried that round trip before this record, all of them workarounds for the
frozen 0.6 ABI:

1. **Capture** travelled as a `thinking-signature` vendor event - an untyped kind string
   plus a JSON payload.
2. **`content-block` had no reasoning case**, so the block could not cross into a guest at
   all: `to_wit_request` dropped it, and the signature was written into the message's
   reserved `extras` map under `THINKING_SIGNATURE_EXTRA`.
3. **Every signature-needing guest rebuilt the `Reasoning` block from that key** by
   convention.

That worked (live-verified 2026-10-17: `gemini-3.8-flash` tool turns complete and
follow-ups answer), but the second and third seams are a spelling contract: the host
writes a string, the guest reads a string, and a typo on either side is a compile-time
no-op and a runtime loss. That is the class gh #581 has just retired for the truncation,
one hop away, and the 0.7 line exists so it can be retired here too.

The 0.6 line being frozen is what forced the seams, and the freeze is why the fix is a
line change rather than a patch: adding a case to a variant is a breaking change to the
canonical ABI.

## Decision

**`content-block` gains a `reasoning` case, carrying a `reasoning-block` record with the
text and the signature:**

```wit
record reasoning-block {
  text: string,
  signature: option<string>,
}

variant content-block {
  text(string),
  image(tuple<string, list<u8>>),
  reasoning(reasoning-block),
}
```

Four properties of that shape are deliberate:

- **`text` and `signature` travel together.** The pre-0.7 workaround echoed the signature
  only and dropped the thought text, which satisfies today's gateway (signatures are the
  MUST, summaries are optional) but cannot satisfy "return the entire response" if the
  gateway ever gets stricter. Carrying both now costs one field and makes full-fidelity
  replay possible without a second pipeline.
- **`signature` is `option<string>`, not `string`.** Absent and empty are different facts:
  a vendor that issued none must send **no field**, and a gateway rejects both casings of
  an empty one. An `option` makes that representable; a `string` invites `Some("")`.
- **A payload-free case would not do.** The truncation needed none (gh #581); a signature
  is the payload.
- **The wire placement is a sibling, not a nesting.** On the outgoing part the signature
  is `{"thoughtSignature": "<sig>", "functionCall": {...}}` - a sibling of `functionCall`.
  The Cloud Code gateway rejects it *inside* `functionCall` as an unknown field, in both
  casings, verified live against real traffic; the public docs describe nested placement
  as legal, so this is a gateway-specific bet, recorded as such below.

**The old channel stays as a compatibility write, for one minor version.** The host keeps
writing `THINKING_SIGNATURE_EXTRA` into the message's reserved extras map, because a guest
built against `@0.7.0` before this case reads the key and would otherwise lose the
signature - and a lost signature is a 400 on the next tool call, not a visible degradation.
Converted guests read the typed case and never the key, which
`tests/regressions/352-typed-reasoning-block.rs` proves by scanning every guest source:
a guest cannot read a key it does not name. The write goes away after the first minor
release that follows the case, when no in-tree guest names the key.

**The case lands on the 0.7 line, not in a 0.6 patch.** A variant case is breaking; the
line was opened by gh #407 and its second change was gh #581, so this is the third. The
support window is unchanged in shape - a 0.6 manifest still loads, and the package rename
still means an old binary is refused at link with an actionable sentence - and every
first-party guest that maps a content block gains an exhaustive-match arm, which is the
compiler enforcing the change rather than a convention remembering it.

## Alternatives considered

**A fourth vendor event** (`reasoning-block` as a kind plus JSON). It needs no ABI change,
and it is the same escape hatch the truncation used. Rejected because it is the same
stringly-typed round trip the record exists to remove: the payload would be hand-parsed on
both sides, and a misspelled field would be a silent no-op. Gh #581 rejected the identical
proposal for the truncation in the same line.

**Keep the extras key only** - the status quo. No interface change, no rebuild, and it
works. Rejected because the failure mode is the one this record exists for: a typo in
either side's spelling compiles and then costs a live turn, and the price of the typed path
is one arm in each mapper with the compiler naming every one of them.

**Nest the signature inside `functionCall`.** The public docs suggest it and it would keep
the part shape unchanged. Rejected on measurement: the gateway answers HTTP 400 for an
unknown field, in both `thoughtSignature` and `thought_signature` casings, verified live.
The sibling placement is what the stream delivers and what the gateway accepts.

**Carry only the signature** (no `text`), matching what the workaround recycled. Rejected
because it would make "return the entire response" unrepresentable, and because the text is
already in the assembled block - dropping it at the boundary would be a loss with no saving.

## Consequences

Every guest that maps `content-block` gains the case: the subscription kit (so `codex` and
`grok`), `anthropic`, `antigravity`, `github-copilot`, `kimi-coding`, `llama`, `meta`,
`openai-compatible`, `openrouter`, and the `conformance`, `skills`, `project-memory` and
`rules-compat` guests - fourteen conversion sites, all in the same change as the WIT edit,
which is why a fresh `wit/` and a stale guest cannot drift apart in the tree. The
`context-transform` world shares the variant, so a transform can now see (and return) a
reasoning block with its signature instead of a `text` block that lost it; that is a
capability, not a requirement - a transform that ignores the case changes nothing.

The compat write is the one piece of the old path that remains, with its removal condition
named at the write site and its "no guest names it" half machine-checked. Two smaller
consequences: the redacted shape (text empty, signature present) is expressible and pinned
by a row, because it is what the old reconstruction produced and it has to keep working;
and `docs/extension-authoring.md` now tells a provider author to read the case rather than
a key.

## Revisit conditions

- **The gateway flips on placement.** The sibling rule is a measured bet, not a documented
  one. A 400 naming the signature on a part that carries it is the symptom; the fix is one
  line in `build_request` plus its row, with capture, persistence and the bridge untouched.
- **A stricter gateway demands the thought text too.** Then the `text` half already
  carried here stops being optional-for-later and becomes the required shape, with no
  further interface change.
- **Per-call signatures.** One run carries one signature today (flat-latest, gh #41's
  model), which is sufficient for single-call turns and verified live. A turn with several
  signed calls would need the signature attached per `tool-call` rather than per message;
  the typed case makes that a `reasoning-block` beside each call instead of a redesign.
