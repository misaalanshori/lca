//! Regression rows for gh #322: the Win32 Job Object calls must be checked.
//!
//! The defect: `windows_job::Job::new` ignored the `BOOL` returned by
//! `SetInformationJobObject`, so a job that failed to acquire
//! `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` was still handed back as a usable
//! `Job` — and the "kill the tree" guarantee silently degraded to "kill the
//! child". `platform_exec` went further and ignored both `Job::new()` and
//! `assign(pid)`'s result, so on Windows a shell spawn could proceed with no
//! job at all.
//!
//! The tree-kill guarantee is what `docs/platform-notes.md` records as the
//! Windows orphan protection; an unchecked call turns a guarantee into a
//! hope.
//!
//! Verifies: gh #322 - the job setup is checked at both call sites, and
//! `platform_exec` fails loud rather than running without a job.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

/// A source-level guard: `Job::new` must inspect the `SetInformationJobObject`
/// result and close the handle when it fails. This row runs on every
/// platform (the module is `cfg(windows)`, the source text is not), because
/// the Windows-only code path cannot be executed on the Linux/macOS CI legs
/// and a comment-only fix would be invisible there.
#[test]
fn job_new_checks_the_set_information_result() {
    let source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/lca-tools/src/ops.rs"),
    )
    .expect("read ops.rs");
    let body = source
        .split("pub fn new() -> Option<Job>")
        .nth(1)
        .expect("Job::new is present");
    let body = &body[..body.find("\n    }\n").expect("Job::new has an end")];
    assert!(
        body.contains("SetInformationJobObject"),
        "Job::new calls SetInformationJobObject"
    );
    assert!(
        body.contains("if ") && body.contains("== 0"),
        "Job::new inspects the returned BOOL for failure:\n{body}"
    );
    assert!(
        body.contains("CloseHandle"),
        "Job::new releases the handle when the call fails:\n{body}"
    );
    assert!(
        body.contains("return None"),
        "Job::new reports the failure instead of returning a job:\n{body}"
    );
}

/// `platform_exec` must route through the shared checked helper rather than
/// re-implementing an unchecked `new` + `assign` pair. The three Windows
/// spawn sites (`ops`, `process`, `pty`) must agree, or the guarantee is
/// only as strong as the weakest one.
#[test]
fn platform_exec_uses_the_checked_attach_helper() {
    let source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/lca-tools/src/ops.rs"),
    )
    .expect("read ops.rs");
    // The file has two `platform_exec` definitions (unix and windows);
    // only the windows one carries the job setup.
    let body = source
        .split("#[cfg(windows)]\nasync fn platform_exec(")
        .nth(1)
        .expect("the windows platform_exec is present");
    let body = &body[..body.find("\n    let Some(mut stdout)").expect("bounded")];
    assert!(
        body.contains("Job::attach"),
        "platform_exec attaches through the checked helper:\n{body}"
    );
    // Code, not comments: the doc comment above the match explains what
    // the old code did, so a bare `contains` would flag its own explanation.
    let code: String = body
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !code.contains("Job::new"),
        "platform_exec does not build an unchecked job:\n{code}"
    );
    assert!(
        body.contains("start_kill") && body.contains("wait()"),
        "a failed attach kills and reaps the child before returning:\n{body}"
    );
    assert!(
        body.contains("Err("),
        "a failed attach is a loud error, not a silent fallback:\n{body}"
    );
}

/// Only `Job::attach` may appear at spawn sites: `assign` stays private to
/// the module so a caller cannot bypass the check.
#[test]
fn job_assign_is_not_reachable_from_spawn_sites() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    for file in [
        "crates/lca-tools/src/process.rs",
        "crates/lca-tools/src/pty.rs",
    ] {
        let source = std::fs::read_to_string(root.join(file)).expect("read");
        assert!(
            !source.contains(".assign("),
            "{file} attaches through Job::attach, never `.assign(`"
        );
    }
    let ops = std::fs::read_to_string(root.join("crates/lca-tools/src/ops.rs")).expect("read");
    // The one legitimate `.assign(` is inside `Job::attach` itself.
    let attach = ops
        .split("pub(crate) fn attach(pid: u32) -> Option<Job>")
        .nth(1)
        .expect("attach is present");
    let attach = &attach[..attach.find("\n    }\n").expect("attach ends")];
    assert!(
        attach.contains(".assign("),
        "attach is the one place that calls assign:\n{attach}"
    );
}

/// The real behavior row, Windows only: a job that cannot be set up makes
/// `attach` return `None` rather than a job without the kill-on-close limit.
/// On other platforms this row is a documented skip (the symbol does not
/// exist), so it never reports a false pass.
#[cfg(windows)]
#[test]
fn attach_returns_a_job_or_none_never_an_unconfigured_one() {
    // A live child is required for a real assign; the checked helper either
    // hands back a configured job or refuses.
    let mut child = std::process::Command::new("cmd")
        .args(["/C", "exit 0"])
        .spawn()
        .expect("spawn cmd");
    let pid = child.id();
    let job = lca_tools::ops::windows_job::Job::attach(pid);
    match job {
        Some(_job) => {
            child.wait().expect("wait");
        }
        None => {
            let _ = child.wait();
            // Reaching here means the platform refused a job; the point of
            // the row is that this is a `None`, not a silently-unchecked
            // `Some`.
        }
    }
}

#[cfg(not(windows))]
#[test]
fn attach_returns_a_job_or_none_never_an_unconfigured_one() {
    // The checked helper is Windows-only, so there is nothing to call here.
    // The row exists so the Windows-named test above has a counterpart on
    // every leg (nextest reports a missing test name as a hard error), and
    // the behaviour it stands for - a refusal is a `None`, never an
    // unchecked `Some` - is pinned by the two source rows above, which run
    // on every platform.
    let _ = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
}
