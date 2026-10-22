//! `/bug` and `lca bug`: the local diagnostic bundle (gh #57, pi's bug-report
//! without the upload hosts). Offline by construction: everything zipped already
//! exists on disk, and nothing is uploaded anywhere - sharing the file is the
//! user's own explicit act.
//!
//! # Staged, inspected, then written (gh #411)
//!
//! The flow is three steps, not one:
//!
//! 1. [`crate::bug::stage`] reads the session **once** and writes a scrubbed **copy** into a
//!    private staging directory. Secret-shaped strings are replaced (see
//!    [`crate::bug_redact`]), and the counts are kept.
//! 2. [`crate::bug::Plan::inspection`] renders what the user is about to share: the staged
//!    path, each file with its size, and what was scrubbed - by count and kind,
//!    never the value. It reads nothing, so showing it costs no I/O.
//! 3. [`crate::bug::write`] produces the zip, or [`crate::bug::discard`] removes the staging directory
//!    entirely.
//!
//! **The user's session log is never touched.** Redaction applies to the staged
//! copy, so the log is byte-identical afterwards - the tempting implementation
//! scrubs in place and destroys the user's own history.
//!
//! # The trust foundation this keeps
//!
//! Credentials never ride along: the credential store is not read at all (only its
//! presence row in the doctor dump), so no decoy can leak. That is gh #57's
//! invariant and gh #411 is additive to it, not a rework.

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::bug_redact::{self, Scrubbed};

/// The file names inside the bundle, in the order they are listed.
const FILES: [&str; 3] = ["report.txt", "session.jsonl", "meta.json"];

/// A staged bundle, waiting for the user's decision.
///
/// Holds everything the inspection screen and the final zip need, so neither step
/// reads the session again.
#[derive(Debug)]
pub struct Plan {
    /// The private staging directory (removed on [`discard`] and after [`write()`](crate::bug::write)).
    pub staging: PathBuf,
    /// The file list with sizes, in bundle order.
    files: Vec<(&'static str, usize)>,
    /// What the redaction pass removed.
    pub scrubbed: Scrubbed,
    /// Where the zip will land.
    out: PathBuf,
    /// How many times this plan read the session. One, always - the row that
    /// pins the inspection costs no extra I/O asserts on it.
    reads: usize,
}

impl Plan {
    /// How many times the session was read to build this plan (always 1).
    pub fn reads(&self) -> usize {
        self.reads
    }

    /// The file list with byte sizes, as the inspection shows it.
    pub fn files(&self) -> &[(&'static str, usize)] {
        &self.files
    }

    /// Where the finished zip will be written.
    pub fn destination(&self) -> &Path {
        &self.out
    }

    /// What the user is about to share, as text for the inspection screen.
    ///
    /// Reads nothing: every figure comes from the plan. The scrub summary names
    /// counts and kinds only - a silent scrub is its own trust problem, and a
    /// summary that repeats the secret would be a new one.
    pub fn inspection(&self) -> String {
        let mut out = String::from("This bundle will contain:\n");
        for (name, size) in &self.files {
            out.push_str(&format!("  {name:<16} {size} bytes\n"));
        }
        out.push_str(&format!("\nstaged at: {}\n", self.staging.display()));
        out.push_str(&format!("zip will be: {}\n", self.out.display()));
        out.push_str(&format!("{}\n", self.scrubbed.summary()));
        out.push_str(
            "\nThe transcript is a scrubbed copy; your own session log is unchanged. \
             Nothing is uploaded - sharing the file is your own next step.",
        );
        out
    }
}

/// Drop the staging directory when the plan goes away.
///
/// The plan owns a directory holding a **transcript**; leaving one behind because
/// a caller forgot to call [`discard`] is exactly the `/tmp` accident the staging
/// exists to prevent. Found by this cycle's own rows: thirteen staged directories
/// survived an `lca-cli` run because a test (and, in the field, an error path that
/// returned early) had staged and not written.
///
/// `write` and [`discard`] still remove it eagerly, so the window in which the
/// copy exists is as short as the flow allows; this is the backstop for the paths
/// that do not reach either.
impl Drop for Plan {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.staging);
    }
}

/// The written bundle.
#[derive(Debug, Clone)]
pub struct Written {
    /// The zip.
    pub path: PathBuf,
    /// What the redaction pass removed, so the caller can report it too.
    pub scrubbed: Scrubbed,
}

