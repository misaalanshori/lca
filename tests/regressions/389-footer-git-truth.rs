//! GitHub #389 (closes #330): the footer tells the truth about git, and stops
//! repainting I/O.
//!
//! Five gaps in one file, and they fail for five different reasons, so this
//! guard pins the *mechanism* of each from outside the crate:
//!
//! 1. **Discovery** — the old reader looked at `<cwd>/.git/HEAD` only, so a
//!    subdirectory, a linked worktree, and a detached HEAD all lost the branch,
//!    and a `reftable` repository printed the placeholder `.invalid`. Walking
//!    up, following the `gitdir:` indirection, and reading a bare object id as
//!    a revision are the three structural fixes; `.invalid` is resolved
//!    through `git --no-optional-locks` because the name lives in a binary ref
//!    table.
//! 2. **No I/O on the frame path** — `render` runs on every streaming delta, so
//!    the branch is a cached field and the refresh rides the tick's cadence.
//!    The measured bound lives beside the code (see the footer's `cache_tests`
//!    rows: 1000 frames, zero reads after the prime).
//! 3. **Zero-noise** — `↑0 ↓0 R0 W0 0.0%` before the first message trains
//!    people to stop reading the footer.
//! 4. **`~` at a path boundary**, plus `USERPROFILE` for native Windows shells.
//! 5. **A degradation ladder** rather than a blind truncation.
//!
//! The behavioural rows live in `crates/lca-ui/src/footer/{git_tests.rs,
//! cache_tests.rs}` and `footer/mod.rs`'s `mod tests`. This is the NFR-24 named
//! guard: it fails if any of the five mechanisms is quietly reverted.

use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(rel: &str) -> String {
    let path = root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {path:?}: {err}"))
}

