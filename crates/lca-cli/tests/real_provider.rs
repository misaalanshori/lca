//! Real-provider smoke test (testing plan section 4): one cheap turn
//! against OpenCode Go, the endpoint `docs/providers/README.md` points
//! the built-in provider at directly. Offline like everything else by
//! default: without `OPENCODE_API_KEY` it skips cleanly (NFR-23), the
//! key is never committed, and the model list is limited to the cheap
//! test models the worker brief allows.
//!
//! Verifies: NFR-23 (real-provider tests skip when their credential is
//! absent), and the live quirks LCA-PROMPT calls out for this endpoint
//! (an OpenAI-shaped dialect over `https://opencode.ai/zen/go/v1`).

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
    let root = std::env::temp_dir().join(format!("lca-real-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let data = root.join("data");
    let config = root.join("config");
    let project = root.join("project");
    for dir in [&data, &config, &project] {
        std::fs::create_dir_all(dir).expect("mkdir");
    }
    // The ad hoc net grant the login modal will attach when the base
    // URL is named (FR-PERM-16, ADR-0022); until that flow lands in
    // Phase 7 the test writes the consent the user would give, exactly
    // as the e2e sandbox does.
    let grants = serde_json::json!({
        "version": 1,
        "projects": {
            project.canonicalize().unwrap_or_else(|_| project.clone()).to_string_lossy(): {
                "trusted": true,
                "net_patterns": ["opencode.ai"],
            }
        },
    });
    std::fs::create_dir_all(data.join("lca")).expect("mkdir lca");
    std::fs::write(
        data.join("lca/grants.json"),
        serde_json::to_vec_pretty(&grants).expect("grants serialize"),
    )
    .expect("write grants");

    // SAFETY: this test process is single-test-per-process under
    // nextest and sets these before any runtime thread spawns.
    unsafe {
        std::env::set_var("XDG_DATA_HOME", &data);
        std::env::set_var("XDG_CONFIG_HOME", &config);
        std::env::set_var("OPENAI_BASE_URL", "https://opencode.ai/zen/go/v1");
        std::env::set_var("OPENAI_MODEL", "deepseek-v4-flash");
        std::env::remove_var("OPENAI_API_KEY");
    }

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("runtime");
    let code = runtime.block_on(lca_cli::headless(
        "Reply with exactly one word: pong",
        true,
        &project,
        &[],
    ));
    assert_eq!(
        code, 0,
        "a real turn against OpenCode Go exits zero (FR-PROV-9's \
         default provider over its live endpoint)"
    );
    let _ = std::fs::remove_dir_all(&root);
}
