//! Project memory read/write round trip and the pinned snapshot
//! injection (gh #199): the `learn` tool appends facts to MEMORY.md,
//! the transform injects a frozen block at the head of the system
//! prompt, and global writes refuse in untrusted workspaces.
//!
//! Verifies: learn appends project facts by default, `scope: global`
//! lands in the separate global file, global refuses when untrusted,
//! the snapshot pins at first read (a mid-session learn does not move
//! `messages[0]`), the block respects the token budget, and the log is
//! untouched (the injection exists only in the returned list).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::collections::HashMap;
use std::sync::Mutex;

use lca_protocol::{ChatMessage, MessageRole};
use project_memory::{MemoryScope, ProjectMemory, read_memory, snapshot_block};

/// An in-memory two-scope filesystem: (scope, path) to text.
struct FakeFs {
    files: Mutex<HashMap<(String, String), String>>,
}

impl FakeFs {
    fn new() -> FakeFs {
        FakeFs {
            files: Mutex::new(HashMap::new()),
        }
    }
}

impl project_memory::FsView for FakeFs {
    fn read(
        &self,
        scope: &str,
        path: &str,
    ) -> Result<Option<String>, lca_protocol::CapabilityError> {
        Ok(self
            .files
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .get(&(scope.to_string(), path.to_string()))
            .cloned())
    }

    fn write(
        &self,
        scope: &str,
        path: &str,
        text: &str,
    ) -> Result<(), lca_protocol::CapabilityError> {
        self.files
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .insert((scope.to_string(), path.to_string()), text.to_string());
        Ok(())
    }
}

// Verifies: the first fact creates the project file with a header;
// the second appends; both survive the read-back.
#[test]
fn learn_appends_project_facts_by_default() {
    let fs = FakeFs::new();
    let memory = ProjectMemory::new_trusted(&fs);
    memory
        .learn("Use pnpm exclusively.", MemoryScope::Project)
        .expect("learn");
    memory
        .learn("Mock API runs on port 8081.", MemoryScope::Project)
        .expect("learn");
    let text = read_memory(&fs, MemoryScope::Project).expect("read");
    assert!(text.contains("# Project Memory"), "{text}");
    assert!(text.contains("- Use pnpm exclusively."), "{text}");
    assert!(text.contains("- Mock API runs on port 8081."), "{text}");
}

// Verifies: `scope: global` lands in the separate global file, never
// mixed into the project's.
#[test]
fn global_scope_lands_in_the_separate_global_file() {
    let fs = FakeFs::new();
    let memory = ProjectMemory::new_trusted(&fs);
    memory
        .learn("Prefer tabs.", MemoryScope::Global)
        .expect("learn global");
    let global = read_memory(&fs, MemoryScope::Global).expect("read global");
    assert!(global.contains("- Prefer tabs."), "{global}");
    let project = read_memory(&fs, MemoryScope::Project).expect("read project");
    assert!(!project.contains("Prefer tabs."), "{project}");
}

// Verifies: the anti-poisoning guardrail — an untrusted workspace may
// learn project facts but never touch global memory, and the refusal
// names the reason.
#[test]
fn global_writes_refuse_in_untrusted_workspaces() {
    let fs = FakeFs::new();
    let memory = ProjectMemory::new_untrusted(&fs);
    memory
        .learn("Use pnpm.", MemoryScope::Project)
        .expect("project writes still work untrusted");
    let err = memory
        .learn(" Poison me. ", MemoryScope::Global)
        .expect_err("global refuses untrusted");
    assert!(err.contains("untrusted"), "{err}");
    assert!(
        read_memory(&fs, MemoryScope::Global)
            .expect("read")
            .is_empty()
            || !read_memory(&fs, MemoryScope::Global)
                .expect("read")
                .contains("Poison"),
        "nothing landed"
    );
}

// Verifies: empty facts refuse rather than appending blank bullets.
#[test]
fn empty_facts_refuse() {
    let fs = FakeFs::new();
    let memory = ProjectMemory::new_trusted(&fs);
    assert!(memory.learn("   ", MemoryScope::Project).is_err());
}

