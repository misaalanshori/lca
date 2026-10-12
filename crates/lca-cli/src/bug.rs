//! `/bug` and `lca bug`: the local diagnostic bundle (gh #57, pi's
//! bug-report without the upload hosts). Offline by construction:
//! everything zipped already exists on disk. The transcript rides
//! along (the same trust as `lca export`); credentials never do (the
//! credential store is not read, only its presence row in the doctor
//! dump). Split from `session_cmds.rs`' neighborhood for the line
//! ceiling; the command glue stays there.

use std::io::Write;
use std::path::{Path, PathBuf};

/// Build the bundle: `report.txt` (the doctor dump), `session.jsonl`
/// (the session's records, one per line), and `meta.json` (version,
/// platform, timestamp). Returns the zip path. Nothing leaves the
/// machine; sharing the file is the user's explicit next step.
pub fn bundle(
    data_dir: &Path,
    cwd: &Path,
    session_id: Option<&str>,
    out_dir: &Path,
) -> Result<PathBuf, String> {
    let store = lca_session::SessionStore::new(data_dir.to_path_buf());
    let session = match session_id {
        Some(id) => store
            .session(cwd, id)
            .map_err(|err| format!("cannot open the session: {err}"))?,
        None => latest_session(&store, cwd)?,
    };
    let records = store
        .read_with(&session, lca_session::ViewMode::Audit)
        .map_err(|err| format!("cannot read the session: {err}"))?
        .records;
    let mut jsonl = String::new();
    for record in &records {
        let line = serde_json::to_string(record)
            .map_err(|err| format!("cannot serialize the session: {err}"))?;
        jsonl.push_str(&line);
        jsonl.push('\n');
    }
    let meta = serde_json::json!({
        "lca": crate::version_text(),
        "platform": std::env::consts::OS,
        "created_ms": lca_session::now_ms(),
        "session": session.id(),
    });
    let files: Vec<(&str, Vec<u8>)> = vec![
        (
            "report.txt",
            crate::doctor::report(data_dir, cwd).into_bytes(),
        ),
        ("session.jsonl", jsonl.into_bytes()),
        (
            "meta.json",
            format!(
                "{}\n",
                serde_json::to_string_pretty(&meta).unwrap_or_default()
            )
            .into_bytes(),
        ),
    ];
    let stamp = session.id();
    let out = out_dir.join(format!("lca-bug-{stamp}.zip"));
    let file =
        std::fs::File::create(&out).map_err(|err| format!("cannot write the bundle: {err}"))?;
    let mut writer = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default();
    for (name, bytes) in &files {
        writer
            .start_file(*name, options)
            .map_err(|err| format!("cannot zip {name}: {err}"))?;
        writer
            .write_all(bytes)
            .map_err(|err| format!("cannot zip {name}: {err}"))?;
    }
    writer
        .finish()
        .map_err(|err| format!("cannot finish the bundle: {err}"))?;
    Ok(out)
}

/// The most recently modified session here, for a session-less `/bug`.
fn latest_session(
    store: &lca_session::SessionStore,
    cwd: &Path,
) -> Result<lca_session::Session, String> {
    let mut sessions = store
        .list_sessions(cwd)
        .map_err(|err| format!("cannot list sessions: {err}"))?;
    sessions.sort_by_key(|summary| summary.modified_ms);
    let id = sessions
        .last()
        .map(|summary| summary.id.clone())
        .ok_or_else(|| "no sessions here".to_string())?;
    store
        .session(cwd, &id)
        .map_err(|err| format!("cannot open the session: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Verifies: gh #57 - the bundle zips exactly the diagnostic
    // files (report, transcript lines, meta) and nothing else; no
    // credential material rides along.
    #[test]
    fn the_bundle_holds_report_transcript_and_meta_only() {
        let root = lca_testkit::scratch_path("lca-bug");
        let data = root.join("data");
        let project = root.join("project");
        std::fs::create_dir_all(&project).expect("mkdir");
        let store = lca_session::SessionStore::new(data.clone());
        let session = store.create_session(&project, "buggy").expect("session");
        store
            .append(
                &session,
                lca_protocol::Record::User {
                    v: 1,
                    ts: 2,
                    id: "u1".to_string(),
                    parent: None,
                    content: "it broke".to_string(),
                    attachments: Vec::new(),
                    queue: None,
                },
            )
            .expect("append");
        // A decoy credential file: presence is fine, contents must
        // never enter the bundle.
        std::fs::write(data.join("credentials.toml"), "token = \"sk-decoy\"").expect("decoy");

        let out = bundle(&data, &project, Some(session.id()), &root).expect("bundle");
        let bytes = std::fs::read(&out).expect("read");
        let text = String::from_utf8_lossy(&bytes);
        assert!(
            !text.contains("sk-decoy"),
            "credential contents never enter the bundle"
        );
        let file = std::fs::File::open(&out).expect("open");
        let mut archive = zip::ZipArchive::new(file).expect("zip");
        let mut names = Vec::new();
        for index in 0..archive.len() {
            names.push(archive.by_index(index).expect("entry").name().to_string());
        }
        names.sort();
        assert_eq!(names, vec!["meta.json", "report.txt", "session.jsonl"]);
        let mut jsonl = String::new();
        std::io::Read::read_to_string(
            &mut archive.by_name("session.jsonl").expect("lines"),
            &mut jsonl,
        )
        .expect("read lines");
        assert!(jsonl.contains("it broke"), "the transcript rides along");
    }
}
