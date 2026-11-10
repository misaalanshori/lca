//! Auto-format on file-mutating tool results (gh #220): after a
//! successful `edit` or `write`, the project's formatter runs over
//! the touched file before the next step. Formatter missing,
//! formatter failing, unknown extension, or toggled-off all degrade
//! to a warning plus an untouched result — formatting must never
//! break a turn.
//!
//! Verifies: formatter selection per extension with config-file
//! detection, path extraction from edit/write args only, the hook
//! outcome shapes, the on/off/run commands, and the manifest grants.

use formatter::{FormatterState, HookOutcome, select_formatter};

fn has_file<'a>(present: &'a [&'a str]) -> impl Fn(&str) -> bool + 'a {
    move |path| present.contains(&path)
}

// Verifies: every documented extension maps to its formatter, with
// config-file detection choosing ruff over black and biome over
// prettier.
#[test]
fn formatter_selection_per_extension() {
    let none = has_file(&[]);
    assert_eq!(
        select_formatter("src/main.rs", &none).map(|command| command.program),
        Some("rustfmt".to_string())
    );
    assert_eq!(
        select_formatter("main.go", &none).map(|command| command.program),
        Some("gofmt".to_string())
    );
    assert_eq!(
        select_formatter("main.zig", &none).map(|command| command.program),
        Some("zig".to_string())
    );
    // Python: ruff when configured, black otherwise.
    let ruff = has_file(&["pyproject.toml"]);
    assert_eq!(
        select_formatter("a.py", &ruff).map(|command| command.program),
        Some("ruff".to_string())
    );
    assert_eq!(
        select_formatter("a.py", &none).map(|command| command.program),
        Some("black".to_string())
    );
    // TypeScript: biome when configured, prettier otherwise.
    let biome = has_file(&["biome.json"]);
    assert_eq!(
        select_formatter("a.ts", &biome).map(|command| command.program),
        Some("biome".to_string())
    );
    assert_eq!(
        select_formatter("a.ts", &none).map(|command| command.program),
        Some("prettier".to_string())
    );
    // Unknown extensions select nothing (silent skip, never error).
    assert!(select_formatter("notes.md", &none).is_none());
    assert!(select_formatter("Makefile", &none).is_none());
}

// Verifies: formatter arguments carry the file in the documented
// shape.
#[test]
fn formatter_arguments_carry_the_file() {
    let none = has_file(&[]);
    let rust = select_formatter("src/main.rs", &none).expect("rustfmt");
    assert!(rust.args.contains(&"src/main.rs".to_string()), "{rust:?}");
    let go = select_formatter("main.go", &none).expect("gofmt");
    assert_eq!(go.args[0], "-w");
}

// Verifies: gh #284 - **every built-in writing tool's path extracts**, and
// nothing else does.
//
// The hook covered `edit` and `write` only, and the tree ships a third writing
// tool: `hash_edit` (extensions/hash-edit, args `{path, edits}`). A session whose
// model used hash_edit therefore never auto-formatted - the hook returned
// `Skipped("no formattable file in this call")` and nothing said so.
//
// `apply_patch` is deliberately absent: gh #284's title named it, but no crate,
// manifest or doc in this tree declares such a tool (`grep -rn "apply_patch"
// --include=*.rs --include=*.md --include=*.toml .` is empty outside `target`), so
// supporting it here would be inventing a contract for a name nothing produces.
// If a writing tool is added later it is added here with the same one-line reason:
// it mutates a file the formatter may claim.
#[test]
fn a_hash_edit_path_extracts_and_the_negative_cases_stay_negative() {
    // The declared shape: `{"path": ..., "edits": [...]}`.
    assert_eq!(
        formatter::extract_path("hash_edit", r#"{"path": "src/a.rs", "edits": []}"#),
        Some("src/a.rs".to_string()),
        "hash_edit is a writing tool, so its path must reach the hook"
    );
    // The negative cases, unchanged: non-mutating tools and a malformed body.
    for tool in ["read", "shell"] {
        assert_eq!(
            formatter::extract_path(tool, r#"{"path": "src/a.rs"}"#),
            None,
            "`{tool}` does not mutate a file"
        );
    }
    assert_eq!(
        formatter::extract_path("hash_edit", r#"{"edits": []}"#),
        None,
        "a hash_edit with no path extracts nothing"
    );
    assert_eq!(
        formatter::extract_path("hash_edit", "not json"),
        None,
        "a malformed body extracts nothing"
    );
}

// Verifies: paths extract from edit/write arguments only — a `path`
// field on any other tool (read included) never triggers a format.
#[test]
fn paths_extract_from_edit_and_write_only() {
    let args = r#"{"path": "src/main.rs"}"#;
    assert_eq!(
        formatter::extract_path("edit", args),
        Some("src/main.rs".to_string())
    );
    assert_eq!(
        formatter::extract_path("write", args),
        Some("src/main.rs".to_string())
    );
    assert_eq!(formatter::extract_path("read", args), None);
    assert_eq!(formatter::extract_path("shell", args), None);
    assert_eq!(formatter::extract_path("edit", r#"{"other": 1}"#), None);
    assert_eq!(formatter::extract_path("edit", "not json"), None);
}

// Verifies: gh #284 - **a hash_edit call really gets formatted end to end.**
//
// The sibling row above pins the extraction; this one pins that the *hook* uses
// it, which is the half that was silently missing: `handle_result` returned
// `Skipped("no formattable file in this call")` for a hash_edit result, so the
// formatter never spawned and nothing said so. Asserting on `spawned` (not on the
// extracted path) is deliberate - a row that only checked `extract_path` would
// have passed on the defect the moment the list changed, without proving the
// result reaches the process at all.
#[test]
fn a_hash_edit_result_is_formatted_end_to_end() {
    let state = FormatterState::new();
    let process = TestProcess::ok();
    let outcome = state.handle_result(
        &process,
        &TestFs::empty(),
        "hash_edit",
        r#"{"path": "src/a.rs", "edits": [{"start_hash": "ab", "end_hash": "", "new_text": "x"}]}"#,
        true,
    );
    assert!(
        matches!(outcome, HookOutcome::Formatted { .. }),
        "a hash_edit result formats like edit/write: {outcome:?}"
    );
    assert!(
        process.spawned.borrow().iter().any(|p| p == "rustfmt"),
        "the formatter actually spawned for a hash_edit path: {:?}",
        process.spawned.borrow()
    );
    // The negative control, so this row cannot pass by formatting everything:
    // a non-mutating tool with the same path field still skips, and spawns nothing.
    let read = TestProcess::ok();
    let skipped = state.handle_result(
        &read,
        &TestFs::empty(),
        "read",
        r#"{"path": "src/a.rs"}"#,
        true,
    );
    assert!(matches!(skipped, HookOutcome::Skipped(_)), "{skipped:?}");
    assert!(
        read.spawned.borrow().is_empty(),
        "`read` mutates nothing, so nothing spawned: {:?}",
        read.spawned.borrow()
    );
}

// Verifies: gh #284 - **the failing formatter's own words reach the warning, and
// the tail is read before the wait.**
//
// The native `read_stderr` was a hardcoded `String::new()`, so every failing
// formatter reported `"rustfmt exited 1 for src/x.rs: "` - a dangling colon, which
// is exactly the copy `docs/testing-plan.md`'s error rule forbids ("tell the user
// what to DO"). The fix is `lca-tools`' bounded read (gh #459's
// `process_read_stderr_buffered`), and the subtlety is **ordering**: `process_wait`
// removes the handle from the engine's table (`take_process`), so a tail read after
// the wait can never see the child's words. This fake records the call order, so a
// future reorder fails here rather than in a user's terminal.
#[test]
fn a_failing_formatters_stderr_reaches_the_warning_after_a_read_that_precedes_the_wait() {
    let state = FormatterState::new();
    let process = TestProcess::failing("error: unexpected token");
    let outcome = state.handle_result(
        &process,
        &TestFs::empty(),
        "write",
        r#"{"path": "src/main.rs"}"#,
        true,
    );
    match &outcome {
        HookOutcome::Warning(note) => {
            assert!(
                note.contains("unexpected token"),
                "the formatter's own words reach the warning: {note}"
            );
            assert!(
                !note.trim_end().ends_with("src/main.rs:") && !note.ends_with(": "),
                "and the warning does not end in a dangling colon: {note:?}"
            );
        }
        other => panic!("a failing formatter warns, got {other:?}"),
    }
    let calls: Vec<&str> = process.calls.borrow().clone();
    let read = calls
        .iter()
        .position(|call| *call == "read_stderr")
        .expect("the tail was read at all");
    let wait = calls
        .iter()
        .position(|call| *call == "wait")
        .expect("the child was waited for");
    assert!(
        read < wait,
        "the stderr read happens BEFORE the wait - `process_wait` drops the handle,          so a read after it can only answer `not found` (gh #284): {calls:?}"
    );
}

// Verifies: gh #284 - the 300-char tail survives the move to a real read.
//
// The existing behaviour is kept deliberately: the warning carries the **last**
// 300 characters of what the formatter said, because the line that explains a
// failure is the last one (a rustfmt error prints the snippet before the reason).
// Before the fix this could not be tested at all - the tail was taken from an
// empty string - so "the tail is 300 chars" was a claim about a `take(300)` call
// rather than about any bytes.
#[test]
fn a_chatty_failing_formatter_is_warned_with_its_last_three_hundred_characters() {
    let state = FormatterState::new();
    // 400 characters, with the distinguishable end.
    let long = format!("{}{}", "x".repeat(390), "TAIL-END");
    let process = TestProcess::failing(&long);
    let outcome = state.handle_result(
        &process,
        &TestFs::empty(),
        "write",
        r#"{"path": "src/main.rs"}"#,
        true,
    );
    match &outcome {
        HookOutcome::Warning(note) => {
            assert!(
                note.contains("TAIL-END"),
                "the tail keeps the end, which is the part that explains the failure: {note}"
            );
            // The warning is `prog exited N for path: <tail>`; the tail itself is
            // capped at 300 chars, so the whole note is bounded.
            let tail_len = note.rsplit(": ").next().unwrap_or("").chars().count();
            assert!(
                tail_len <= 300,
                "the tail is capped at 300 characters, got {tail_len}: {note}"
            );
            // A tail, not the whole thing: the full 400 characters cannot fit.
            assert!(
                !note.contains(&long),
                "the whole 400-character message does not fit in the tail: {note}"
            );
            assert!(
                note.contains(&"x".repeat(290)),
                "the tail is the LAST 300 characters, so it is 290 x's then TAIL-END: {note}"
            );
        }
        other => panic!("a failing formatter warns, got {other:?}"),
    }
}

// Verifies: the hook outcome matrix — only enabled + ok + known
// file formats; everything else names its skip.
#[test]
fn the_hook_formats_only_when_everything_lines_up() {
    let state = FormatterState::new();
    let run = |tool: &str, status_ok: bool| {
        state.handle_result(
            &TestProcess::ok(),
            &TestFs::empty(),
            tool,
            r#"{"path": "src/main.rs"}"#,
            status_ok,
        )
    };
    assert!(matches!(run("write", true), HookOutcome::Formatted { .. }));
    assert!(matches!(run("edit", true), HookOutcome::Formatted { .. }));
    assert!(
        matches!(run("write", false), HookOutcome::Skipped(_)),
        "failed results never format"
    );
    assert!(
        matches!(run("read", true), HookOutcome::Skipped(_)),
        "non-mutating tools never format"
    );
    let off = FormatterState::disabled();
    assert!(
        matches!(
            off.handle_result(
                &TestProcess::ok(),
                &TestFs::empty(),
                "write",
                r#"{"path": "src/main.rs"}"#,
                true
            ),
            HookOutcome::Skipped(_)
        ),
        "toggled off formats nothing"
    );
}

// Verifies: a missing formatter binary warns instead of failing,
// and an unparseable file (formatter exit nonzero) warns with the
// formatter's own stderr tail.
#[test]
fn missing_or_failing_formatters_warn() {
    let state = FormatterState::new();
    let missing = state.handle_result(
        &TestProcess::missing(),
        &TestFs::empty(),
        "write",
        r#"{"path": "src/main.rs"}"#,
        true,
    );
    assert!(matches!(missing, HookOutcome::Warning(_)), "{missing:?}");
    let failing = state.handle_result(
        &TestProcess::failing("error: expected `;`"),
        &TestFs::empty(),
        "write",
        r#"{"path": "src/main.rs"}"#,
        true,
    );
    match failing {
        HookOutcome::Warning(note) => assert!(note.contains("expected `;`"), "{note}"),
        other => panic!("warns with stderr, got {other:?}"),
    }
}

// Verifies: the commands — on/off flip the flag, run formats a file
// directly and reports, run with nothing walks the workspace.
#[test]
fn commands_toggle_and_run() {
    let state = FormatterState::new();
    assert!(state.enabled());
    let empty_process = TestProcess::ok();
    let empty_fs = TestFs::empty();
    assert!(
        state
            .handle_command(&empty_process, &empty_fs, "off")
            .contains("off")
    );
    assert!(!state.enabled());
    assert!(
        state
            .handle_command(&empty_process, &empty_fs, "on")
            .contains("on")
    );
    assert!(state.enabled());
    let report = state.handle_command(
        &TestProcess::ok(),
        &TestFs::with(&["src/main.rs", "notes.md"]),
        "run src/main.rs",
    );
    assert!(report.contains("src/main.rs"), "{report}");
    assert!(!report.contains("notes.md"), "{report}");
    let walked = state.handle_command(&TestProcess::ok(), &TestFs::with(&["a.py", "b.rs"]), "run");
    assert!(
        walked.contains("a.py") && walked.contains("b.rs"),
        "{walked}"
    );
    let unknown = state.handle_command(&empty_process, &empty_fs, "frobnicate");
    assert!(unknown.contains("on|off|run"), "{unknown}");
}

use std::cell::RefCell;

// A canned process backend.
//
// Gh #284: it records the **call order** of the two result reads, because the
// interesting half of the stderr fix is ordering rather than text - the engine's
// `process_wait` removes the handle from its table, so a stderr read after the
// wait can never see the failing child's words. A fake that answered
// `read_stderr` happily in either order would pass a row the real engine fails.
struct TestProcess {
    mode: RefCell<Mode>,
    spawned: RefCell<Vec<String>>,
    calls: RefCell<Vec<&'static str>>,
}

#[derive(Clone)]
enum Mode {
    Ok,
    Missing,
    Failing(String),
}

impl TestProcess {
    fn ok() -> TestProcess {
        TestProcess {
            mode: RefCell::new(Mode::Ok),
            spawned: RefCell::new(Vec::new()),
            calls: RefCell::new(Vec::new()),
        }
    }

    fn missing() -> TestProcess {
        TestProcess {
            mode: RefCell::new(Mode::Missing),
            spawned: RefCell::new(Vec::new()),
            calls: RefCell::new(Vec::new()),
        }
    }

    fn failing(stderr: &str) -> TestProcess {
        TestProcess {
            mode: RefCell::new(Mode::Failing(stderr.to_string())),
            spawned: RefCell::new(Vec::new()),
            calls: RefCell::new(Vec::new()),
        }
    }
}

impl formatter::ProcessView for TestProcess {
    fn spawn(&self, program: &str, _args: &[String]) -> Result<u32, String> {
        self.spawned.borrow_mut().push(program.to_string());
        match &*self.mode.borrow() {
            Mode::Ok | Mode::Failing(_) => Ok(1),
            Mode::Missing => Err("no such binary".to_string()),
        }
    }

    fn wait(&self, _handle: u32) -> Result<i32, String> {
        self.calls.borrow_mut().push("wait");
        match &*self.mode.borrow() {
            Mode::Ok => Ok(0),
            Mode::Failing(_) => Ok(1),
            Mode::Missing => Err("no handle".to_string()),
        }
    }

    /// Gh #284: answers the shape the real engine answers through
    /// `process_read_stderr_buffered` - what the child has **already** written, never
    /// a wait, and nothing for a live child that wrote nothing. The call log is what
    /// lets a row assert the ordering the engine imposes.
    fn read_stderr(&self, _handle: u32) -> String {
        self.calls.borrow_mut().push("read_stderr");
        match &*self.mode.borrow() {
            Mode::Failing(text) => text.clone(),
            _ => String::new(),
        }
    }
}

// A canned workspace: files that exist, with config probes.
struct TestFs {
    files: Vec<String>,
}

impl TestFs {
    fn empty() -> TestFs {
        TestFs { files: Vec::new() }
    }

    fn with(files: &[&str]) -> TestFs {
        TestFs {
            files: files.iter().map(|file| file.to_string()).collect(),
        }
    }
}

impl formatter::FsView for TestFs {
    fn exists(&self, path: &str) -> bool {
        self.files.iter().any(|file| file == path)
    }

    /// A flat fixture: `list("")` is the whole set, `list(dir)` is empty.
    /// The command tests pass explicit paths, so only the root listing is
    /// exercised here.
    fn list(&self, dir: &str) -> Vec<String> {
        if dir.is_empty() {
            self.files.clone()
        } else {
            Vec::new()
        }
    }

    fn stat(&self, path: &str) -> Option<(bool, u64)> {
        self.files
            .iter()
            .find(|file| file.as_str() == path)
            .map(|_| (false, 0))
    }
}

// Verifies: the manifest test contract — hooks-tool-result plus
// command worlds; process plus workspace read-write.
#[test]
fn manifest_grants_match_the_manifest() {
    let manifest = formatter::MANIFEST;
    assert!(
        manifest.contains("worlds = [\"hooks-tool-result\", \"command\"]"),
        "{manifest}"
    );
    let grants = formatter::manifest_grants();
    assert!(grants.process, "formatter binaries run as processes");
    assert_eq!(grants.fs.len(), 1, "workspace read-write only");
    assert!(manifest.contains("[capabilities.process]"), "{manifest}");
    assert!(
        manifest.contains("workspace = \"read-write\""),
        "{manifest}"
    );
}

// ---------------------------------------------------------------------------
// gh #357: a bare run must never walk Git or build roots
// ---------------------------------------------------------------------------
//
// `/formatter run` with no argument descended every directory the `fs`
// capability could list, so it rewrote `target/**/*.rs` (build-script output:
// rebuild churn, or divergent generated code) and stat'ed `.git/objects`
// file-by-file through the capability boundary for nothing.
//
// The skip rule itself is `lca_tools::SKIP_DIRS`, shared with the `glob`/`grep`
// walker rather than re-spelled here - two lists drift, and the drift is
// invisible until one of them rewrites a generated file.

// A fake workspace that answers `list`/`stat` from a fabricated tree, so the
// walker can be driven over `.git/`, `target/`, and `node_modules/` without a
// real filesystem.
//
// `tracked` records every path the walker stat'ed: the wasted-traversal half
// of the defect is that `.git/objects` was reached at all, and counting stats
// is how that is pinned.
struct TreeFs {
    files: Vec<String>,
    tracked: RefCell<Vec<String>>,
}

impl TreeFs {
    fn new(files: &[&str]) -> TreeFs {
        TreeFs {
            files: files.iter().map(|file| file.to_string()).collect(),
            tracked: RefCell::new(Vec::new()),
        }
    }

    /// What a bare `run` would hand to a formatter, driven through the real
    /// command path so the walker (not a flat filter) decides.
    fn bare_run_targets(&self) -> Vec<String> {
        let state = FormatterState::new();
        let process = TestProcess::ok();
        let report = state.handle_command(&process, self, "run");
        // One line per target: `path: program` or `path: reason`.
        report
            .lines()
            .filter(|line| line.contains(": rustfmt") || line.contains(": prettier"))
            .filter_map(|line| line.split(": ").next().map(str::to_string))
            .collect()
    }
}

impl formatter::FsView for TreeFs {
    fn exists(&self, path: &str) -> bool {
        self.files.iter().any(|file| file == path)
    }

    /// One level: the **entry names** directly under `dir` (not full paths),
    /// matching `FsView::list`'s contract.
    fn list(&self, dir: &str) -> Vec<String> {
        let prefix = if dir.is_empty() {
            String::new()
        } else {
            format!("{dir}/")
        };
        let mut children: Vec<String> = Vec::new();
        for path in &self.files {
            let Some(rest) = path.strip_prefix(&prefix) else {
                continue;
            };
            if rest.is_empty() || path == dir {
                continue;
            }
            let child = rest.split('/').next().unwrap_or(rest);
            if child.is_empty() {
                continue;
            }
            if !children.iter().any(|seen: &String| seen == child) {
                children.push(child.to_string());
            }
        }
        children
    }

    /// Records every path the walker stats: the wasted-traversal half of the
    /// defect is that `.git/objects` was reached at all, and counting stats is
    /// how that is pinned.
    fn stat(&self, path: &str) -> Option<(bool, u64)> {
        self.tracked.borrow_mut().push(path.to_string());
        if self.files.iter().any(|file| file == path) {
            return Some((false, 0));
        }
        let prefix = format!("{path}/");
        if self.files.iter().any(|file| file.starts_with(&prefix)) {
            return Some((true, 0));
        }
        None
    }
}

// Verifies: gh #357 acceptance 1 and 3 - a walked tree containing a VCS root,
// a build root, and a package root yields exactly the source file.
#[test]
fn a_bare_walk_skips_git_target_and_node_modules() {
    let fs = TreeFs::new(&[
        ".git/objects/ab/cdef",
        ".git/HEAD",
        "target/out/y.rs",
        "target/debug/build/x/out/gen.rs",
        "node_modules/p/z.js",
        "src/z.rs",
    ]);
    let found = fs.bare_run_targets();
    assert_eq!(
        found,
        vec!["src/z.rs"],
        "only the source file is formattable; the walked tree has four others"
    );
}

// Verifies: gh #357 - the skip happens at descent time, not by filtering
// afterwards: nothing inside a skip root is stat'ed at all. That is the
// wasted-traversal half of the defect (`.git/objects` on a real repo is tens
// of thousands of capability-boundary stats per bare run).
#[test]
fn nothing_inside_a_skip_root_is_stat_ed() {
    let fs = TreeFs::new(&[
        ".git/objects/ab/cdef",
        ".git/objects/ef/0123",
        "target/out/y.rs",
        "node_modules/p/z.js",
        "src/z.rs",
    ]);
    let _ = fs.bare_run_targets();
    let stats = fs.tracked.borrow().clone();
    for stat in &stats {
        assert!(
            !stat.starts_with(".git/"),
            "the walk never enters .git/ (it stat'ed {stat})"
        );
        assert!(
            !stat.starts_with("target/"),
            "the walk never enters target/ (it stat'ed {stat})"
        );
        assert!(
            !stat.starts_with("node_modules/"),
            "the walk never enters node_modules/ (it stat'ed {stat})"
        );
    }
    assert!(
        stats.iter().any(|stat| stat == "src/z.rs"),
        "and it still reached the source tree: {stats:?}"
    );
}

// Verifies: gh #357 - component-wise matching. A directory merely *starting*
// with a skip name is ordinary source (`src/targeted/x.rs`), and a file whose
// name starts with one is ordinary too (`src/.gitignore.rs`). A substring
// test would silently stop formatting these, which is the failure mode that
// would neuter the tool.
#[test]
fn a_name_that_merely_starts_with_a_skip_root_is_still_formatted() {
    for path in [
        "src/targeted/x.rs",
        "src/target_helpers/mod.rs",
        "src/.gitignore.rs",
        "src/node_modules_helper.rs",
        "targeted/y.rs",
        "src/git-tools/main.rs",
    ] {
        assert!(
            !formatter::is_skipped_path(path),
            "{path} is ordinary source, not under a skip root"
        );
        assert_eq!(formatter::refusal_reason(path), None, "{path}");
    }

    // And a component equal to a skip name is caught at any depth.
    for path in [
        ".git/config",
        "target/debug/x.rs",
        "nested/target/gen.rs",
        "src/node_modules/vendored.js",
        "./target/out/y.rs",
        "target/",
    ] {
        assert!(
            formatter::is_skipped_path(path),
            "{path} lies under a skip root"
        );
        assert!(formatter::refusal_reason(path).is_some(), "{path}");
    }
}

// Verifies: gh #357 acceptance 2 - an explicitly named path under a skip root
// is refused with a reason that names the root, not formatted.
#[test]
fn an_explicitly_named_path_under_a_skip_root_is_refused_with_a_reason() {
    for (path, root) in [
        ("target/gen/out.rs", "target"),
        (".git/hooks/pre-commit.rs", ".git"),
        ("node_modules/pkg/index.js", "node_modules"),
        ("crates/x/target/debug/build/y.rs", "target"),
    ] {
        let reason =
            formatter::refusal_reason(path).unwrap_or_else(|| panic!("{path} must be refused"));
        assert!(reason.contains(root), "names the root `{root}`: {reason}");
        assert!(
            reason.contains("refused"),
            "the refusal is stated, not implied: {reason}"
        );
        assert!(
            reason.contains("generated") || reason.contains("version-control"),
            "and says why: {reason}"
        );
    }
    // An ordinary source path has no refusal reason.
    assert_eq!(formatter::refusal_reason("src/main.rs"), None);
}

// Verifies: gh #357 acceptance 2 through the command surface - `run <path>`
// under a skip root reports the refusal and never spawns a formatter.
#[test]
fn the_run_command_refuses_a_skip_root_path() {
    let state = FormatterState::new();
    let process = TestProcess::ok();
    let fs = TestFs::with(&["target/gen/out.rs"]);
    let report = state.handle_command(&process, &fs, "run target/gen/out.rs");
    assert!(report.contains("refused"), "{report}");
    assert!(report.contains("target"), "names the root: {report}");
    assert!(
        process.spawned.borrow().is_empty(),
        "nothing was spawned for a refused path: {:?}",
        process.spawned.borrow()
    );
}

// Verifies: gh #357 - the positive control. A non-skipped nested directory
// still formats. Without this row the fix could pass every skip test by
// simply formatting nothing.
#[test]
fn a_non_skipped_nested_directory_still_formats() {
    let state = FormatterState::new();
    let process = TestProcess::ok();
    // Deep, non-skip nesting: the walk must still reach it.
    let fs = TreeFs::new(&["src/deep/nested/module/main.rs"]);
    assert_eq!(
        fs.bare_run_targets(),
        vec!["src/deep/nested/module/main.rs"]
    );

    let report = state.handle_command(&process, &fs, "run src/deep/nested/module/main.rs");
    assert!(
        report.contains("rustfmt"),
        "the file formats normally: {report}"
    );
    assert!(
        process.spawned.borrow().iter().any(|p| p == "rustfmt"),
        "rustfmt actually ran: {:?}",
        process.spawned.borrow()
    );
}

// Verifies: gh #357 - the hook path is gated too. A model that edits a
// generated file (`target/…`) must not trigger a reformat of build output.
#[test]
fn the_hook_does_not_format_a_generated_file_the_model_edited() {
    let state = FormatterState::new();
    let process = TestProcess::ok();
    let outcome = state.handle_result(
        &process,
        &TestFs::empty(),
        "write",
        r#"{"path": "target/debug/build/x/out/gen.rs"}"#,
        true,
    );
    assert!(matches!(outcome, HookOutcome::Refused(_)), "{outcome:?}");
    assert!(
        process.spawned.borrow().is_empty(),
        "no formatter spawned for a build-root path"
    );
}
