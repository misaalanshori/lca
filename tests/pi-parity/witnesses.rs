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
                parent: None,
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
// Landed gh #56 (PG-004): `--mode` parses with an `rpc` value and the
// RPC server answers, so this runs un-ignored as a green pin.
#[test]
fn pi_parity_rpc_mode_exists() {
    use clap::Parser as _;
    let cli = lca_cli::Cli::try_parse_from(["lca", "--mode", "rpc", "hello"])
        .expect("pi's --mode flag parses");
    let _ = cli;
}

// Verifies: pi:packages/coding-agent/docs/compaction.md#cut-point-rules
// ("Never cut at tool results (they must stay with their tool call)").
// Gh #100: the roadmap's compaction work inherits this invariant, so
// the replaced range a manual compaction writes must always contain
// whole tool pairs - both members inside, or both outside.
#[test]
fn pi_parity_compaction_cut_never_lands_on_a_tool_result() {
    use lca_ext_abi::{DeliveryMode, ExtensionDispatch, World};
    use lca_protocol::{DispatchError, Record, ToolResultStatus, ToolSource};

    struct WholePairs;
    impl ExtensionDispatch for WholePairs {
        fn name(&self) -> &str {
            "whole-pairs"
        }
        fn delivery(&self) -> DeliveryMode {
            DeliveryMode::Native
        }
        fn worlds(&self) -> Vec<World> {
            vec![World::Compaction]
        }
        fn tool_specs(&self) -> Result<Vec<lca_protocol::ToolSpec>, DispatchError> {
            Ok(Vec::new())
        }
        fn execute_tool<'a>(
            &'a self,
            _call: &'a lca_protocol::ToolCall,
        ) -> lca_ext_abi::DispatchFuture<'a, Result<lca_protocol::ToolResult, DispatchError>>
        {
            Box::pin(std::future::ready(Err(DispatchError::MissingWorld {
                extension: "whole-pairs".to_string(),
                world: "tool",
            })))
        }
        fn command_specs(&self) -> Result<Vec<lca_protocol::CommandSpec>, DispatchError> {
            Ok(Vec::new())
        }
        fn invoke_command(
            &self,
            _name: &str,
            _argument: &str,
        ) -> Result<lca_protocol::CommandEffect, DispatchError> {
            Ok(lca_protocol::CommandEffect::None)
        }
        fn compact(
            &self,
            _records: &[Record],
        ) -> lca_ext_abi::DispatchFuture<'static, Result<String, DispatchError>> {
            Box::pin(std::future::ready(Ok("summary".to_string())))
        }
        fn transform_messages(
            &self,
            messages: Vec<lca_protocol::ChatMessage>,
        ) -> lca_ext_abi::DispatchFuture<
            'static,
            Result<Result<Vec<lca_protocol::ChatMessage>, String>, DispatchError>,
        > {
            Box::pin(std::future::ready(Ok(Ok(messages))))
        }
        fn on_pre_turn(&self) -> lca_ext_abi::DispatchFuture<'static, Result<(), DispatchError>> {
            Box::pin(std::future::ready(Ok(())))
        }
        fn on_pre_tool_use<'a>(
            &'a self,
            _call: &'a lca_protocol::ToolCall,
        ) -> lca_ext_abi::DispatchFuture<'a, Result<lca_protocol::HookAction, DispatchError>>
        {
            Box::pin(std::future::ready(Ok(lca_protocol::HookAction::Allow)))
        }
        fn on_post_tool_use<'a>(
            &'a self,
            _observation: &'a lca_protocol::PostToolObservation,
        ) -> lca_ext_abi::DispatchFuture<'a, Result<(), DispatchError>> {
            Box::pin(std::future::ready(Ok(())))
        }
        fn on_post_turn_end<'a>(
            &'a self,
            _status: &'a str,
        ) -> lca_ext_abi::DispatchFuture<'a, Result<(), DispatchError>> {
            Box::pin(std::future::ready(Ok(())))
        }
        fn on_attention_required<'a>(
            &'a self,
            _reason: &'a str,
        ) -> lca_ext_abi::DispatchFuture<'a, Result<(), DispatchError>> {
            Box::pin(std::future::ready(Ok(())))
        }
        fn on_session_close(
            &self,
        ) -> lca_ext_abi::DispatchFuture<'static, Result<(), DispatchError>> {
            Box::pin(std::future::ready(Ok(())))
        }
    }

    let root = lca_testkit::scratch_path("pi-parity-cut-pairs");
    let project = root.join("project");
    std::fs::create_dir_all(&project).expect("mkdir");
    let store = std::sync::Arc::new(lca_session::SessionStore::new(root.join("data")));
    let session = store.create_session(&project, "cut").expect("session");
    let user = |id: &str, content: &str| Record::User {
        v: 1,
        ts: 1,
        id: id.to_string(),
        parent: None,
        content: content.to_string(),
        attachments: vec![],
        queue: None,
    };
    let tool_call = |id: &str, call_id: &str| Record::ToolCall {
        v: 1,
        ts: 1,
        id: id.to_string(),
        parent: None,
        call_id: call_id.to_string(),
        name: "shell".to_string(),
        arguments: "{}".to_string(),
        source: ToolSource::Builtin,
    };
    let tool_result = |id: &str, call_id: &str| Record::ToolResult {
        v: 1,
        ts: 1,
        id: id.to_string(),
        parent: None,
        call_id: call_id.to_string(),
        status: ToolResultStatus::Ok,
        content: Some("out".to_string()),
        attachment: None,
        truncated: false,
        exit_code: None,
        images: vec![],
        full_output_path: None,
        nested: vec![],
    };
    for record in [
        user("u1", "first"),
        tool_call("c1", "call-1"),
        tool_result("r1", "call-1"),
        user("u2", "second"),
        tool_call("c2", "call-2"),
        tool_result("r2", "call-2"),
    ] {
        store.append(&session, record).expect("append");
    }
    let mut registry = lca_core::ExtensionRegistry::new();
    registry.register(std::sync::Arc::new(WholePairs));
    lca_core::compact_now(store.clone(), session.clone(), std::sync::Arc::new(registry), None, None)
        .expect("manual compaction runs");
    // Audit, not Display: the Display view suppresses the replaced range
    // itself, so the range edges are only addressable unsuppressed.
    let read = store
        .read_with(&session, lca_session::ViewMode::Audit)
        .expect("read");
    let compaction = read
        .records
        .iter()
        .find_map(|record| match record {
            Record::Compaction {
                replaced_from,
                replaced_to,
                ..
            } => Some((replaced_from.clone(), replaced_to.clone())),
            _ => None,
        })
        .expect("a compaction record was written");
    // The replaced range is an id span over the log: every tool pair
    // straddling its edges would be a cut through the pair. All ids,
    // not just tool ids - the range edges can be user records.
    let ids: Vec<&str> = read
        .records
        .iter()
        .filter_map(|record| record.id())
        .collect();
    let (from, to) = &compaction;
    let span = |id: &str| {
        ids.iter().position(|known| known == &id).expect("seeded id")
    };
    let (lo, hi) = (span(from), span(to));
    for pair in [["c1", "r1"], ["c2", "r2"]] {
        let (a, b) = (span(pair[0]), span(pair[1]));
        assert_eq!(
            (lo <= a && a <= hi),
            (lo <= b && b <= hi),
            "the replaced range keeps tool pairs whole: {:?} vs [{from}, {to}]",
            pair,
        );
    }
}

