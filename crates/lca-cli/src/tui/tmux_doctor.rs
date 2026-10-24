//! The tmux keyboard doctor (gh #400 gap 4): tmux's own defaults quietly break
//! modified keys, and nothing said so until a support thread.
//!
//! Outside tmux this costs nothing - `TMUX` unset returns [`Doctor::Silent`]
//! before a subprocess exists, which is most users. Inside tmux the probe is
//! **spawned, never awaited**: one `tmux show` measured 34 ms on this machine and
//! the load it runs beside is 200-300 ms, so a probe started while the terminal is
//! claimed (`startup::claim_terminal`, private) has finished long before the header
//! is composed. Nothing on the first-frame path waits for it.
//!
//! ## What is actually broken - measured, not copied
//!
//! pi warns about two things. Copying both would have been wrong here.
//!
//! `extended-keys` defaults to **`off`** in tmux 3.5, so a stock `~/.tmux.conf`
//! never reports `Ctrl+Shift+<key>` or `Shift+Enter` at all. That is the real
//! defect, and it is invisible from inside the app: the chord never arrives, so a
//! correctly-working input path looks broken.
//!
//! `extended-keys-format` is **not** a defect for LCA. pi prefers `csi-u`; LCA's
//! key parser reads both dialects - verified against tmux's own documented
//! encodings (`man tmux`, `extended-keys-format`) through
//! [`lca_tui::engine::keys::parse_key`], not inferred:
//!
//! | format  | `C-S-a` on the wire       | `parse_key`   |
//! |---------|---------------------------|---------------|
//! | `xterm` | `ESC [ 27 ; 6 ; 65 ~`     | `shift+ctrl+a`|
//! | `csi-u` | `ESC [ 65 ; 6 u`          | `shift+ctrl+a`|
//!
//! So advising `set -g extended-keys-format csi-u` as a *repair* would tell a
//! reader to change a setting that changes nothing for them - the stale-advice
//! failure this repository keeps closing. The format appears in the copy only as
//! **stability advice** (pin one encoding rather than inherit whatever tmux
//! defaults to next release), never as the thing that fixes a chord.
//!
//! ## Silence is the contract
//!
//! `extended-keys` already `on`/`always` -> nothing at all. A doctor that reports
//! health is a doctor people learn to ignore, and the notice has to earn its row
//! in the one moment it is right. A probe that finds no `tmux` binary, refuses, or
//! times out is equally silent: a startup announcing "could not check tmux" about
//! a *diagnostic* is worse than one that skips it.

use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// How long one `tmux show` may take before it is abandoned.
///
/// pi uses 2 s. This is a startup diagnostic that runs beside a longer load, so it
/// gets a fifth of that: a tmux server that cannot answer two `show` calls in
/// 400 ms has a bigger problem than the notice would report, and the answer is
/// silence either way.
pub const PROBE_BUDGET: Duration = Duration::from_millis(400);

/// The option whose `off` value is the defect.
const EXTENDED_KEYS: &str = "extended-keys";

/// The option that is advice, never a repair.
const EXTENDED_KEYS_FORMAT: &str = "extended-keys-format";

/// The doctor's finding for one launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Doctor {
    /// Nothing to say: outside tmux, healthy, or the probe failed. The three are
    /// deliberately indistinguishable - the contract is that a diagnostic stays
    /// quiet unless it has news, and "could not check" is not news.
    Silent,
    /// `extended-keys` is `off`, so modified keys never reach LCA.
    ExtendedKeysOff {
        /// The `extended-keys-format` tmux reported, when it answered. Used for
        /// stability advice only, never as the reason for the notice.
        format: Option<String>,
    },
}

