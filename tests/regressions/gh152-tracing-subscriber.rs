//! GitHub #152: the tracing subscriber is installed, bounded, and records
//! grant-store failures.
//!
//! Pi ground truth: pi's `--verbose` debug logger. One test, three phases
//! in order (a single function, because the global subscriber installs
//! once per process).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use std::sync::{Arc, Mutex};
use std::time::Duration;

// Verifies: #152 (the subscriber installs into the log dir; a bounded
// rotation keeps one spare; a grant-store write failure warns).
#[test]
fn gh152_diagnostics_install_rotate_and_record() {
    let root = lca_testkit::scratch_path("gh152-diagnostics");
    let logs = root.join("logs");
    std::fs::create_dir_all(&logs).expect("mkdir");

    // Phase 1: an oversized log rotates before anything is installed.
    std::fs::write(logs.join("lca.log"), vec![b'x'; 2 * 1024 * 1024]).expect("seed");
    lca_cli::rotate_log_if_oversized(&logs.join("lca.log"));
    assert!(
        logs.join("lca.log.1").is_file(),
        "the oversized log moves aside, bounded at one spare"
    );

    // Phase 2: install and record.
    let installed = lca_cli::init_diagnostics_with_dir(false, &root).expect("init");
    assert!(installed, "first install wins");
    assert!(
        !lca_cli::init_diagnostics_with_dir(false, &root).expect("idempotent"),
        "second install is a no-op, never a panic"
    );
    tracing::warn!("gh152 probe line");
    let log = std::fs::read_to_string(logs.join("lca.log")).expect("read log");
    assert!(
        log.contains("WARN") && log.contains("gh152 probe line"),
        "the WARN line lands in the file log: {log}"
    );

    // Phase 3: a grant-store write failure warns instead of evaporating.
    let project = root.join("project");
    std::fs::create_dir_all(&project).expect("mkdir");
    let grants_path = root.join("grants.json");
    let grants = lca_permissions::GrantStore::open(&grants_path).expect("open");
    // Sabotage after open: the store read fine, but every save fails.
    let _ = std::fs::remove_file(&grants_path);
    std::fs::create_dir(&grants_path).expect("sabotage");
    let store = lca_session::SessionStore::new(root.join("data"));
    let session = store.create_session(&project, "gh152").expect("session");
    let mut tools = lca_tools::ToolExecutor::new(
        Arc::new(lca_tools::NativeOps::default()),
        project.clone(),
        project.clone(),
        65536,
        Duration::from_secs(30),
    );
    let provider = lca_testkit::FakeProvider::builder()
        .turn(|t| {
            t.tool_call("shell", r#"{"command":"echo hi"}"#)
                .usage(lca_testkit::fake_usage(10, 5, 0, 0))
        })
        .build();
    let mut prompt = Always;
    let config = lca_core::AgentConfig {
        provider: "fake".to_string(),
        model: "faux-1".to_string(),
        retry_limit: 0,
        retry_base_delay: Duration::ZERO,
        max_iterations: 5,
        ..lca_core::AgentConfig::default()
    };
    let outcome = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
        .block_on(async {
            let mut agent = lca_core::Agent::new(
                &store,
                &session,
                &provider,
                &mut tools,
                Arc::new(Mutex::new(grants)),
                &mut prompt,
                None,
                config,
            );
            agent
                .run_turn("hi", &mut Sink, &lca_tools::CancelFlag::new())
                .await
        });
    assert_eq!(outcome.status, lca_core::TurnStatus::Error);
    let log = std::fs::read_to_string(logs.join("lca.log")).expect("read log");
    assert!(
        log.contains("permission store"),
        "the write failure is a WARN line, not silence: {log}"
    );
}

struct Always;

impl lca_permissions::PermissionPrompt for Always {
    fn ask(&mut self, _action: &lca_permissions::Action) -> lca_permissions::Decision {
        lca_permissions::Decision::Always
    }

    fn review_proposals(&mut self, _diff: &lca_permissions::ProposalDiff) -> bool {
        false
    }
}

struct Sink;

impl lca_core::TurnSink for Sink {
    fn on_event(&mut self, _event: lca_core::TurnEvent) {}
}
