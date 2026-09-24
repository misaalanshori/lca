# ADR-0025: Pin the checked address for `net` connections

Status: accepted.

Date: 2026-09-24.

## Context

The `net` capability refuses a hostname that resolves to a loopback or
private address, so a granted public hostname cannot be rebound to a local
target (FR-PERM-13, ADR-0011). The audit found the check was a time-of-check /
time-of-use hole: `net_request` resolved the hostname and checked the result,
then handed the URL to `hyper`, whose default connector resolved the hostname
*again* at connect time. An attacker who controls DNS for a granted host could
answer with a public address during the check and a local address during the
connect, and the refusal would never fire. `docs/threat-model.md` listed the
scenario as stopped; it was not.

Closing the hole means the connection must use exactly the addresses that were
checked, not a second resolution. `hyper-util`'s `HttpConnector` takes a
resolver as a `tower_service::Service<Name>`, and `R: Resolve` is a blanket impl
over that trait, so a custom resolver can pin the answer. `tower-service` is a
tiny, std-only trait crate already present transitively; it is not in the SRDD's
closed dependency list, so this record and the table row are its justification.

## Decision

Resolve once, check, and pin. `net_request` resolves the hostname, runs the
rebinding check, and records the checked addresses in the engine's pin map
(`Capabilities::pin`). The HTTP client is built with a `PinnedResolver` that
returns a pinned host's checked addresses verbatim and falls through to the
system resolver only for names that were never checked (IP literals and ad hoc
local names, which the connector short-circuits anyway).

`tower-service` is added as a direct dependency of `lca-tools` for the
`Service<Name>` bound the resolver must implement. It is std-only, has no
transitive tree of its own, and is already compiled as a hyper-util dependency.

## Alternatives considered

Re-resolve and connect by rewriting the URI to the checked IP. It works for
plain HTTP and breaks HTTPS: the TLS server name comes from the URI host, so a
certificate for the hostname fails against an IP, and disabling verification is
not an option.

A per-request TLS configuration with an overridden server name. Hand-rolling
rustls configuration to spoof SNI is more code in the security path than the
pinning itself, for the same effect.

Check the resolved address and accept the residual race, documenting it. It
leaves the threat model's scenario partially open and makes the documentation
lie in the other direction; pinning is small and removes the class.

## Consequences

`net`'s rebinding guarantee is now true as stated. The pin map is refreshed on
every request, so a legitimate DNS change is picked up on the next call; a
stale pin cannot outlive the request that set it.

The dependency table gains one row and `deny.toml`/`cargo-deny` sees one more
crate, already vendored in the lock.

A future move to a resolver with caching or Happy Eyeballs must preserve the
"connect to what was checked" property, not just the first answer.

## Revisit conditions

`hyper-util` growing a first-class way to pass a pre-resolved address per
request, which would make the custom resolver unnecessary. Evidence that pinning
breaks a legitimate provider whose DNS rotates addresses *within* a single
request, which the fallback (re-pin on the next request) should already cover.
