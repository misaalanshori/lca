//! Real-provider smoke test (testing plan section 4): one cheap turn
//! against OpenCode Go, the endpoint `docs/providers/README.md` points
//! the built-in provider at directly. Offline like everything else by
//! default: without `OPENCODE_API_KEY` it skips cleanly (NFR-23), the
//! key is never committed, and the model list is limited to the cheap
//! test models the worker brief allows.
//!
//! gh #30's fold-in: the smoke runs the binary as a child so its output
//! is readable, and a provider **usage-limit 429** becomes a *named
//! skip* (the account is out of quota - environment, NFR-23's spirit)
//! while real transport and provider errors still fail the run. The
//! model is [`lca_testkit::SMOKE_MODEL`] (`LCA_SMOKE_MODEL` overrides):
//! one const, never a hardcoded id — a delisted model must mean one edit,
//! not a repo-wide hunt.
//!
//! Verifies: NFR-23 (real-provider tests skip when their credential is
//! absent), the quota skip path (gh #30), and the live quirks
//! LCA-PROMPT calls out for this endpoint (an OpenAI-shaped dialect over
//! `https://opencode.ai/zen/go/v1`).

#[test]
fn opencode_go_completes_one_turn() {
    let Ok(key) = std::env::var("OPENCODE_API_KEY") else {
        eprintln!("skipping: OPENCODE_API_KEY is not set (NFR-23)");
        return;
    };
    if key.is_empty() {
        eprintln!("skipping: OPENCODE_API_KEY is empty (NFR-23)");
        return;
    }

    // A sandboxed user state, so the smoke never touches real sessions
    // or grants (the testkit's isolation rule, applied in-process).
    let root = lca_testkit::scratch_path("lca-real");
    let _ = std::fs::remove_dir_all(&root);
    let project = root.join("project");
    std::fs::create_dir_all(&project).expect("mkdir");
    // The ad hoc net grant the login modal will attach when the base
    // URL is named (FR-PERM-16, ADR-0022); the test writes the consent
    // the user would give, exactly as the e2e sandbox does. The store is
    // `<home>/.lca` on every platform (the 2026-10-01 data-dir move),
    // and HOME below points that home at this sandbox.
    let grants = serde_json::json!({
        "version": 1,
        "projects": {
            project.canonicalize().unwrap_or_else(|_| project.clone()).to_string_lossy(): {
                "trusted": true,
                "net_patterns": ["opencode.ai"],
            }
        },
    });
    std::fs::create_dir_all(root.join(".lca")).expect("mkdir .lca");
    std::fs::write(
        root.join(".lca/grants.json"),
        serde_json::to_vec_pretty(&grants).expect("grants serialize"),
    )
    .expect("write grants");

    // The turn runs as a child process so its output is readable: the
    // skip decision reads what the endpoint actually said (gh #30). The
    // key reaches the child through the environment and is never printed.
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_lca"))
        .args(["-p", "Reply with exactly one word: pong", "--json"])
        .current_dir(&project)
        .env("HOME", &root)
        .env("USERPROFILE", &root)
        .env("XDG_DATA_HOME", &root)
        .env("LCA_UPDATE_CHECK", "false")
        .env("OPENAI_BASE_URL", "https://opencode.ai/zen/go/v1")
        .env("OPENAI_MODEL", lca_testkit::smoke_model())
        .env("OPENCODE_API_KEY", &key)
        .env_remove("OPENAI_API_KEY")
        .output()
        .expect("spawn the smoke turn");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    match smoke_outcome(output.status.code().unwrap_or(1), &combined) {
        SmokeOutcome::Passed => {}
        SmokeOutcome::Skipped(reason) => {
            eprintln!("skipping: {reason}");
            return;
        }
        SmokeOutcome::Failed(reason) => {
            panic!("a real turn against OpenCode Go failed: {reason}\n{combined}");
        }
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// What a smoke run means (gh #30's fold-in): an account usage limit is
/// the environment speaking, not a regression - skip it by name. A
/// transport failure, an auth failure, or a non-quota 429 is what the
/// smoke exists to catch, and still fails the run.
#[derive(Debug, PartialEq, Eq)]
enum SmokeOutcome {
    /// The turn completed.
    Passed,
    /// The account is out of quota: named skip (NFR-23's spirit).
    Skipped(String),
    /// Something real broke.
    Failed(String),
}

fn smoke_outcome(exit: i32, output: &str) -> SmokeOutcome {
    if exit == 0 {
        return SmokeOutcome::Passed;
    }
    let lower = output.to_lowercase();
    let quota = (lower.contains("429") || lower.contains("http 429"))
        && (lower.contains("usage limit") || lower.contains("quota"));
    if quota {
        return SmokeOutcome::Skipped(
            "the provider reports a usage limit (HTTP 429): the account is \
             out of quota for now, which is environment, not a regression"
                .to_string(),
        );
    }
    let detail = output
        .lines()
        .rev()
        .find(|line| line.starts_with('{') || line.starts_with("error"))
        .unwrap_or("no output")
        .trim()
        .to_string();
    SmokeOutcome::Failed(format!("exit {exit}: {detail}"))
}

// Verifies: gh #30's fold-in (the skip path, not just the message): a
// mocked usage-limit 429 response decides Skipped - which is what makes
// the smoke return early with a named reason - while a transport error
// and a non-quota 429 keep failing, and exit 0 passes.
#[test]
fn a_usage_limit_response_decides_a_named_skip_and_real_errors_still_fail() {
    // The exact shape the endpoint returns on quota, quoted from the
    // live run's JSON envelope (no network in this row).
    let quota = r#"{"class":"transport","message":"retry 1/3: extension call failed: openai-compatible: provider returned HTTP 429: Go usage limit exceeded","retryable":true,"type":"error"}"#;
    let outcome = smoke_outcome(3, quota);
    let SmokeOutcome::Skipped(reason) = outcome else {
        panic!("a quota response must decide a skip, got {outcome:?}");
    };
    assert!(
        reason.contains("usage limit") && reason.contains("quota"),
        "the skip says why: {reason}"
    );

    // A transport failure is not a quota: still a failure, naming itself.
    let transport = smoke_outcome(3, r#"{"message":"connection refused"}"#);
    let SmokeOutcome::Failed(reason) = transport else {
        panic!("a transport failure must fail, got {transport:?}");
    };
    assert!(
        reason.contains("exit 3") && reason.contains("connection refused"),
        "the failure names itself: {reason}"
    );
    // A 429 that is not a quota is not skipped either.
    assert!(
        matches!(
            smoke_outcome(3, "HTTP 429: too many requests, slow down"),
            SmokeOutcome::Failed(_)
        ),
        "a rate limit that does not name the quota still fails"
    );
    assert_eq!(smoke_outcome(0, ""), SmokeOutcome::Passed);
}
