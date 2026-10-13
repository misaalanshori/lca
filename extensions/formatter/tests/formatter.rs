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
struct TestProcess {
    mode: RefCell<Mode>,
    spawned: RefCell<Vec<String>>,
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
        }
    }

    fn missing() -> TestProcess {
        TestProcess {
            mode: RefCell::new(Mode::Missing),
            spawned: RefCell::new(Vec::new()),
        }
    }

    fn failing(stderr: &str) -> TestProcess {
        TestProcess {
            mode: RefCell::new(Mode::Failing(stderr.to_string())),
            spawned: RefCell::new(Vec::new()),
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
        match &*self.mode.borrow() {
            Mode::Ok => Ok(0),
            Mode::Failing(_) => Ok(1),
            Mode::Missing => Err("no handle".to_string()),
        }
    }

    fn read_stderr(&self, _handle: u32) -> String {
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

    fn list(&self, _dir: &str) -> Vec<String> {
        self.files.clone()
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
