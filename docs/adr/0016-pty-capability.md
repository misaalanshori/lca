# ADR-0016: A pty capability for interactive terminal sessions

Status: accepted.

Date: 2026-09-20.

## Context

The existing `process` capability spawns a command and returns a handle with stdin, stdout, and stderr streams, which covers a large share of what an extension wrapping an external tool needs, including long-running processes with streamed output. It does not cover a program that behaves differently when it is not attached to a real terminal: an interactive REPL, a full-screen terminal application, something in the shape of tmux. Those programs check whether they are attached to a pseudo-terminal and change their output accordingly, and some refuse to run usefully at all without one.

The question raised during design discussion was whether supporting this forces the native-linked, unsandboxed path, the way ADR-0007 anticipated some capabilities might.

## Decision

It does not. Add `pty` as its own capability rather than routing this case to the native-linked path or the out-of-process runner ADR-0007 declined to build.

```toml
[capabilities.pty]
reason = "Runs an interactive session inside a terminal panel."
```

It grants allocation of a pseudo-terminal: a program spawned under it sees a real terminal, with rows, columns, and raw mode available, and the extension receives resize events and can forward them. The actual PTY allocation happens host-side, behind the import, exactly like every other capability in the catalog; the extension's own code never leaves the sandbox to get it. Combined with the `ui` capability's panel region, this is enough to build something tmux-shaped as a genuine WASM extension: the panel renders the session's output, the extension relays keystrokes and resize events through `pty`, and the spawned program itself, not the extension, is the only thing that ever runs unsandboxed, in the same sense any command run through `process` already does.

The `reason` field is required, following the same pattern `process` already uses, since "allocate an interactive terminal" is not self-explanatory enough for a consent screen without knowing what it is for.

## Alternatives considered

Extend `process` to optionally return a PTY-backed handle instead of a plain pipe-backed one. It would avoid a new capability name. It also means `process`'s consent text and its risk profile would need to vary depending on a parameter the user cannot easily evaluate at a glance, since a PTY-backed process is genuinely a different kind of access, closer to handing over an interactive session than to running one bounded command. Keeping it as its own capability keeps the consent screen honest about what is actually being granted.

Treat this as forcing the native-linked path, on the reasoning that terminal emulation is inherently a systems-level concern. This was the premise the question was testing, and it does not hold up: the privileged operation is PTY allocation specifically, which is a boundable, describable interface like every other capability, not a case of needing arbitrary native code execution the way loading an unconstrained native plugin would.

## Consequences

This is the second concrete piece of evidence, after the local-network capability, for the pattern ADR-0007 anticipated: collect real gaps, add narrow capabilities deliberately, rather than reaching for a broader escape hatch. Both should be read together as validating that approach rather than as separate one-off additions.

The capability catalog, the manifest schema, and the conformance extension all need a `pty` entry, following the same treatment every other capability gets.

Rendering the PTY's output still goes through the widget tree from ADR-0003, not raw terminal bytes; a `pty`-holding extension can display what the interactive session produces inside its panel, but it does so by rendering into the declarative vocabulary like anything else with the `ui` capability, not by writing escape sequences directly.

## Revisit conditions

None specific to this record; it follows the general capability-catalog growth policy already established.