/// The doctor's one notice, or `None` when it has nothing to say.
///
/// The copy names **the command, the file, the restart, and what it fixes** - a
/// reader who does not know what `extended-keys` means will not run a bare
/// `set -g` line. The restart sentence is not padding: `tmux source-file` reloads
/// bindings but *not* this option, and a reader who sourced their config would
/// otherwise conclude the advice does not work.
pub fn notice(doctor: &Doctor) -> Option<String> {
    match doctor {
        Doctor::Silent => None,
        Doctor::ExtendedKeysOff { format } => {
            let mut text = String::from(
                "tmux extended-keys is off, so Ctrl+Shift chords and Shift+Enter never \
                 reach lca. Add `set -g extended-keys on` to ~/.tmux.conf, then restart \
                 tmux - `source-file` reloads only part of the config.",
            );
            // Stability, not repair: LCA reads both formats, so this asks the
            // reader to pin an encoding rather than depend on a default that may
            // move. Naming their current format makes it advice instead of a
            // change they cannot evaluate.
            let format = format.as_deref().unwrap_or("");
            if !format.is_empty() && format != "csi-u" {
                text.push_str(&format!(
                    " Also pin `set -g extended-keys-format csi-u`: lca reads {format} \
                     too, so this is stability rather than a fix."
                ));
            }
            Some(text)
        }
    }
}

/// Decide from the two answers.
///
/// Pure, so the whole decision table is testable without a tmux server.
/// `keys == None` means the probe never answered - silent, because "could not
/// check" is not news. `always` is healthy: it reports modified keys whether or
/// not the application asked for them.
pub fn decide(keys: Option<&str>, format: Option<&str>) -> Doctor {
    match keys {
        None => Doctor::Silent,
        Some("on") | Some("always") => Doctor::Silent,
        // Any other answer means tmux knows the option and has it not-on. A tmux
        // that does not know the option at all exits non-zero, which `show`
        // reports as `None` - so this arm is a real `off`/`0`/whatever, never a
        // version mismatch.
        Some(_) => Doctor::ExtendedKeysOff {
            format: format.map(str::to_string),
        },
    }
}

/// Read one `tmux show -gv` option, bounded. `None` when tmux is absent, refuses,
/// or does not answer inside [`PROBE_BUDGET`].
///
/// `-v` prints the value alone, which is why the caller compares against `on`
/// rather than parsing `extended-keys on` out of a `name value` line.
fn show(option: &str, tmux: &str) -> Option<String> {
    let mut child = Command::new(tmux)
        .args(["show", "-gv", option])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    // The bounded-wait shape `terminal::tmux_forwards_hyperlinks` uses: the
    // deadline is enforced here rather than trusting the child, because a wedged
    // tmux server is exactly what this must not turn into a startup hang.
    let deadline = Instant::now() + PROBE_BUDGET;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(_) => return None,
        }
    }
    let out = child.wait_with_output().ok()?;
    if !out.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if value.is_empty() { None } else { Some(value) }
}

/// Run the doctor for real: `TMUX` unset is [`Doctor::Silent`] with **no
/// subprocess at all**.
pub fn examine() -> Doctor {
    examine_with(&|key| std::env::var(key).ok(), "tmux")
}

/// [`examine`] over an injected environment reader and binary name, so tests pin
/// the ladder without touching process env or needing a live tmux server.
///
/// The env reader is a closure rather than a map because the real one is
/// `std::env::var`; a test passes a `HashSet` lookup and the two paths share
/// every line below the check.
pub fn examine_with(env: &dyn Fn(&str) -> Option<String>, tmux: &str) -> Doctor {
    if env("TMUX").is_none_or(|value: String| value.is_empty()) {
        return Doctor::Silent;
    }
    let keys = show(EXTENDED_KEYS, tmux);
    let format = show(EXTENDED_KEYS_FORMAT, tmux);
    decide(keys.as_deref(), format.as_deref())
}

/// The launch's doctor cell: filled by a spawned thread while the startup load
/// runs, read when the header is composed.
///
/// `OnceLock` rather than a channel because there is exactly one finding per
/// launch and the reader never blocks - the same shape
/// `crate::update::spawn` uses for its background check.
pub type Cell = std::sync::Arc<OnceLock<Doctor>>;

