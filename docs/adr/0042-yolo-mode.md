# ADR-0042: Yolo mode, and the read-only fatigue cut

Status: accepted.

Date: 2026-10-01.

## Context

The owner drove 0.5.2 hard enough to say the quiet part out loud: they want
hands-free runs. The permission layer is the product's spine - the model
cannot touch the shell or the filesystem without the user's word - and on a
long task that word is asked dozens of times, each prompt interrupting the
thing the user is actually doing. "Approve everything, loudly" is the
request; the design question is what that is allowed to mean.

Two facts shape the answer. The session log already records every
permission decision (`Record::Permission`: the action verbatim, the
decision, the pattern), so an automatic approval can be as auditable as a
human one. And the folder-trust work (ADR-0039) already established the
precedent that a *policy* answer beats a per-action prompt: a trusted folder
auto-approves in-workspace commands without asking, and the log shows the
trust, not thirty prompts.

## Decision

**A mode, not a bypass.** `permissions.mode` is `ask` (default) or `yolo`,
and `--yolo` sets it for one process through the flag layer, which wins over
any file. The mode lives on the grant store as session state - never in the
grant file, never in the session log as a mode change - because losing it
must only ever fail closed.

**Yolo answers prompts, it does not overrule words.** When a prompt would be
shown, yolo instead answers "always, for this exact pattern": the pattern is
persisted to the user grant store exactly as a human "always" answer
persists it, and the caller writes the same `Record::Permission` with
`decision: always`. Approve everything must never mean forget everything.
Two things yolo does *not* do: it does not override an explicit deny rule
(the user's own sentence outranks a mode that answers prompts, and a deny
rule never prompted in the first place), and it does not silence a proposal
review.

**Loud, every frame.** While the mode is on, the footer carries its own
line in the error role - `YOLO: every permission prompt auto-approved
(--yolo)` - and the transcript opens with one line naming the mode, what it
does, and that deny rules still deny. A mode that approves silently is the
thing this record exists to prevent.

**Read-only tools stop prompting.** `read`, `list`, `glob`, and `grep`
against a path outside the workspace no longer ask; the model needs to find
its bearings, and asking on every read is most of the fatigue. Nothing is
recorded for these: no user decision was made. A deny rule still refuses
them, and writes/edits outside the workspace still ask exactly as before.
This narrows FR-TOOL-3 to reads-with-consent-only for *writes*, and the
SRDD says so.

## Alternatives considered

**A per-tool toggle set** (`permissions.allow_read`, `permissions.allow_shell`,
…). More knobs, more states to explain, and the states people actually want
are two: "ask me" and "don't". A toggle per tool is a configuration screen
pretending to be a policy.

**Silent auto-approval** (yolo with no marker, or markers only in the log).
Rejected outright: the owner asked for hands-free, not blind. The footer
line and the startup banner are the price of the mode, and they cost
nothing when the mode is off.

**Making yolo persist in the config as a first-class default** (write
`permissions.mode = "yolo"` on the first `--yolo` run). Auto-writing a
security posture into a file the user did not edit is the kind of
convenience that turns into an incident report. The flag is per-process;
the file is the user's to edit.

**Yolo overriding deny rules too.** "Approve everything" taken literally.
Rejected: a deny rule is a deliberate, specific statement ("never run
`rm -rf *`"), not fatigue, and a mode whose whole justification is fatigue
should not be able to overrule it. A user who wants a rule gone deletes the
rule.

**Recording yolo as a single `mode` record instead of per-action
`permission` records.** Cheaper to write, and useless for audit: the point
of the trail is to answer "what did it do", not "what was it set to".

## Consequences

The interface and the capability engines share one grant store, so an
extension's `process`/`pty` command reaches the same answer: yolo applies to
every permission check in the process, not only the model's tools. That is
the intent - a hands-free run should not stall on an extension's command -
and it is worth stating, because it widens the blast radius of the mode.

Reads outside the workspace now reach the model without a prompt, which
means a prompt injection can ask for `/etc/passwd` or a private key and get
it. This is a real, accepted narrowing of FR-TOOL-3, named in
`docs/threat-model.md` with its residual risk; deny rules are the mechanism
a user has against a specific path, and `--no-read-auto` does not exist
because a second knob was judged worse than the honest default.

The permission matrix in testing-plan section 14 gains its three columns
(ask / read-only auto / yolo) with the session log as the evidence, and the
R10 matrix in the cycle's report carries the same rows.

## Revisit conditions

Evidence that users turn yolo on and leave it on, which would argue for the
mode being louder still (a per-turn reminder in the transcript) rather than
quieter. Evidence that the read cut leaks something a user cared about,
which would argue for narrowing it to in-project-plus-temp rather than
reinstating the prompt. A request for per-path read rules would be a rule
feature (already present), not a mode.
