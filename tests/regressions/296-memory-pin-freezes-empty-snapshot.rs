//! GitHub #296: project memory's lazy pin never froze an *empty* snapshot,
//! so a mid-session `learn` mutated `messages[0]`.
//!
//! `pinned_snapshot` held `Mutex<Option<String>>`, which conflates "not yet
//! initialised" with "initialised but empty". A session starting with no
//! `MEMORY.md` stored `None`; a `learn` mid-session writes the file; the
//! next call saw `is_none()` and **re-read**, injecting a snapshot block
//! into `messages[0]` on turn 2+. That is a root-system mutation
//! mid-session — the same KV-cache bust class as gh #299, whose fix landed
//! the cycle before.
//!
//! The second half of the defect: the read was `MemoryScope::Project` only,
//! so facts learned with `scope: "global"` (the private `MEMORY.md`) were
//! never injected at all.
//!
//! The fix is the issue's own shape: a tri-state pin
//! (`Mutex<Option<Option<String>>>`, outer = frozen, inner = the snapshot, so
//! empty freezes as an initialised `None`) and a global+project combined
//! read through the existing `snapshot_block`.
//!
//! Verifies: the whole lifecycle through the real `ProjectMemory` handle —
//! empty start, mid-session learn, second turn byte-identical; and global
//! facts present in the snapshot.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::collections::HashMap;
use std::sync::Mutex;

use lca_protocol::{ChatMessage, ContentBlock, MessageRole};
use project_memory::{FsView, MemoryScope, ProjectMemory, read_memory};

/// An in-memory two-scope filesystem, so the lifecycle runs without a real
/// workspace (the extension's own test fake, inlined: the regressions crate
/// has no shared `common` module).
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

impl FsView for FakeFs {
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

fn head(messages: &[ChatMessage]) -> String {
    messages[0]
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn conversation() -> Vec<ChatMessage> {
    vec![
        ChatMessage::text(MessageRole::System, "You are LCA.".to_string()),
        ChatMessage::text(MessageRole::User, "hello".to_string()),
    ]
}

// Verifies: gh #296 acceptance 1+2+4 — a session starting with no memory
// keeps `messages[0]` byte-identical across a mid-session `learn`.
#[test]
fn an_empty_start_freezes_and_never_moves_the_head() {
    let fs = FakeFs::new();
    let memory = ProjectMemory::new_trusted(&fs);

    let before = conversation();
    let first = memory.transform(before.clone());
    assert_eq!(first, before, "no memory means no mutation on turn 1");

    memory
        .learn("A fact learned mid-session.", MemoryScope::Project)
        .expect("learn writes the file");

    let second = memory.transform(before.clone());
    assert_eq!(
        second, first,
        "the empty snapshot must have frozen: the head cannot move"
    );
    assert!(
        !head(&second).contains("A fact learned mid-session."),
        "the new fact belongs to the next session:\n{}",
        head(&second)
    );

    // It is on disk for that next session.
    let text = read_memory(&fs, MemoryScope::Project).expect("read");
    assert!(text.contains("A fact learned mid-session."), "{text}");
}

// Verifies: gh #296 acceptance 3 — global facts load alongside project
// facts in the one injected block.
#[test]
fn global_and_project_facts_share_the_snapshot() {
    let fs = FakeFs::new();
    let memory = ProjectMemory::new_trusted(&fs);
    memory
        .learn("Project rule.", MemoryScope::Project)
        .expect("learn project");
    memory
        .learn("Global preference.", MemoryScope::Global)
        .expect("learn global");

    let out = memory.transform(conversation());
    let text = head(&out);
    assert!(
        text.contains("Project rule."),
        "project fact missing:\n{text}"
    );
    assert!(
        text.contains("Global preference."),
        "global fact missing — the read ignored MemoryScope::Global:\n{text}"
    );
}

// Verifies: gh #296 acceptance 3 — a global-only memory still produces a
// snapshot; the pre-fix read of Project alone injected nothing.
#[test]
fn global_memory_alone_is_injected() {
    let fs = FakeFs::new();
    let memory = ProjectMemory::new_trusted(&fs);
    memory
        .learn("Only a global fact.", MemoryScope::Global)
        .expect("learn global");

    let out = memory.transform(conversation());
    assert!(
        head(&out).contains("Only a global fact."),
        "a global-only snapshot must inject:\n{}",
        head(&out)
    );
}

// Verifies: gh #296 acceptance 1+3 — the global half freezes too, so a
// mid-session global `learn` cannot reopen the bust through the new read.
#[test]
fn a_mid_session_global_learn_stays_out_of_the_head() {
    let fs = FakeFs::new();
    let memory = ProjectMemory::new_trusted(&fs);
    let before = conversation();
    let first = memory.transform(before.clone());
    memory
        .learn("Global late fact.", MemoryScope::Global)
        .expect("learn global");
    let second = memory.transform(before.clone());
    assert_eq!(second, first, "the combined read must freeze both scopes");
}

// Verifies: gh #296 (the non-empty half is unchanged) — a session that
// DOES start with memory still pins at first transform, and a later
// `learn` does not move the head.
#[test]
fn a_non_empty_start_still_pins_byte_identically() {
    let fs = FakeFs::new();
    let memory = ProjectMemory::new_trusted(&fs);
    memory
        .learn("Fact one.", MemoryScope::Project)
        .expect("learn");
    let first = memory.transform(conversation());
    assert!(head(&first).contains("Fact one."), "{}", head(&first));
    memory
        .learn("Fact two.", MemoryScope::Project)
        .expect("learn");
    let second = memory.transform(conversation());
    assert_eq!(second, first, "pinned, not re-read");
    assert!(!head(&second).contains("Fact two."), "{}", head(&second));
}
