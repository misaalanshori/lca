//! GitHub #81: an induced panic writes `crash-*.log` under the data
//! directory with the version, the loaded extensions, and the faulting
//! context; the file is the whole point, the terminal restore is gh #96's.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use std::sync::{Mutex, OnceLock};

fn recorder() -> &'static Mutex<Vec<u8>> {
    static RECORDER: OnceLock<Mutex<Vec<u8>>> = OnceLock::new();
    RECORDER.get_or_init(|| Mutex::new(Vec::new()))
}

fn record(bytes: &[u8]) {
    recorder()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .extend_from_slice(bytes);
}

// Verifies: gh #81 (an induced panic writes the crash file with its
// rows: version, extensions, message, and frames).
#[test]
fn gh81_induced_panic_writes_the_crash_file_with_its_rows() {
    let dir = lca_testkit::scratch_path("lca-crash-log");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("mkdir");
    lca_tui::set_crash_context(lca_tui::CrashContext {
        version: "9.9.9-test".to_string(),
        data_dir: dir.clone(),
        extensions: vec!["probe-one".to_string(), "probe-two".to_string()],
    });
    lca_tui::install_panic_hook_with(record);
    let _ = std::panic::catch_unwind(|| panic!("gh81 synthetic crash"));
    let mut hits = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("crash dir") {
        let path = entry.expect("entry").path();
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("crash-") && name.ends_with(".log"))
        {
            hits.push(std::fs::read_to_string(&path).expect("read"));
        }
    }
    assert!(!hits.is_empty(), "the induced panic wrote a crash file");
    let ours = hits
        .iter()
        .find(|text| text.contains("gh81 synthetic crash"))
        .expect("our panic's file");
    for row in [
        "9.9.9-test",
        "probe-one",
        "probe-two",
        "gh81 synthetic crash",
    ] {
        assert!(ours.contains(row), "the crash file names {row}: {ours}");
    }
}
