# ADR-0008: Build-time composition for extension dependencies

Status: accepted. One consequence is revised by ADR-0015: `completion` joins the 1.0 capability set.

Date: 2026-09-20.

## Context

Extension authors will ask how one extension depends on another. The word dependency covers three different needs, and they have three different answers.

Code reuse: an author wants a shared library for vendor authentication across three provider extensions.

Service access: a workflow extension wants to ask the active provider for a completion.

Version coupling: one extension refuses to load without another at a given version.

Treating these as one problem leads to a package resolver inside the agent, which is a large component with a long tail of failure modes.

## Decision

Code reuse happens at build time through Component Model composition, or through ordinary vendoring. The author composes dependencies into one component before publishing. What arrives at the host is one artifact with one manifest and one capability set.

Service access happens through a host capability. An extension that wants a completion asks the host, and the host routes to whatever provider is active. An extension never calls another extension's instance.

Version coupling is not supported. An extension that cannot function without another is one half of a composed component and should be published that way.

The host gains nothing for any of this. That is the decision's main property.

## Alternatives considered

A host-side resolver that reads dependency declarations from the manifest, fetches the dependencies, and links them at load time. It gives the familiar package manager experience. It brings version solving, a lockfile per extension, diamond conflicts where two extensions need different versions of the same dependency, and a capability question with no clean answer: when A pulls in B, whose manifest declares B's grants, and what does the consent screen show. Rejected.

Direct calls between extension instances, with the host acting as a registry of live instances. It solves service access without a host capability for every service. It turns the extension graph from a star into a general graph, which needs load ordering, cycle detection, and a defined behavior when one node is disabled mid-session. It also lets one extension's capability set reach another's through a call chain, which breaks the model where grants attach to an instance.

Vendoring only, with no composition support. It works and it duplicates code across artifacts. Composition is strictly better and costs the host nothing, so there is no reason to forbid it.

## Consequences

The extension graph stays a star with the host at the center. Load order does not matter. Disabling an extension cannot break another one.

Shared code is duplicated across artifacts when authors vendor rather than compose. This costs bytes on disk and nothing else.

Service access capabilities need designing as the cases appear. The completion capability is the obvious first one and it is not in the 1.0 set, because no 1.0 extension needs it yet. (Superseded on this point by ADR-0015, where the default compaction strategy became the consumer that justifies it.)

The extension authoring guide needs a section showing the composition workflow. Authors will not discover it on their own. This is a Phase 8 documentation task.

Deferring the resolver past 1.0 is not a temporary measure. Reversing this decision means accepting version solving and the capability question, and those costs do not shrink with time.

## Revisit conditions

An ecosystem large enough that duplicated shared code becomes a maintenance problem across many extensions. Evidence that composition tooling is too hard for typical authors, which would argue for better tooling rather than a host resolver.
