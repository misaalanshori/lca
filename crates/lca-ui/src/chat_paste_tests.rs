//! The paste matrix (R1/R6): every text surface takes a bracketed paste the way
//! the editor does, and the masked secret field masks it instead of dropping it.
//!
//! Split out of `chat_tests.rs` under gate 11's 1,200-line ceiling (#575b, the
//! piece gh #574 left behind). The shared fixtures moved first, which is free
//! (no reported test name mentions them); this is the one subject left in the
//! file whose rows share nothing but those fixtures, so it moved whole and no
//! row body changed.
//!
//! **Path change, deliberate:** `chat_tests.rs` is a declared test module
//! (`chat.rs` holds `#[path = "chat_tests.rs"] pub(crate) mod tests`), so a row
//! cannot leave it without gaining a segment; these seven report as
//! `chat::paste_tests::<row>` where they were `chat::tests::<row>`. `include!`
//! would have kept the names and hidden the rows from `module_sources`, which is
//! the failure mode gh #574 was filed about. The report carries both the raw
//! diff and the normalised one.

use super::tests_support::{chat, strip};
use super::*;

#[test]
fn a_large_bracketed_paste_becomes_a_marker() {
    let mut chat = chat();
    let big: String = (0..12)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    chat.handle_key(&format!("\x1b[200~{big}\x1b[201~"));
    assert_eq!(chat.editor.text(), "[paste #1 +12 lines]");
}

// ---------------------------------------------------------------------------
// R1/R6: the paste matrix. Every text surface takes a bracketed paste the
// way the editor does; the masked field masks it.
// ---------------------------------------------------------------------------

/// Wrap text as a real bracketed-paste event (what the engine emits).
fn bracketed(text: &str) -> String {
    format!("\x1b[200~{text}\x1b[201~")
}

// Verifies: R1/R6 - a paste into the masked secret field lands in the
// buffer and is masked in the frame; the secret bytes never appear.
#[test]
fn paste_into_the_masked_secret_field_is_masked() {
    let mut chat = chat();
    chat.apply_login_next(LoginNext::Secret {
        provider: "p".into(),
        label: "API key (input hidden)".into(),
        masked: true,
    });
    let secret = "sk-super-secret-value";
    chat.handle_key(&bracketed(secret));
    assert_eq!(chat.world.secret.as_ref().expect("prompt").input, secret);
    let text = strip(&chat.viewport(80, 24, 0)).join("\n");
    assert!(
        !text.contains(secret),
        "the secret never reaches the frame:\n{text}"
    );
    assert!(
        text.contains(&"*".repeat(secret.chars().count())),
        "the paste is masked, not dropped:\n{text}"
    );
}

// Verifies: R1 - a multi-line paste into a single-line field is flattened,
// not dropped and not broken across lines (pi's `input.ts` contract).
#[test]
fn paste_into_a_base_url_field_is_flattened() {
    let mut chat = chat();
    chat.apply_login_next(LoginNext::Secret {
        provider: "p".into(),
        label: "Base URL".into(),
        masked: false,
    });
    chat.handle_key(&bracketed("https://api.example.com/v1\r\nnext\ttab"));
    assert_eq!(
        chat.world.secret.as_ref().expect("prompt").input,
        "https://api.example.com/v1next    tab"
    );
}

// Verifies: R1 - the tmux CSI-u paste dialect reaches the same buffer,
// decoded.
#[test]
fn paste_into_the_secret_field_decodes_the_csi_u_dialect() {
    let mut chat = chat();
    chat.apply_login_next(LoginNext::Secret {
        provider: "p".into(),
        label: "API key (input hidden)".into(),
        masked: true,
    });
    chat.handle_key(&bracketed("sk-a\x1b[106;5ub"));
    assert_eq!(chat.world.secret.as_ref().expect("prompt").input, "sk-ab");
}

// Verifies: R1 - the model picker's search box takes a paste.
#[test]
fn paste_into_the_model_picker_search_filters_the_list() {
    let mut chat = chat();
    chat.model_picker = Some(ModelPicker::new(vec![
        crate::ModelRow::from(("alpha-1".into(), "alpha-1".into())),
        crate::ModelRow::from(("beta-2".into(), "beta-2".into())),
    ]));
    chat.handle_key(&bracketed("beta"));
    let picker = chat.model_picker.as_ref().expect("picker");
    assert_eq!(picker.query, "beta");
    assert_eq!(picker.matches, vec![1]);
}

// Verifies: R1 - the resume picker's search box takes a paste.
#[test]
fn paste_into_the_resume_picker_search_filters_the_list() {
    let mut chat = chat();
    chat.resume_picker = Some(crate::resume::ResumePicker::new(vec![
        crate::resume::SessionEntry {
            id: "a".into(),
            title: "alpha".into(),
            messages: 1,
            age: "now".into(),
            labels: Vec::new(),
            modified_ms: 0,
            parent: None,
        },
        crate::resume::SessionEntry {
            id: "b".into(),
            title: "beta".into(),
            messages: 2,
            age: "now".into(),
            labels: Vec::new(),
            modified_ms: 0,
            parent: None,
        },
    ]));
    chat.handle_key(&bracketed("beta"));
    let picker = chat.resume_picker.as_ref().expect("picker");
    assert_eq!(picker.query, "beta");
    assert_eq!(picker.matches, vec![1]);
}

// Verifies: R1 - the transcript search box takes a paste.
#[test]
fn paste_into_the_transcript_search_takes_the_query() {
    let mut chat = chat();
    chat.search = Some(String::new());
    chat.handle_key(&bracketed("the answer"));
    assert_eq!(chat.search.as_deref(), Some("the answer"));
}
