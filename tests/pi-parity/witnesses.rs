//! Red witnesses for future milestones (RM-001 rule: named here AND in
//! `docs/pi-parity.md` with their owning issue, failing ONLY on the
//! not-yet-built behavior).
//!
//! `#[ignore]`-gated so main stays green — the one sanctioned exception to
//! green pins, and the reason is structural: CI runs this target on every
//! push, so a hard-red witness would hold main red. Run them with
//! `cargo nextest run -E 'test(pi_parity)' --run-ignored`.

use std::collections::BTreeMap;

// Verifies: pi:packages/coding-agent/docs/compaction.md#when-it-triggers
// (auto-compaction triggers on a token budget). Landed gh #36 phase 1:
// the budget keys exist in our snake_case vocabulary, so this witness
// runs un-ignored (its camelCase spelling predates the vocabulary).
#[test]
fn pi_parity_compaction_uses_token_budget_trigger() {
    let mut flags = BTreeMap::new();
    flags.insert("compaction.reserve_tokens".to_string(), "16384".to_string());
    flags.insert(
        "compaction.keep_recent_tokens".to_string(),
        "20000".to_string(),
    );
    let config = lca_config::Config::load(&lca_config::LoadInput {
        flags,
        ..Default::default()
    })
    .expect("pi's compaction budget keys load");
    assert_eq!(
        config.compaction_threshold(),
        0.8,
        "fraction stays until budgets land"
    );
}

// Verifies: pi:packages/coding-agent/docs/settings.md (`defaultTools`:
// `read`, `bash`, `edit`, `write`).
// Witness for #39 (EFG-005, tool-set parity): LCA's shell tool is still
// named `shell`, so a pi-shaped `bash` dispatch fails.
#[test]
#[ignore = "witness for #39: the builtin set has no bash yet"]
fn pi_parity_builtin_bash_tool_name() {
    assert!(
        lca_core::BUILTIN_TOOLS.contains(&"bash"),
        "pi's default tool loadout names bash, not shell"
    );
}

// Verifies: pi:packages/coding-agent/docs/session-format.md#entry-base
// (entries form one in-file tree via `id`/`parentId`).
// Witness for #37 (EFG-002, RM-011): LCA forks into a second directory
// with a `fork-point` record, so the child's history is not in its file.
#[test]
#[ignore = "witness for #37: forks are directories, not in-file branches"]
fn pi_parity_session_tree_branches_in_file() {
    let root = lca_testkit::scratch_path("pi-parity-witness-fork");
    let project = root.join("project");
    std::fs::create_dir_all(&project).expect("mkdir");
    let store = lca_session::SessionStore::new(root.join("data"));
    let parent = store.create_session(&project, "parent").expect("session");
    store
        .append(
            &parent,
            lca_protocol::Record::User {
                v: 1,
                ts: 1,
                id: "u1".to_string(),
                content: "shared history".to_string(),
                attachments: vec![],
                queue: None,
            },
        )
        .expect("append");
    let child = store.fork(&parent, "u1").expect("fork");
    let raw = std::fs::read_to_string(child.log_path()).expect("child log");
    assert!(
        raw.contains("shared history"),
        "in-file branching keeps the parent's records in the child's file"
    );
}

// Verifies: pi:packages/coding-agent/docs/cli.md (`--mode rpc`) and
// pi:packages/coding-agent/docs/rpc.md (bidirectional JSONL protocol).
// Witness for #56 (PG-004, RPC/JSON event parity): LCA has no `--mode`
// flag and no RPC server yet.
#[test]
#[ignore = "witness for #56: no --mode flag yet"]
fn pi_parity_rpc_mode_exists() {
    use clap::Parser as _;
    let cli = lca_cli::Cli::try_parse_from(["lca", "--mode", "rpc", "hello"])
        .expect("pi's --mode flag parses");
    let _ = cli;
}