// Verifies: the session-start pin — the block freezes at first
// transform; a later `learn` updates the file for future sessions
// but does not move the active session's head (ADR-0017).
#[test]
fn the_snapshot_pins_at_first_transform() {
    let fs = FakeFs::new();
    let memory = ProjectMemory::new_trusted(&fs);
    memory
        .learn("Fact one.", MemoryScope::Project)
        .expect("learn");
    let conversation = || {
        vec![ChatMessage::text(
            MessageRole::System,
            "Be helpful.".to_string(),
        )]
    };
    let first = memory.transform(conversation());
    assert!(head(&first).contains("Fact one."), "{}", head(&first));
    memory
        .learn("Fact two.", MemoryScope::Project)
        .expect("learn");
    let second = memory.transform(conversation());
    assert!(
        !head(&second).contains("Fact two."),
        "pinned, not re-read:\n{}",
        head(&second)
    );
    // But the file itself carries both, for the next session.
    let text = read_memory(&fs, MemoryScope::Project).expect("read");
    assert!(text.contains("Fact two."), "{text}");
}

fn head(messages: &[ChatMessage]) -> String {
    messages[0]
        .content
        .iter()
        .filter_map(|block| match block {
            lca_protocol::ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// Verifies: the block carries the memory under its heading, ahead of
// the existing system text.
#[test]
fn the_block_injects_at_the_head_of_the_system_prompt() {
    let fs = FakeFs::new();
    let memory = ProjectMemory::new_trusted(&fs);
    memory
        .learn("Use pnpm.", MemoryScope::Project)
        .expect("learn");
    let out = memory.transform(vec![ChatMessage::text(
        MessageRole::System,
        "Be helpful.".to_string(),
    )]);
    assert_eq!(out.len(), 1);
    let text = head(&out);
    assert!(text.starts_with("# Project Memory"), "{text}");
    assert!(text.contains("Use pnpm."), "{text}");
    assert!(text.contains("Be helpful."), "{text}");
}

// Verifies: with no memory file the transform is the identity.
#[test]
fn no_memory_leaves_messages_untouched() {
    let fs = FakeFs::new();
    let memory = ProjectMemory::new_trusted(&fs);
    let before = vec![ChatMessage::text(MessageRole::User, "Hi.".to_string())];
    assert_eq!(memory.transform(before.clone()), before);
}

// Verifies: the manifest test contract — tool plus
// context-transform worlds, workspace and private read-write.
#[test]
fn manifest_grants_match_the_manifest() {
    let manifest = project_memory::MANIFEST;
    assert!(
        manifest.contains("worlds = [\"tool\", \"context-transform\"]"),
        "{manifest}"
    );
    let grants = project_memory::manifest_grants();
    assert_eq!(grants.fs.len(), 2, "workspace + private, both read-write");
    assert!(
        manifest.contains("workspace = \"read-write\""),
        "{manifest}"
    );
    assert!(manifest.contains("private = \"read-write\""), "{manifest}");
}

// Verifies: `run_learn` parses the tool arguments the model sends
// (fact required, scope defaulting to project).
#[test]
fn learn_arguments_default_scope_to_project() {
    assert_eq!(
        run_learn_arguments(r#"{"fact": "Use pnpm."}"#),
        ("Use pnpm.".to_string(), MemoryScope::Project)
    );
    assert_eq!(
        run_learn_arguments(r#"{"fact": "Tabs.", "scope": "global"}"#),
        ("Tabs.".to_string(), MemoryScope::Global)
    );
    assert!(run_learn_bad(r#"{"scope": "project"}"#));
    assert!(run_learn_bad(r#"{"fact": "X", "scope": "everywhere"}"#));
}

fn run_learn_arguments(json: &str) -> (String, MemoryScope) {
    project_memory::parse_learn_args(json).expect("parse")
}

fn run_learn_bad(json: &str) -> bool {
    project_memory::parse_learn_args(json).is_err()
}

#[test]
fn snapshot_block_respects_the_token_budget() {
    let big = "x".repeat(100_000);
    let block = snapshot_block(&big);
    assert!(
        block.len() <= 4 * project_memory::MEMORY_TOKEN_BUDGET + 64,
        "truncated near the budget, got {} chars",
        block.len()
    );
    assert!(block.contains("truncated"), "{block}");
}
