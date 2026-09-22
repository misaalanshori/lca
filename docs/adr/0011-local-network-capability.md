# ADR-0011: Local network access as a separate capability

Status: accepted.

Date: 2026-09-20.

## Context

A provider extension for a local model server, such as LM Studio or Ollama, needs to reach a server on the user's own machine or local network. The existing `net` capability cannot serve this case as written: it permits HTTPS only, on port 443, including on loopback, and local model servers typically serve plain HTTP on an arbitrary port, commonly 11434 or 1234.

The case is broader than loopback alone. A user running models on a desktop machine and using the agent from a laptop on the same network needs to reach an address like `192.168.0.47`, not just `127.0.0.1`. That is still a fundamentally different trust situation from reaching an arbitrary internet host, but it is not the narrow loopback-only case either.

Extending `net` itself to permit this was considered and rejected. HTTPS-only is a load-bearing invariant of that capability, not an incidental default, and carving out exceptions inside it makes the rule harder to state and harder to audit. The two cases also differ in more than the port: pattern matching for `net` is hostname-based, while a local-network grant is more naturally expressed as an address range.

A second problem surfaces once any address-based exemption exists: DNS rebinding. A hostname granted under `net`, matched and approved as a normal internet host, could resolve to a private or loopback address at connection time, either through misconfiguration or a deliberate attack, and reach a target the user never intended to expose to that extension.

## Decision

Add `net-local` as its own capability, separate from `net`.

```toml
[capabilities.net-local]
addresses = ["127.0.0.1", "192.168.0.0/16", "*.local"]
```

It grants HTTP or HTTPS, any port, to loopback addresses, to CIDR ranges within the private-use blocks defined by RFC 1918 and the link-local block, and to mDNS-style `.local` hostnames for the common case of discovering a machine on the network by name rather than requiring the user to know its address. The final range class, refined before implementation, also includes IPv6 unique-local and carrier-grade NAT blocks, `localhost`, and `::1`; the capability catalog holds the canonical list. The host validates every entry in the manifest against this class and refuses a manifest that tries to declare a public range under `net-local`; a provider wanting general internet access still declares `net`.

Alongside it, `net` gains an explicit defense: the host resolves every hostname it connects to and checks the resolved address, not just the hostname string, against the private-use and loopback ranges. A `net` grant whose hostname resolves to one of those ranges is refused and the attempt is recorded as a rebinding attempt specifically, distinct from an ordinary denial, because it can indicate DNS manipulation rather than a simple misconfiguration. An extension that legitimately needs a local address declares `net-local` for it; `net` never resolves there, regardless of what its hostname pattern matched.

Consent text for `net-local` names the situation plainly: "Connect to a device on your local network," distinct from `net`'s "Connect to api.example.com over the internet," because the two carry different risk and a person evaluating them should be told which one they are looking at.

## Alternatives considered

Extend `net` with a loopback-only exception, leaving broader private-network access unsupported. It would cover a single-machine setup and not the laptop-reaching-desktop case that motivated this record. Rejected as too narrow for the actual use case.

Extend `net` with a general "allow any address" flag per host pattern. It reintroduces the port and scheme exceptions inside the one capability whose simplicity is part of its value, and it does not solve the rebinding problem, since a flag on a hostname pattern says nothing about what that hostname resolves to at connection time.

No local-network capability at all, treating this as out of scope. It would leave LM Studio, Ollama, and any similar local-first provider unimplementable within the sandbox, forcing them to the native-linked path for no reason connected to what they actually need to do.

## Consequences

The `net` capability's resolved-address check needs a real implementation and a real test: a hostname that resolves to a private address must be caught even when the pattern that hostname matched looks entirely ordinary. This is exactly the kind of check the conformance extension and the fuzz targets should cover, since it is the sort of thing that is easy to get right in the common case and wrong at the boundary.

`net-local` extensions are meaningfully higher risk than pure loopback would have been, since a private network can include other people's devices, most obviously on shared or public networks with local routing enabled. The consent text says so, and the capability catalog's entry for it should be explicit that the grant reaches further than "this machine."

The manifest schema gains a new capability definition with its own address-pattern validation, distinct from `net`'s hostname pattern validation.

## Revisit conditions

Evidence that CIDR-range grants are too coarse in practice, for example a user wanting to permit one specific device on their network rather than the whole private range it sits in, which would argue for a narrower per-address grant mode rather than only ranges.