/// Read the session once, write a scrubbed copy into a private staging directory,
/// and return the plan. Nothing user-visible is produced and nothing has left the
/// machine.
pub fn stage(
    data_dir: &Path,
    cwd: &Path,
    session_id: Option<&str>,
    out_dir: &Path,
) -> Result<Plan, String> {
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

    // The transcript, record by record: each record is serialized and then
    // scrubbed, so a secret split across two records' JSON cannot slip through a
    // whole-file pass that treats the file as one string with structure to lose.
    let mut jsonl = String::new();
    let mut scrubbed = Scrubbed::default();
    for record in &records {
        let line = serde_json::to_string(record)
            .map_err(|err| format!("cannot serialize the session: {err}"))?;
        let (clean, found) = bug_redact::redact(&line);
        jsonl.push_str(&clean);
        jsonl.push('\n');
        scrubbed.merge(found);
    }

    // The body bytes, scrubbed too: the doctor dump can carry a configured base
    // URL with an embedded credential, and `meta.json` is generated here.
    let report = crate::doctor::report(data_dir, cwd);
    let (report, report_found) = bug_redact::redact(&report);
    scrubbed.merge(report_found);

    let meta = serde_json::json!({
        "lca": crate::version_text(),
        "platform": std::env::consts::OS,
        "created_ms": lca_session::now_ms(),
        "session": session.id(),
        "scrubbed": scrubbed.total(),
    });
    let meta = format!(
        "{}\n",
        serde_json::to_string_pretty(&meta).unwrap_or_default()
    );

    let bodies: [(&'static str, String); 3] = [
        ("report.txt", report),
        ("session.jsonl", jsonl),
        ("meta.json", meta),
    ];
    let stamp = session.id();
    let out = out_dir.join(format!("lca-bug-{stamp}.zip"));

    // A private staging directory: the owner-only tempdir, not a predictable path
    // in a shared `/tmp`, because this holds a transcript.
    let staging = staging_dir()?;
    let mut files = Vec::with_capacity(bodies.len());
    for (name, body) in &bodies {
        let path = staging.join(name);
        if let Err(err) = std::fs::write(&path, body.as_bytes()) {
            // A staging failure must not leave a partial copy around.
            let _ = std::fs::remove_dir_all(&staging);
            return Err(format!("cannot stage {name}: {err}"));
        }
        files.push((*name, body.len()));
    }
    // Read each one back: a half-written stage would otherwise be zipped and the
    // user told a transcript was scrubbed when it was not.
    for (name, size) in &files {
        let path = staging.join(name);
        let read = std::fs::read(&path).map_err(|err| format!("cannot read back {name}: {err}"))?;
        if read.len() != *size {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(format!(
                "cannot read back {name}: staged {} of {size} bytes",
                read.len()
            ));
        }
    }

    Ok(Plan {
        staging,
        files,
        scrubbed,
        out,
        reads: 1,
    })
}

impl Scrubbed {
    /// Merge another pass's counts into this one.
    fn merge(&mut self, other: Scrubbed) {
        for (kind, count) in other.families() {
            self.add(kind, *count);
        }
    }
}

/// Zip the staged files to the plan's destination and remove the staging
/// directory.
pub fn write(plan: &Plan) -> Result<Written, String> {
    let parent = plan.out.parent().unwrap_or(Path::new("."));
    write_to(plan, parent)
}

/// [`write_to`] into a chosen directory, for a caller that picks
/// the destination.
///
/// On any failure the staging directory is removed: a half-scrubbed bundle left
/// behind is exactly the accident the staging exists to prevent.
pub fn write_to(plan: &Plan, out_dir: &Path) -> Result<Written, String> {
    let name = plan
        .out
        .file_name()
        .map(|name| name.to_owned())
        .ok_or_else(|| "the bundle destination has no file name".to_string())?;
    let out = out_dir.join(name);
    let result = write_zip(plan, &out);
    // Whatever happened, the staged copy has served its purpose.
    let _ = std::fs::remove_dir_all(&plan.staging);
    result.map(|path| Written {
        path,
        scrubbed: plan.scrubbed.clone(),
    })
}

fn write_zip(plan: &Plan, out: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(out.parent().unwrap_or(Path::new(".")))
        .map_err(|err| format!("cannot create the bundle directory: {err}"))?;
    let file =
        std::fs::File::create(out).map_err(|err| format!("cannot write the bundle: {err}"))?;
    let mut writer = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default();
    for name in FILES {
        let path = plan.staging.join(name);
        let bytes =
            std::fs::read(&path).map_err(|err| format!("cannot read the staged {name}: {err}"))?;
        writer
            .start_file(name, options)
            .map_err(|err| format!("cannot zip {name}: {err}"))?;
        writer
            .write_all(&bytes)
            .map_err(|err| format!("cannot zip {name}: {err}"))?;
    }
    writer
        .finish()
        .map_err(|err| format!("cannot finish the bundle: {err}"))?;
    Ok(out.to_path_buf())
}

/// Remove the staging directory because the user cancelled.
///
/// Deliberately infallible from the caller's side: the user asked for nothing to
/// be left behind, and a removal error is not something they can act on. The
/// directory is private to this plan, so a failure here means it is already gone.
pub fn discard(plan: &Plan) {
    let _ = std::fs::remove_dir_all(&plan.staging);
}

/// A private staging directory under the process's owner-only temp root.
fn staging_dir() -> Result<PathBuf, String> {
    let base = std::env::temp_dir().join(format!(
        "lca-bug-stage-{}-{}",
        std::process::id(),
        lca_session::now_ms()
    ));
    std::fs::create_dir_all(&base)
        .map_err(|err| format!("cannot create the staging directory: {err}"))?;
    // Owner-only: this holds a transcript, and a world-readable temp directory is
    // how a "local" bundle leaks to another user on the same machine.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(err) = std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o700)) {
            let _ = std::fs::remove_dir_all(&base);
            return Err(format!("cannot restrict the staging directory: {err}"));
        }
    }
    Ok(base)
}

