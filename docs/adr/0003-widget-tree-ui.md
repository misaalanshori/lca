# ADR-0003: Declarative widget tree for extension rendering

Status: accepted.

Date: 2026-09-20.

## Context

Extensions need to put things on screen. A status line segment showing a token budget, a panel listing open tasks, a modal asking a question. The question is what crosses the boundary between an extension and the terminal.

The direct answer is to let an extension write bytes to the terminal. It is the most flexible option and it is what a plugin in a terminal application usually gets.

It also hands an untrusted component the terminal control stream. Escape sequences can move the cursor anywhere, overwrite text that is already drawn, change the title, and hide content. An extension that draws a fake permission prompt is indistinguishable from a real one. Escape sequence injection is a known attack class, and the whole point of the capability model is that an extension gets only what it was granted.

There is a second problem with no security angle at all. Raw terminal access couples the ABI to whatever the renderer does today. Changing the layout engine would break every extension that drew around it.

## Decision

Extensions return a widget tree. The host lays it out and draws it.

The vocabulary for the first release is small: a text span with a semantic color role, a box with a border and an optional title, a row, a column, a spinner, a progress bar, and a key-value list. Semantic roles name intent rather than color, so a role like `warning` or `muted` renders correctly under any theme and degrades to plain text when the terminal has no color.

Text spans carry data, not control codes. The host escapes or strips any control character in a span before it draws. An extension that returns escape sequences sees them rendered as literal characters.

Regions are fixed: the status line, the footer, a side panel, and a modal. An extension registers for a region through the `ui` capability and returns a tree for it.

The widget variant reserves an extension point from the first release, the same way the provider event stream does. Adding cases to a WIT variant breaks the canonical ABI, so the case that carries a future widget kind has to exist before the ABI freezes.

## Alternatives considered

Raw escape sequence passthrough. Most flexible, and it gives up both the spoofing protection and the ability to change the renderer. Rejected.

A virtual terminal per extension, where the extension draws into a buffer that the host composites into a region. This contains the escape sequences inside a bounded area, so the spoofing problem shrinks to the region. It needs a terminal emulator inside the agent, which is a large component with its own parsing bugs, and it still couples extensions to cell-level layout.

No extension rendering at all. Extensions would return text and the host would decide how to show it. This is simple and it is close to what a first release needs. It also rules out the panel and modal cases, which are what makes a workflow extension feel like part of the agent rather than a command that prints.

## Consequences

Extension authors cannot draw anything the vocabulary does not cover. Some legitimate designs are not expressible. The vocabulary grows deliberately, one ABI version at a time.

The renderer can change freely. Switching the layout engine, adding a theme system, or changing how the status line packs segments breaks nothing.

The host controls the visual language, so extension output looks like the rest of the agent without any effort from the author.

A hostile extension cannot forge a permission prompt, because it cannot draw outside its region and cannot emit control codes at all.

## Revisit conditions

A pattern of extension authors working around the vocabulary with ugly constructions. Evidence that a specific high-value extension cannot be built. Either of these argues for growing the vocabulary rather than replacing the model.