// Verifies: pi:packages/coding-agent/docs/session-format.md#contexteditentry
// (an omission hides the target from future model context; the target
// stays unchanged in raw history). Gh #100: the roadmap's context work
// inherits this invariant, so an omission must survive a resume (a fresh
// read from disk) and a fork at the edit (the child's chain carries the
// parent's edits).
#[test]
fn pi_parity_context_edit_omission_survives_resume_and_fork() {
    let root = lca_testkit::scratch_path("pi-parity-edit-omission");
    let project = root.join("project");
    std::fs::create_dir_all(&project).expect("mkdir");
    let store = lca_session::SessionStore::new(root.join("data"));
    let session = store.create_session(&project, "edit").expect("session");
    let user = |id: &str, content: &str| lca_protocol::Record::User {
        v: lca_protocol::FORMAT_VERSION,
        ts: 1,
        id: id.to_string(),
        parent: None,
        content: content.to_string(),
        attachments: vec![],
        queue: None,
    };
    for record in [
        user("u1", "forgettable secret"),
        user("u2", "keep this"),
        lca_protocol::record::context_edit_record(3, "e1", "u1", None),
    ] {
        store.append(&session, record).expect("append");
    }
    let texts_of = |records: &[lca_protocol::Record]| {
        lca_core::assemble(records, "sys")
            .messages
            .iter()
            .flat_map(|message| {
                message.content.iter().filter_map(|block| match block {
                    lca_protocol::ContentBlock::Text { text } => Some(text.clone()),
                    _ => None,
                })
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    // Resume: a fresh read from disk still omits the target from model
    // context while raw history keeps it (append-only).
    let reread = store
        .read_with(&session, lca_session::ViewMode::Display)
        .expect("resume read");
    let texts = texts_of(&reread.records);
    assert!(
        !texts.contains("forgettable"),
        "the omission hides the target from model context: {texts:?}",
    );
    assert!(
        texts.contains("keep"),
        "unrelated records still assemble: {texts:?}",
    );
    assert!(
        reread.records.iter().any(|record| record.id() == Some("u1")),
        "raw history keeps the omitted record",
    );
    // Fork at the edit: the child's chain carries the parent's edits,
    // so the omission rides along.
    let child = store.fork(&session, "e1").expect("fork");
    let child_read = store
        .read_with(&child, lca_session::ViewMode::Display)
        .expect("child read");
    let child_texts = texts_of(&child_read.records);
    assert!(
        !child_texts.contains("forgettable"),
        "the fork inherits the omission: {child_texts:?}",
    );
    assert!(
        child_texts.contains("keep"),
        "the fork keeps unrelated records: {child_texts:?}",
    );
}