/// Start the probe on its own thread and return the cell it will fill.
///
/// Spawned before the load, so the ~68 ms of two `tmux show` calls happens while
/// the registry is being assembled rather than after the first frame. A plain
/// thread rather than `tokio::spawn`: the work is `std::process`, not async I/O,
/// and it has to run on the path that exists before the runtime is built.
///
/// A thread that cannot be spawned leaves the cell empty, which reads as silence -
/// the only failure mode of `Builder::spawn`, and it degrades to "no notice",
/// which is the correct answer.
pub fn spawn() -> Cell {
    let cell: Cell = std::sync::Arc::new(OnceLock::new());
    let fill = std::sync::Arc::clone(&cell);
    let _ = std::thread::Builder::new()
        .name("lca-tmux-doctor".into())
        .spawn(move || {
            let _ = fill.set(examine());
        });
    cell
}

/// The notice from a cell filled by [`spawn`]: `None` while the probe is still
/// running, and `None` forever when it found nothing to say.
///
/// A still-running probe is silent rather than blocking, which is the whole point
/// of the spawn: the worst case is that a slow tmux loses its notice, never that
/// it delays the first frame.
pub fn notice_from(cell: &Cell) -> Option<String> {
    cell.get().and_then(notice)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::collections::HashMap;

    /// An env reader over a fixed map - `lca-cli`'s tests never set process env
    /// (they run in parallel), so the ladder takes the reader as an argument.
    ///
    /// Unix only because every caller is: the rows that use it need `stub_tmux`.
    #[cfg(unix)]
    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        move |key: &str| map.get(key).cloned()
    }

    /// A stub binary that answers like `tmux show -gv <option>`.
    ///
    /// Written to a scratch dir rather than mocking `Command`, so the bound, the
    /// exit-status check and the trimming are all exercised as they really run.
    ///
    /// Unix only: the fixture is a `#!/bin/sh` script, and Windows `CreateProcess`
    /// cannot execute one - it fails with "not a valid application", which made the
    /// first version of these rows red on `windows-latest` while green here. The
    /// *decisions* are platform-free and covered everywhere by `the_decision_table_*`;
    /// what is Unix-only is the process mechanics (spawn, exit status, kill), and
    /// half-covering those on Windows with a `.cmd` shim would test the shim.
    #[cfg(unix)]
    fn stub_tmux(script: &str, name: &str) -> std::path::PathBuf {
        use std::io::Write as _;
        let dir = std::env::temp_dir().join(format!("lca-doctor-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let path = dir.join("tmux");
        let mut file = std::fs::File::create(&path).expect("stub file");
        file.write_all(script.as_bytes()).expect("write stub");
        drop(file);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                .expect("chmod stub");
        }
        path
    }

    // Verifies: gh #400 gap 4 - **outside tmux the probe never runs.** The rule
    // is an assertion on the call count, not a timing one: a probe that fires and
    // discards its answer is still a subprocess per launch for most users.
    #[test]
    #[cfg(unix)] // `stub_tmux` is a shell script; see its doc.
    fn outside_tmux_no_probe_runs_at_all() {
        // A stub that records every invocation by appending to a file. If the
        // doctor spawns it, the file exists.
        let marker = std::env::temp_dir().join(format!("lca-doctor-called-{}", std::process::id()));
        let _ = std::fs::remove_file(&marker);
        let stub = stub_tmux(
            &format!(
                "#!/bin/sh\necho called >> '{}'\necho off\nexit 0\n",
                marker.display()
            ),
            "noprope",
        );
        let doctor = examine_with(&env_of(&[]), stub.to_str().expect("utf8 path"));
        assert_eq!(doctor, Doctor::Silent, "unset TMUX is silent");
        assert!(
            !marker.exists(),
            "and it never spawned tmux: a probe that runs and discards its answer is still a \
             subprocess on every launch outside tmux"
        );

        // An empty TMUX (`TMUX=` in a wrapper script) is the same case.
        let empty = examine_with(&env_of(&[("TMUX", "")]), stub.to_str().expect("utf8 path"));
        assert_eq!(empty, Doctor::Silent, "an empty TMUX is not inside tmux");
        assert!(!marker.exists(), "and still no subprocess");
    }

    // Verifies: gh #400 gap 4 - **the off case is reported with the command and
    // the reason.** A reader who does not know what `extended-keys` means will not
    // run a bare `set -g` line, so the copy has to survive being read cold.
    #[test]
    #[cfg(unix)] // `stub_tmux` is a shell script; see its doc.
    fn extended_keys_off_names_the_command_the_file_and_the_reason() {
        let stub = stub_tmux("#!/bin/sh\necho off\nexit 0\n", "off");
        let doctor = examine_with(
            &env_of(&[("TMUX", "/tmp/tmux-1000/default,1,0")]),
            stub.to_str().expect("utf8 path"),
        );
        assert_eq!(
            doctor,
            Doctor::ExtendedKeysOff {
                format: Some("off".into())
            },
            "the stub answers `off` for both options"
        );
        let text = notice(&doctor).expect("a notice");
        assert!(
            text.contains("set -g extended-keys on"),
            "the exact command: {text}"
        );
        assert!(text.contains("~/.tmux.conf"), "the file: {text}");
        assert!(
            text.contains("Ctrl+Shift") && text.contains("Shift+Enter"),
            "what it fixes, in the user's words: {text}"
        );
        assert!(
            text.contains("restart"),
            "and that a restart is needed - `source-file` does not cover this option: {text}"
        );
    }

    // Verifies: gh #400 gap 4 - **a healthy setup produces nothing at all.** The
    // doctor has to earn its row; reporting health is what teaches people to
    // ignore it.
    #[test]
    #[cfg(unix)] // `stub_tmux` is a shell script; see its doc.
    fn a_healthy_tmux_says_nothing() {
        for keys in ["on", "always"] {
            let stub = stub_tmux(
                &format!("#!/bin/sh\necho {keys}\n"),
                &format!("healthy-{keys}"),
            );
            let doctor = examine_with(
                &env_of(&[("TMUX", "/tmp/tmux-1000/default,1,0")]),
                stub.to_str().expect("utf8 path"),
            );
            assert_eq!(doctor, Doctor::Silent, "`{keys}` is healthy");
            assert_eq!(notice(&doctor), None, "and silent");
        }
    }

    // Verifies: gh #400 gap 4 - **the format is advice, not a repair.** This is
    // the row that keeps LCA from copying pi's warning: pi needs `csi-u`, LCA
    // reads both dialects, so telling a reader their `xterm` format is broken
    // would be false. Asserted on the wording, because the defect being guarded
    // against is a *false claim*, not a missing one.
    #[test]
    fn the_xterm_format_is_named_as_stability_not_as_the_fault() {
        let doctor = decide(Some("off"), Some("xterm"));
        let text = notice(&doctor).expect("a notice");
        assert!(
            text.contains("stability"),
            "the format is offered as stability: {text}"
        );
        assert!(
            text.contains("xterm") && text.contains("lca reads"),
            "and it says so while naming the format the user has: {text}"
        );
        // The load-bearing assertion: with extended-keys already on, an `xterm`
        // format is **not** reported. That is the difference from pi.
        assert_eq!(
            decide(Some("on"), Some("xterm")),
            Doctor::Silent,
            "LCA parses xterm-format chords, so this is not a defect here"
        );
    }

    // Verifies: gh #400 gap 4 - **a failed probe is silent, and bounded.** A
    // missing binary, a refusal, and a hang all have to leave the launch
    // unaffected; "could not check tmux" is not news worth a row.
    #[test]
    #[cfg(unix)] // `stub_tmux` is a shell script; see its doc.
    fn a_failed_or_hanging_probe_is_silent_and_cannot_stall_the_launch() {
        let inside = env_of(&[("TMUX", "/tmp/tmux-1000/default,1,0")]);

        // No such binary.
        assert_eq!(
            examine_with(&inside, "/nonexistent/tmux-for-lca-test"),
            Doctor::Silent,
            "a missing tmux is silent"
        );

        // A refusal (non-zero exit) is silent, not an error.
        let refusing = stub_tmux("#!/bin/sh\necho no server running 1>&2\nexit 1\n", "refuse");
        assert_eq!(
            examine_with(&inside, refusing.to_str().expect("utf8 path")),
            Doctor::Silent,
            "a tmux that refuses is silent"
        );

        // **The bounded call.** A tmux that never answers must return, which is
        // the whole reason the deadline exists. `PROBE_BUDGET` is the measurement:
        // the assertion is that two hanging probes finish inside a small multiple
        // of it rather than hanging the launch forever.
        let hanging = stub_tmux("#!/bin/sh\nsleep 30\n", "hang");
        let started = Instant::now();
        let doctor = examine_with(&inside, hanging.to_str().expect("utf8 path"));
        let elapsed = started.elapsed();
        assert_eq!(doctor, Doctor::Silent, "a hang is silent");
        assert!(
            elapsed < PROBE_BUDGET * 3,
            "two bounded probes cost about two budgets, not the sleep: took {elapsed:?}"
        );
    }

    // Verifies: gh #400 gap 4 - **`decide` is the whole decision table.** Six
    // inputs, three outcomes, no tmux server: the healthy/silent pair that a
    // naive `!= "on"` check gets wrong is `always`, which tmux really does report.
    #[test]
    fn the_decision_table_is_exhaustive() {
        assert_eq!(decide(None, None), Doctor::Silent);
        assert_eq!(decide(Some("on"), Some("csi-u")), Doctor::Silent);
        assert_eq!(decide(Some("always"), Some("xterm")), Doctor::Silent);
        assert_eq!(
            decide(Some("off"), None),
            Doctor::ExtendedKeysOff { format: None }
        );
        assert_eq!(
            decide(Some("off"), Some("csi-u")),
            Doctor::ExtendedKeysOff {
                format: Some("csi-u".into())
            }
        );
        // A value tmux does not define today still means "not on", and saying so
        // is better than silently treating it as healthy.
        assert_eq!(
            decide(Some("0"), Some("xterm")),
            Doctor::ExtendedKeysOff {
                format: Some("xterm".into())
            }
        );
    }

    // Verifies: gh #400 gap 4 - **the launch path spawns, never waits.** Reading a
    // cell before the thread has filled it is silent rather than blocking, which
    // is what keeps the first frame free of the probe's 68 ms.
    #[test]
    fn an_unfilled_cell_reads_as_silence_rather_than_blocking() {
        let cell: Cell = std::sync::Arc::new(OnceLock::new());
        assert_eq!(notice_from(&cell), None, "nothing filled yet is silent");
        let _ = cell.set(Doctor::Silent);
        assert_eq!(notice_from(&cell), None, "and a healthy finding is too");
        let cell: Cell = std::sync::Arc::new(OnceLock::new());
        let _ = cell.set(Doctor::ExtendedKeysOff { format: None });
        assert!(
            notice_from(&cell).is_some(),
            "a finding is reported once filled"
        );
    }

    // Verifies: gh #400 gap 4 - **the notice is two scannable rows**, not one long
    // line: the diagnosis and the advice are separate sentences and a reader
    // skims for the second.
    #[test]
    fn the_notice_is_split_at_its_sentence_boundary() {
        let doctor = decide(Some("off"), Some("xterm"));
        let text = notice(&doctor).expect("a notice");
        let rows = crate::tui::startup::wrap_notice(&text);
        assert!(
            rows.len() >= 2,
            "diagnosis and advice break apart: {rows:?}"
        );
        assert!(
            rows[0].contains("extended-keys is off"),
            "the diagnosis leads: {rows:?}"
        );
        assert!(
            rows.iter()
                .any(|row| row.contains("set -g extended-keys on")),
            "the command is somewhere in the rows: {rows:?}"
        );
    }
}
