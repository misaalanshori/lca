//! The `lca doctor` diagnostics dump (gh #81, pi's diagnostics row):
//! version, paths, extensions, config, sessions, crashes. Reads only;
//! every absent file is a row, never an error. `report` is the
//! testable core (it reads the filesystem but changes nothing);
//! `run` prints it.

use std::path::{Path, PathBuf};

/// Build the dump for one data directory and project.
pub fn report(data_dir: &Path, cwd: &Path) -> String {
    let mut out = String::new();
    out.push_str(&crate::version_text());
    out.push('\n');
    row(&mut out, "data dir", &data_dir.display().to_string());
    let log = crate::diagnostics::log_dir(data_dir).join("lca.log");
    row(&mut out, "log file", &sized(&log));
    // gh #258: the two debug files (absent until `--debug` runs once).
    let logs = crate::diagnostics::log_dir(data_dir);
    row(&mut out, "debug log", &sized(&logs.join("debug.log")));
    row(
        &mut out,
        "debug wire log",
        &sized(&logs.join("debug-wire.log")),
    );
    row(
        &mut out,
        "user config",
        &present(&data_dir.join("config.toml")),
    );
    // gh #481: from `$HOME` the project path and the user path are one file, so
    // reporting it twice as two layers said "present" for a project layer that
    // does not exist. Say so instead, and keep both rows so the paths stay
    // visible.
    let project_state =
        if crate::project_ext::project_file_is_the_user_file(cwd, data_dir, "config.toml") {
            "same file as the user config (no separate project layer)".to_string()
        } else {
            present(&cwd.join(".lca").join("config.toml"))
        };
    row(&mut out, "project config", &project_state);
    let ext_root = data_dir.join("extensions");
    let installed = if !ext_root.is_dir() {
        "none".to_string()
    } else {
        match lca_registry::InstallTree::new(&ext_root).list() {
            Ok(entries) => {
                let mut names: Vec<String> = entries.into_iter().map(|(name, _)| name).collect();
                names.sort();
                format!("{} ({})", names.len(), names.join(", "))
            }
            Err(err) => format!("unreadable ({err})"),
        }
    };
    row(&mut out, "installed extensions", &installed);
    row(
        &mut out,
        "built-in extensions",
        &crate::ext::builtin_names().join(", "),
    );
    let sessions = match lca_session::SessionStore::new(data_dir.to_path_buf()).list_sessions(cwd) {
        Ok(list) => list.len().to_string(),
        Err(err) => format!("unreadable ({err})"),
    };
    row(&mut out, "sessions here", &sessions);
    row(&mut out, "crash reports", &crashes(data_dir));
    out
}

/// Print the dump; the command's whole job.
pub fn run(data_dir: &Path, cwd: &Path) -> i32 {
    print!("{}", report(data_dir, cwd));
    crate::exit::OK
}

fn row(out: &mut String, key: &str, value: &str) {
    out.push_str(key);
    out.push_str(": ");
    out.push_str(value);
    out.push('\n');
}

fn present(path: &Path) -> String {
    if path.is_file() {
        "present".to_string()
    } else {
        "absent".to_string()
    }
}

fn sized(path: &PathBuf) -> String {
    match std::fs::metadata(path) {
        Ok(meta) => format!("{} ({} bytes)", path.display(), meta.len()),
        Err(_) => format!("{} (absent)", path.display()),
    }
}

/// Crash files, newest last: the count plus the newest name, so a
/// report pasted into an issue says whether a crash is on disk.
fn crashes(data_dir: &Path) -> String {
    let mut logs: Vec<String> = std::fs::read_dir(data_dir)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|entry| {
                    let name = entry.file_name().into_string().ok()?;
                    (name.starts_with("crash-") && name.ends_with(".log")).then_some(name)
                })
                .collect()
        })
        .unwrap_or_default();
    logs.sort();
    match logs.last() {
        Some(newest) => format!("{} (newest {newest})", logs.len()),
        None => "none".to_string(),
    }
}

#[cfg(test)]
mod tests {
    // Verifies: gh #481 - **the dump does not invent a project layer.** From
    // `$HOME` the project path and the user path are one file; reporting it as
    // "present" twice named a layer that does not exist. Both rows stay, and the
    // project row says what is actually true.
    #[test]
    fn the_dump_does_not_report_a_project_layer_that_is_the_user_file() {
        let root = lca_testkit::scratch_path("lca-doctor-481");
        let _ = std::fs::remove_dir_all(&root);
        let home = root.join("home");
        let data = home.join(".lca");
        std::fs::create_dir_all(&data).expect("mkdir");
        std::fs::write(
            data.join("config.toml"),
            "provider = \"openai-compatible\"\n",
        )
        .expect("write");
        // `cwd` is the home itself: the coincidence.
        let text = super::report(&data, &home);
        assert!(
            text.contains("user config: present"),
            "the user file is there: {text}"
        );
        assert!(
            text.contains("project config: same file as the user config"),
            "and the project row says there is no separate layer: {text}"
        );
        // A genuinely different project still names its own file.
        let project = root.join("checkout");
        std::fs::create_dir_all(project.join(".lca")).expect("mkdir");
        std::fs::write(project.join(".lca").join("config.toml"), "").expect("write");
        let text = super::report(&data, &project);
        assert!(
            text.contains("project config: present"),
            "a real project file is reported as itself: {text}"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    // Verifies: gh #81 (the dump prints: version rows plus one row per
    // section, absent files as rows rather than errors).
    #[test]
    fn the_dump_names_every_section() {
        let root = lca_testkit::scratch_path("lca-doctor");
        let _ = std::fs::remove_dir_all(&root);
        let data = root.join("data");
        let project = root.join("project");
        std::fs::create_dir_all(&data).expect("mkdir");
        std::fs::create_dir_all(&project).expect("mkdir");
        std::fs::write(data.join("crash-1-2.log"), "lca crash report\n").expect("crash");
        let text = super::report(&data, &project);
        for row in [
            "abi ",
            "crate ",
            "target ",
            "data dir:",
            "log file:",
            "user config: absent",
            "project config: absent",
            "installed extensions:",
            "built-in extensions:",
            "sessions here:",
            "crash reports: 1",
        ] {
            assert!(text.contains(row), "the dump names {row}: {text}");
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
