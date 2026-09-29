//! Released defect (Windows, the 2026-09-25 ConPTY quarantine): the `pty`
//! capability's `CreateProcessW` passed `PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE`
//! the *address* of a local `HPCON` instead of the HPCON value itself, and
//! left the child's std handles unmarked. `UpdateProcThreadAttribute` still
//! returned success, so the child was born on a fresh default console and the
//! pty pipe received zero bytes — the whole Windows pty trio and the ConPTY
//! real-terminal pair sat `#[ignore]`d. Microsoft's sample and wezterm's
//! `procthreadattr::set_pty` pass the value; `STARTF_USESTDHANDLES` with
//! invalid handles keeps a redirected-stdio parent from leaking its own
//! handles into the child.
//!
//! This guards the user-visible half: a program spawned under a pty has its
//! terminal output reach the reader. Against the old code it reads zero bytes
//! and fails on the deadline.
//!
//! Verifies: ADR-0016, docs/platform-notes.md (the Windows pty path).

use std::time::{Duration, Instant};

#[test]
fn a_pty_childrens_terminal_output_reaches_the_reader() {
    let dir = lca_testkit::scratch_path("regression-conpty-attach");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("mkdir");
    let token = "conpty-attached";
    let (program, args): (&str, Vec<String>) = if cfg!(windows) {
        (
            "cmd",
            vec!["/C".to_string(), "echo".to_string(), token.to_string()],
        )
    } else {
        ("echo", vec![token.to_string()])
    };
    let mut pty = lca_tools::PtyChild::spawn(program, &args, &dir, 24, 80, &[]).expect("spawn");
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut out = String::new();
    while !out.contains(token) && Instant::now() < deadline {
        match pty.read(4096) {
            Ok(Some(chunk)) if !chunk.is_empty() => out.push_str(&String::from_utf8_lossy(&chunk)),
            Ok(Some(_)) | Ok(None) => std::thread::sleep(Duration::from_millis(2)),
            Err(_) => break,
        }
    }
    pty.kill();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        out.contains(token),
        "the pty reader never saw the child's output: {out:?}"
    );
}
