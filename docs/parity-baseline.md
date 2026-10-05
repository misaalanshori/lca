# Parity baseline receipt (P0-A / RM-003 / #105)

Cycle P0-A, recorded 2026-10-05. Resolves QA-019: the parity work starts
from a recorded green baseline.

## Pi reference pin

The phase reference tree exists exactly as the roadmap pins it:

- Roadmap pin: `v1.0.0-25-ga276dabe5` — **present**
  (`git -C ~/gits/pi describe --tags a276dabe5` returns exactly that;
  `a276dabe5` is 25 commits after `v1.0.0`, "fix(tui,coding-agent):
  convert non-PNG images for Kitty in Image").
- Tags present at record time: `v1.0.0`, `v1.0.1`, `v1.0.2`, `v1.0.3`.
- `~/gits/pi` HEAD freshly pulled at record time: `997d31f28`
  (pre-pull HEAD was `0b7287ce7`; the pull added `v1.0.3` and 21 commits
  past it). Per standing rule 1, pi's source at this tree is the
  implementation reference for parity cycles; the fixed pin above is the
  versioned anchor.

Match pi here / differ here, because…: the pin itself is a match (exact
roadmap reference, no substitution); using post-pin HEAD as the live
oracle is deliberate, because the roadmap's assumption 1 names the
checked-out 1.0.0-era tree as the oracle and marks post-tag conclusions
explicitly.

## Green baseline (clean worktree of `v0.5.4`)

Run on a clean detached worktree at tag `v0.5.4` (`537ec6c`),
never the working tree. Worktree removed after the run.

1. `cargo nextest run --workspace`
   → **1098 tests run: 1098 passed, 0 skipped, 0 failed** (263.9 s).
2. `cargo clippy --workspace --all-targets --all-features -- -D warnings`
   → **clean, exit 0** (~2 m 13 s).
3. `bash scripts/traceability.sh`
   → **all 171 requirements have at least one verifying test, exit 0**
   (FR-WEB-1/2/3 and NFR-11 deferred per
   `scripts/deferred-requirements.txt`).

Acceptance (#105): **green baseline, zero unexpected failures.**
No defect list. Parity edits start on top of this commit.
