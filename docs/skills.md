# Skills

A skill is a directory with a `SKILL.md` in it: a short header naming the skill and
saying when it applies, then the instructions. LCA reads them from a project's
`.lca/skills/<name>/SKILL.md`, from `~/.lca/skills/<name>/SKILL.md`, and from an
extension's `resources/skills/<name>/SKILL.md` bag. The directory name is the default
skill name.

Loading a skill puts its **body** in front of the model, nothing else: not the header,
not the file name. Whatever you write below the header is what the model receives, so
write the whole instruction there.

## The header

Four fields, all optional, all `key: value`:

| Field | Meaning |
|---|---|
| `name` | The skill's name. Defaults to the directory name. |
| `description` | One line shown in the skill catalog. |
| `match` | Comma-separated trigger words. With `skills.inject_matched` on, a skill whose trigger word appears in your message is injected without being asked for. |
| `disable-model-invocation` | `true` keeps the skill out of the catalog and away from matched injection: only `/skill:<name>` loads it. |

Everything else in the file is body.

## Two ways to write the header

Both do the same thing; pick whichever reads better for your file.

**Fenced.** Open with a line of three dashes and close with one. Everything between
the dashes is the header, so a line inside it never leaks into the instructions even
if it is prose:

```markdown
---
name: deploy
description: Ship the current branch
match: ship, deploy
---
Run `just deploy`. If it fails, do not retry.
```

**Unfenced.** Write the fields at the top with no dashes. The header ends at the first
line that is not one of the four fields above — a blank line, or the first line of your
instructions:

```markdown
name: deploy
description: Ship the current branch
Run `just deploy`. If it fails, do not retry.
```

An unfenced file may also close its header with a single `---`, the older shape:

```markdown
name: deploy
---
Run `just deploy`.
```

### What ends an unfenced header

The four field names above, and nothing else, are header syntax. The header ends at:

- a **blank line**;
- a `---` line;
- **any other line** — including one that contains a colon. `Note: this repository
  needs Rust 1.85.` is the first line of your instructions, not a header field, so it
  is shown to the model.

That last rule is deliberate, and it is the one to remember: an unrecognised line is
**instructions**, not discarded header. Dropping something you wrote would be
invisible to you; showing it to the model is something you can see in the skill's
text and fix.

So an unfenced header cannot carry a field LCA does not know. If you want to write a
line that merely *looks* like a field — `Status: draft` — fence the header, or move the
line below a blank line. Inside a fence, an unrecognised line is header chrome and is
skipped.

A field's value may contain colons: `description: Use when: the build is red` keeps
the whole sentence after the first colon.

## Invoking a skill

- `/skill:<name>` loads it explicitly. That always works, including for a skill with
  `disable-model-invocation: true`.
- The `skill` tool lets the model load one by name. Restricted skills are not
  offered to it.
- With `skills.inject_matched` on, a skill whose `match` word appears in your message
  is injected automatically. Restricted skills never are.

See [Configuration](configuration.md) for `skills.inject_matched`.