// Verifies: gh #389 gap 1 - discovery walks up, follows `gitdir:`, and reads a
// detached HEAD as a revision. Each of these is a distinct failure mode of the
// old `<cwd>/.git/HEAD` reader, so each is asserted separately.
#[test]
fn branch_discovery_handles_every_repository_shape() {
    let git = read("crates/lca-ui/src/footer/git.rs");
    assert!(
        git.contains("start.ancestors()"),
        "a subdirectory has no `.git` of its own: discovery must walk up"
    );
    assert!(
        git.contains(r#"strip_prefix("gitdir:")"#),
        "a worktree's `.git` is a file holding `gitdir: <path>`"
    );
    assert!(
        git.contains("commondir"),
        "a worktree keeps its own HEAD but shares refs through `commondir`"
    );
    assert!(
        git.contains("HeadRef::Detached"),
        "a detached HEAD is a bare object id, which the old `ref: ` prefix \
         check could never match"
    );
    assert!(
        git.contains("refs/heads/.invalid"),
        "`reftable` repositories write that placeholder into HEAD; showing it \
         as a branch name is the lie this gap is about"
    );
    assert!(
        git.contains("--no-optional-locks"),
        "the reftable fallback must not take `.git/index.lock`: a background \
         invocation racing the user's own commit fails it"
    );
    assert!(
        git.contains("MAX_WALK_DEPTH"),
        "the walk is bounded, so a pathological tree cannot cost unbounded \
         `stat` calls on the refresh path"
    );
}

// Verifies: gh #389 gap 2 - **the frame path performs no filesystem I/O and
// cannot spawn.** `Footer::render` reads the cached field; discovery lives
// behind a different call.
#[test]
fn the_frame_path_touches_no_filesystem() {
    let footer = read("crates/lca-ui/src/footer/mod.rs");
    let start = footer
        .find("pub fn render(&self, width: u16, theme: &Theme)")
        .expect("Footer::render exists");
    let rest = &footer[start..];
    let end = rest.find("\n    }\n").expect("render has a body");
    let body = &rest[..end];

    for forbidden in [
        "fs::read",
        "fs::metadata",
        "resolve_branch",
        "git_branch(",
        "Command::new",
        "GitCache::refr",
    ] {
        assert!(
            !body.contains(forbidden),
            "`render` runs on every streaming delta; it must not contain \
             `{forbidden}` - that is the #330 defect"
        );
    }
    assert!(
        body.contains("self.git.branch()"),
        "the branch is a cached field read"
    );
}

// Verifies: gh #389 gap 2 - the refresh rides the existing tick, and the cache
// keeps the last value when it has expired. A footer flickering between `main`
// and nothing is worse than one that is a second stale.
#[test]
fn the_cache_refreshes_on_the_tick_and_never_blanks() {
    let chat = read("crates/lca-ui/src/chat.rs");
    // Formatting-independent: `cargo fmt` may break the call across lines, so
    // match the two halves rather than the whole expression (a line-grep here
    // has already gone red once for exactly that reason).
    let inside_tick = {
        let start = chat
            .find("pub fn tick(&mut self) -> bool")
            .expect("tick exists");
        let rest = &chat[start..];
        let end = rest.find("\n    }\n").expect("tick has a body");
        &rest[..end]
    };
    assert!(
        inside_tick.contains("refresh_if_stale(") && inside_tick.contains("footer"),
        "the cadence is driven by `Chat::tick`, the seam that already exists, \
         rather than a new thread"
    );
    assert!(
        !chat.contains("thread::spawn") || !chat.contains("git_refresh"),
        "no background thread was added for the branch"
    );
    let cache = read("crates/lca-ui/src/footer/cache.rs");
    assert!(
        cache.contains("pub fn branch(&self) -> Option<&str>"),
        "the frame-facing accessor is a field read"
    );
    assert!(
        cache.contains("stale, never blank") || cache.contains("Stale, never blank"),
        "the stale-not-blank rule is documented where it is implemented"
    );
    assert!(
        cache.contains("REFRESH"),
        "the cadence is a named constant, so the report and the code agree"
    );
}

// Verifies: gh #389 gap 3 - the zero counters are suppressed. The old shape was
// one unconditional `format!` of all four counters and the ratio.
#[test]
fn zero_counters_are_suppressed() {
    let footer = read("crates/lca-ui/src/footer/mod.rs");
    assert!(
        !footer.contains("\"↑{} ↓{} R{} W{} {cache_pct:.1}%\""),
        "the unconditional zero row is the defect: each counter goes when it \
         carries no information"
    );
    assert!(
        footer.contains("(n > 0).then("),
        "a counter is rendered only when non-zero"
    );
    assert!(
        footer.contains("if cache > 0 {"),
        "the cache ratio is a ratio of those counters, so it follows them"
    );
}

// Verifies: gh #389 gap 4 - `~` shortening is a path-boundary prefix, and
// `USERPROFILE` is consulted for native Windows shells.
#[test]
fn home_shortening_is_boundary_aware_and_windows_aware() {
    let footer = read("crates/lca-ui/src/footer/mod.rs");
    let start = footer
        .find("pub fn shorten_home(")
        .expect("shorten_home exists");
    let rest = &footer[start..];
    let end = rest.find("\n}\n").expect("shorten_home has a body");
    let body = &rest[..end];
    assert!(
        body.contains("strip_prefix('/')") || body.contains("strip_prefix('\\\\')"),
        "the rest must start with a separator, or `$HOME2/x` shortens wrongly"
    );
    assert!(
        body.contains("path.to_string()"),
        "a path that is not genuinely under `home` is returned unchanged"
    );
    assert!(
        footer.contains("\"USERPROFILE\""),
        "`pwsh`/`cmd` do not set HOME; USERPROFILE is the real value there"
    );
    assert!(
        !footer.contains("std::env::var(\"HOME\").ok();"),
        "the HOME-only lookup is the #330 defect"
    );
}

// Verifies: gh #389 gap 5 - degradation drops the least informative cell
// first, and the row is bounded at every width.
#[test]
fn the_row_degrades_in_a_defined_order() {
    let footer = read("crates/lca-ui/src/footer/mod.rs");
    assert!(
        footer.contains("fn bare_model("),
        "dropping the provider prefix is the first rung"
    );
    let start = footer
        .find("let rungs = [")
        .expect("the ladder is a named list of rungs");
    let rest = &footer[start..];
    let end = rest.find("];").expect("the rung list ends");
    let rungs = &rest[..end];
    assert!(
        rungs.contains("true") && rungs.contains("false"),
        "the context cell is dropped on a later rung"
    );
    assert!(
        footer.contains("Nothing fit on even the most degraded rung")
            && footer.contains("truncate_to_width(&row, width"),
        "truncation is the last resort, after every rung has been measured"
    );
    assert!(
        footer.contains("visible_width(&row) <= width"),
        "each rung is measured before it is accepted, so the row never overflows"
    );
}