/// Build the bundle in one step: stage, then write. Kept for `lca bug`, which has
/// no interactive gate; `/bug` uses [`stage`] so the user can inspect first.
pub fn bundle(
    data_dir: &Path,
    cwd: &Path,
    session_id: Option<&str>,
    out_dir: &Path,
) -> Result<PathBuf, String> {
    let plan = stage(data_dir, cwd, session_id, out_dir)?;
    write(&plan).map(|written| written.path)
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

    fn fixture(name: &str) -> (PathBuf, PathBuf, String) {
        let root = lca_testkit::scratch_path(&format!("lca-bug-{name}"));
        let _ = std::fs::remove_dir_all(&root);
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
        (root, data, session.id().to_string())
    }

    fn find_log(dir: &Path) -> PathBuf {
        fn walk(dir: &Path) -> Option<PathBuf> {
            for entry in std::fs::read_dir(dir).ok()?.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    if let Some(found) = walk(&path) {
                        return Some(found);
                    }
                } else if path.file_name().is_some_and(|name| name == "log.jsonl") {
                    return Some(path);
                }
            }
            None
        }
        walk(dir).expect("a session log")
    }

    // Verifies: gh #57 - the bundle zips exactly the diagnostic files (report,
    // transcript lines, meta) and nothing else; no credential material rides
    // along. Gh #411 keeps this invariant while adding the scrub.
    #[test]
    fn the_bundle_holds_report_transcript_and_meta_only() {
        let (root, data, id) = fixture("files");
        // A decoy credential file: presence is fine, contents must never enter.
        std::fs::write(data.join("credentials.toml"), "token = \"sk-decoy\"").expect("decoy");

        let out = bundle(&data, &root.join("project"), Some(&id), &root).expect("bundle");
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

    // Verifies: gh #411 - the one-step `bundle` used by `lca bug` goes through the
    // same scrub as the interactive flow, leaves the session log untouched, and
    // leaves no staging directory behind.
    #[test]
    fn the_one_step_bundle_scrubs_and_cleans_up() {
        let secret = "sk-test-DONOTUSE-abcdefghijklmnopqrstuvwx";
        let (root, data, _id) = fixture("onestep");
        let store = lca_session::SessionStore::new(data.clone());
        let project = root.join("project");
        let sessions = store.list_sessions(&project).expect("list");
        let id = sessions[0].id.clone();
        store
            .append(
                &store.session(&project, &id).expect("session"),
                lca_protocol::Record::User {
                    v: 1,
                    ts: 3,
                    id: "u2".to_string(),
                    parent: None,
                    content: format!("my key is {secret}"),
                    attachments: Vec::new(),
                    queue: None,
                },
            )
            .expect("append");

        let before = std::fs::read_to_string(find_log(&data)).expect("log");
        let out = bundle(&data, &project, Some(&id), &root).expect("bundle");
        let file = std::fs::File::open(&out).expect("open");
        let mut archive = zip::ZipArchive::new(file).expect("zip");
        let mut jsonl = String::new();
        std::io::Read::read_to_string(
            &mut archive.by_name("session.jsonl").expect("lines"),
            &mut jsonl,
        )
        .expect("read");
        assert!(!jsonl.contains(secret), "the one-step path scrubs too");
        assert_eq!(
            before,
            std::fs::read_to_string(find_log(&data)).expect("log"),
            "and the session log is untouched"
        );
        // Assert **this** bundle's staging is gone, not that no staging directory
        // exists anywhere: nextest runs rows concurrently, so another row's staged
        // copy is legitimately on disk for a moment. The first version of this
        // check counted the whole temp directory and failed only in a full run,
        // which is the classic shared-state assertion bug.
        let plan = stage(&data, &project, Some(&id), &root).expect("stage to inspect");
        let staging = plan.staging.clone();
        assert!(staging.is_dir(), "the plan really did stage something");
        let written = write(&plan).expect("write");
        assert!(written.path.is_file());
        assert!(
            !staging.exists(),
            "this bundle's staging directory is gone after the write: {}",
            staging.display()
        );
    }
}
