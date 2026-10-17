//! GitHub #263: `expand_at_files` truncated `@file` content with
//! `String::truncate(INPUT_MAX_BYTES)` and no char-boundary check, so a
//! file whose 1,048,576th byte lands inside a multi-byte UTF-8 sequence
//! (CJK, emoji, accented) panicked with
//! `byte index 1048576 is not a char boundary` and killed the CLI.
//!
//! Verifies: end to end through the real `lca -p` binary — a 1 MiB+ file
//! straddling the bound expands and runs instead of panicking (a Rust
//! panic exits 101; the expected clean outcome is a provider error, exit
//! 3, against a closed loopback port so no real network is touched), and
//! nothing on either stream says "panicked".

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

// The CLI round trip is Unix-gated like its siblings (the harness spawns
// a POSIX shell). The cut arithmetic itself is pinned portably by
// `crates/lca-cli`'s `at_file_truncation_never_splits_a_character`.
#[cfg(unix)]
mod unix {
    use std::path::{Path, PathBuf};
    use std::process::Command;

    const LIMIT: usize = 1024 * 1024;

    /// The built `lca` binary, or `None` when this run did not build one
    /// (the harness-absence rule, testing plan §14).
    fn binary() -> Option<PathBuf> {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        ["target/debug/lca", "target/release/lca"]
            .iter()
            .map(|relative| manifest.join(relative))
            .find(|path| path.is_file())
    }

    /// Write a file whose byte `LIMIT` falls inside a 3-byte CJK char.
    fn write_straddling(path: &Path) {
        let mut content = "a".repeat(LIMIT - 1);
        content.push_str("世界");
        std::fs::write(path, &content).expect("write fixture");
        assert!(std::fs::metadata(path).expect("meta").len() > LIMIT as u64);
    }

    // Verifies: gh #263 AC1 - a straddling `@file` never panics the CLI.
    #[test]
    fn a_straddling_at_file_never_panics_the_cli() {
        let Some(binary) = binary() else {
            eprintln!("skip: the lca binary is not built in this run (harness-absence rule)");
            return;
        };
        let root = lca_testkit::scratch_path("gh263-at-file-bin");
        let _ = std::fs::remove_dir_all(&root);
        let home = root.join("home");
        let project = root.join("project");
        std::fs::create_dir_all(&home).expect("mkdir home");
        std::fs::create_dir_all(&project).expect("mkdir project");
        write_straddling(&project.join("big.txt"));

        let out = Command::new(&binary)
            .args([
                // `@big.txt` must be its own argv token: `split_at_files`
                // recognises a whole token, not an `@path` prefix.
                "-p",
                "hi",
                "@big.txt",
                "--allow-host",
                "127.0.0.1",
            ])
            .current_dir(&project)
            .env("HOME", &home)
            .env("USERPROFILE", &home)
            .env("LCA_UPDATE_CHECK", "false")
            // A closed loopback port: the run gets past expansion and dies
            // on connect, so no real network is touched.
            .env("OPENAI_API_KEY", "dummy-not-a-real-key")
            .env("OPENAI_BASE_URL", "http://127.0.0.1:1/v1")
            .output()
            .expect("run lca");
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        let code = out.status.code().unwrap_or(-1);

        // 101 is Rust's panic exit; the bug produced exactly that.
        assert_ne!(code, 101, "the CLI must not panic: {stderr}");
        assert!(
            !stderr.contains("panicked") && !stderr.contains("not a char boundary"),
            "no char-boundary panic: {stderr}"
        );
        // The run got past expansion and failed on the (closed) transport.
        assert!(
            stderr.contains("openai-compatible") || stderr.contains("connect"),
            "the run passed @file expansion and failed at the provider: {stderr}"
        );
        let _ = stdout;
        let _ = std::fs::remove_dir_all(&root);
    }
}
