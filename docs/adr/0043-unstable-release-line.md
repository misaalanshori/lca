# ADR-0043: The unstable release line

Status: accepted.

Date: 2026-10-02.

## Context

The project ships one line today: a tagged release, six binaries, a
checksum file, a provenance attestation, and an installer whose whole
version story is "the latest release, or the one you pinned". That line
is deliberate and stays untouched. What it cannot answer is the question
the owner actually asked every day of development: *can I install the
build from this commit, right now, through the same one-liner, without a
human cutting a release?*

The options were per-commit releases, a nightly schedule, resolving
"latest unstable" through the GitHub API at install time, and a rolling
pre-release. Each was weighed against two rules the installer cycle
already set: the installers parse zero JSON (a redirect GitHub already
serves is the whole resolution mechanism), and a release is a deliberate
act with a changelog entry, an approval, and a reproducibility receipt.

## Decision

**One rolling pre-release, replaced per green commit.** A GitHub release
tagged `unstable`, marked `prerelease`. The `unstable` workflow runs
after the `ci` workflow completes successfully on `main` (`workflow_run`,
so the test pipeline's own signal gates publication and no test is run a
second time), builds the same six targets `publish.yml` builds with the
same toolchain pins, produces `artifacts.sha256` the same way, attests
provenance with the same action, and replaces the release's assets with
`gh release upload --clobber` (creating the release once, idempotently,
with `--prerelease --latest=false`).

**The `unstable` tag is the documented exception to "a tag identifies the
exact release".** It names a *channel*, not an announcement: the tag is
created once and never moved, but what rolls is the release it heads —
its assets are replaced after every green commit, so the tag's target
SHA is explicitly *not* the provenance of the binaries currently behind
it. Provenance for those binaries is the attestation plus the version
they carry. `releases/latest` never resolves to it (`prerelease`, and
`--latest=false`), which is what keeps the stable installers' redirect
path true. `docs/release-policy.md` carries this as a dated annotation on
its tag rule rather than a rewrite of the rule.

**Version baking: `X.Y.Z.b<sha7>`.** `crates/lca-cli/build.rs` emits
`PRODUCT_VERSION` from `LCA_BUILD_VERSION` when it is set and from
`CARGO_PKG_VERSION` otherwise; `lca --version`'s first line is that
product version, and the other three lines (`abi`, `crate`, `target`)
keep their shapes — an unstable build still says `crate 0.5.2`, because
the crate version is a different number that means a different thing. The
workflow sets `LCA_BUILD_VERSION=<workspace version>.b<first seven sha
digits>`, so a stable build is byte-identical to what it was before this
ADR and every existing version assertion still passes. The installed
`lca <old> -> <new>` report carries whichever strings the two binaries
printed, hash forms included.

**What unstable guarantees, and what it does not.** Built from a commit
that passed the full pipeline on all three platforms; checksummed from
the same directory as the binary; provenance-attested exactly as a stable
artifact is. It may break — that is the point of the line. It gets no
changelog entry (the history lives in git and in the CI run artifacts,
90-day retention, per build) and no support promise beyond "reinstall
without the flag".

## Alternatives considered

**A pre-release per commit.** Rejected: ~120 MB of assets per commit
forever (six binaries plus checksums), a release page that becomes a
scrollbar, and installers that would have to query the API for "the
newest pre-release" — JSON parsing in the install path, which is the
rule the installer cycle already wrote down as forbidden.

**A nightly schedule.** Rejected: an installer line that is up to a day
behind `main` is a lie with a timestamp, and the schedule would need its
own failure story when a night's build is red.

**Resolving "latest unstable" through the API at install time.** Rejected
for the same reason as per-commit releases: the redirect-only resolution
is what keeps `install.sh` a script a user can read in one sitting.

**Moving the `unstable` tag itself to each new commit.** Rejected: it
would make the tag mean "whatever was last green" in the git graph while
the assets behind it are *also* replaced, two moving parts where one
suffices, and it would break anyone who pinned a SHA by that tag. The tag
stays put; the release's assets are the channel.

## Consequences

- The rolling release's asset names are the stable asset names, so both
  installers share every line of code outside the one URL branch.
- The tag's target SHA lags the assets; the baked `X.Y.Z.b<sha7>` and the
  attestation are the provenance readers should use, and the release
  notes say so.
- Gate 10 (both installer suites) now covers the `--unstable`/
  `-Unstable` path through `FR-INSTALL-10`, including the usage error
  for `--unstable` with `--version`.
- Build history for the line is CI run artifacts (90-day retention) plus
  git; there is deliberately no release-per-commit record.

## Addendum 2026-10-13 (gh #251): the unstable extension channel

The rolling line above covers the native binary only; extensions
shipped separately on manual dispatches, so an unstable-binary user
could not install the matching extension build. The `unstable`
workflow now carries a parallel `extensions` job on every green
main commit: it builds the curated first-party set (the same eight
`publish-extensions.yml` ships — bundled providers and the skills
zip already ride the binary line) for `wasm32-wasip2` and publishes
each to ghcr.io under two tags.

- `:unstable` is the rolling channel tag, clobbered in place — the
same channel-pointer semantics as the binary line: the tag names
the channel, never a pin.
- `:b<sha7>` is the immutable commit pin, for reproducible bug
reports and bisecting.
- The published manifest's `version` is stamped
`<base>-unstable.b<sha7>` at publish time (a staged copy, never a
rewritten repo file), so `lca ext list` and `lca ext info` display
exact commit provenance.

The job runs parallel to the binary matrix with no `needs` edge,
so it cannot delay binary publishing; stable tags are untouched
(the manual release flow still owns `abi-*` and version tags).

## Addendum 2026-10-13 (gh #257): unstable prune and stable `:latest`

A rolling channel leaves a superseded manifest per commit behind,
so the `extensions` job ends with a prune step
(`scripts/prune-unstable-ghcr.sh`, `--execute --keep 2`): the live
`:unstable` carrier plus one rollback spare stay, older
`:unstable` carriers, stale `:b<sha7>` pins, and untagged orphans
go. The unit of safety is the tag-name guard, checked first and
unconditionally — a version carrying any semver (`X.Y.Z...`),
`abi-*`, or `latest` tag is never a deletion candidate, whatever
else it carries. The script defaults to `--dry-run`, and
`--execute` refuses unless `GITHUB_ACTIONS=true`, so a dev
machine can list but never prune; its `selftest` (fake version
lists in, only eligible ids out) runs as a CI gate. Prune
failures warn, never fail the publish.

Stable releases publish the canonical `:latest` tag alongside
`abi-*` and the version tag (both `publish.yml` and
`publish-extensions.yml`), so plain OCI clients resolving the
default tag stop 404ing. `:latest` is itself a protected tag
under the prune guard, and the unstable job never pushes it.

## Addendum 2026-10-14 (gh #258): debug-trace in unstable binaries

The full-stack observability crate (`crates/lca-debug`, gh #258) is
compiled into the rolling unstable release binaries via
`--features debug-trace`. This enables `--debug`, `--debug-wire`,
and `LCA_LOG` domain tracing at runtime on the unstable line,
allowing developers and testers to inspect wire traffic and WASM
boundaries without building from source. Stable releases continue
to omit the feature, preserving the 25.0 MB size ceiling (NFR-1)
and zero runtime overhead.

