//! Path resolution and content addressing, split from `lib.rs` for the
//! workspace file ceiling (gate 11). Behaviour unchanged.

use std::path::{Component, Path, PathBuf};

use unicode_normalization::UnicodeNormalization;

/// Resolve `path` against `cwd`, canonicalizing the existing prefix
/// and normalizing the rest (unresolvable tails stay lexical).
pub fn resolve_target(cwd: &Path, path: &Path) -> PathBuf {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    let mut existing = joined.clone();
    let mut remainder: Vec<std::ffi::OsString> = Vec::new();
    loop {
        if let Ok(canonical) = std::fs::canonicalize(&existing) {
            let mut out = canonical;
            for part in remainder.iter().rev() {
                out.push(part);
            }
            return normalize(&out);
        }
        match (existing.file_name(), existing.parent()) {
            (Some(name), Some(parent)) => {
                remainder.push(name.to_os_string());
                existing = parent.to_path_buf();
            }
            _ => return normalize(&joined),
        }
    }
}

fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

// ---------------------------------------------------------------------
// gh #405 gap 1: macOS screenshot path variants.
// ---------------------------------------------------------------------

/// The on-disk names a localised `~/Desktop` can have (gh #405).
///
/// macOS localises the Finder *display* name but the directory keeps the local
/// language's spelling. `Desktop` is first so an English install needs no search.
const DESKTOP_DIRS: &[&str] = &["Desktop", "Bureau", "Schreibtisch", "Escritorio"];

/// Where newer macOS puts screenshots (gh #405 design point 4).
///
/// A **fallback**, searched only after every Desktop spelling: a Desktop file wins
/// whenever both could match, so enabling the fallback never changes which of two
/// real files a user gets.
const PICTURES_DIRS: &[&str] = &["Pictures", "Images", "Bilder", "Imágenes"];

/// The filename shapes macOS has used for screenshots and recordings (gh #405).
///
/// Enumerated rather than derived from one template: a `replace`/`strip_prefix` on
/// `"Screen Shot {date} at {time}"` cannot produce `Screenshot {date} {time}`, and
/// one moved space is exactly how this class of bug arrives.
///
/// The `stem` is `"Screen Shot"`, `"Screenshot"`, `"Screen Recording"`, or
/// `"Capture d'écran"`; each is combined with the two date/time joins.
const SHOT_STEMS: &[&str] = &[
    "Screen Shot",
    "Screenshot",
    "Screen Recording",
    "ScreenRecording",
    "Capture d'écran",
];

/// Every filename the given `stem`/`date`/`time`/`ext` could plausibly have.
///
/// Public for the row that asserts the set is enumerated rather than derived.
#[must_use]
pub fn shot_name_candidates(stem: &str, date: &str, time: &str, ext: &str) -> Vec<String> {
    vec![
        format!("{stem} {date} at {time}.{ext}"),
        format!("{stem} {date} {time}.{ext}"),
        // macOS also writes the time with a narrow no-break space (U+202F) before
        // AM/PM, and some releases omit the extension separator difference.
        format!("{stem} {date} at {time}\u{202f}.{ext}"),
    ]
}

/// The full candidate filename set for one date/time pair across every known
/// screenshot stem (gh #405).
#[must_use]
pub fn screenshot_candidates(date: &str, time: &str, ext: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for stem in SHOT_STEMS {
        names.extend(shot_name_candidates(stem, date, time, ext));
    }
    names
}

/// A failed screenshot resolution: the paths that were tried, in order.
///
/// Carrying the list rather than a bare "not found" is design point 3: a hint that
/// points at a file which is not there is worse than an honest miss, so the error
/// shows the search instead of guessing a winner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenshotMiss {
    tried: Vec<PathBuf>,
}

impl ScreenshotMiss {
    /// Every candidate location tried, in search order.
    #[must_use]
    pub fn tried(&self) -> &[PathBuf] {
        &self.tried
    }
}

impl std::fmt::Display for ScreenshotMiss {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "not found; tried:")?;
        for path in &self.tried {
            write!(f, "\n  {}", path.display())?;
        }
        Ok(())
    }
}

/// Resolves a user-typed macOS screenshot path across the known name and folder
/// variants (gh #405 gap 1).
///
/// Constructed with a home directory so the rows can drive it against a scratch
/// tree; production passes the real home.
///
/// The lookup is **shape-driven**: a path whose filename matches none of the
/// screenshot shapes resolves to itself when it exists and to `None` otherwise, so
/// ordinary paths are never reinterpreted (that would turn a typo into a wrong-file
/// read, which is worse than the not-found it replaces).
#[derive(Debug, Clone)]
pub struct ScreenshotLookup {
    home: PathBuf,
}

impl ScreenshotLookup {
    /// A lookup rooted at `home`.
    #[must_use]
    pub fn new(home: &Path) -> Self {
        Self {
            home: home.to_path_buf(),
        }
    }

    /// The variants to try for `path`, in search order.
    ///
    /// `None` when the filename matches no screenshot shape, which is the signal
    /// that this path is not ours to interpret.
    fn variants(&self, path: &Path) -> Option<Vec<PathBuf>> {
        // A filename is required for the shape test; the stem carries the match.
        path.file_name()?.to_str()?;
        let ext = path.extension()?.to_str()?;
        let stem = path.file_stem()?.to_str()?;
        // gh #405: normalise the *typed* name to NFC before parsing it, so the
        // comparison against the enumerated candidates is spelling-insensitive. A
        // macOS disk hands back NFD while a paste is usually NFC (and a French
        // `Capture d'écran` is the common case), so without this the search works
        // in one direction only.
        let stem_nfc: String = stem.nfc().collect();
        let known = SHOT_STEMS
            .iter()
            .find(|candidate| stem_nfc.starts_with(**candidate))?;
        let rest = stem_nfc.strip_prefix(known)?.trim_start().to_string();
        let (date, time) = match rest.split_once(" at ") {
            Some((date, time)) => (date, time),
            None => rest.rsplit_once(' ')?,
        };
        // Normalise the time's separator so U+202F and ASCII space both work.
        let time = time.replace('\u{202f}', " ");
        let names = shot_name_candidates(known, date.trim(), time.trim(), ext);

        let mut out = Vec::new();
        for dir in [DESKTOP_DIRS, PICTURES_DIRS].concat() {
            for name in &names {
                out.push(self.home.join(dir).join(name));
                // gh #405: macOS writes NFD on disk while a paste is often NFC, so
                // a name with an accent needs both spellings tried. NFC of an
                // already-ASCII name is identical, so this doubles the list only
                // where it can matter.
                let nfc: String = name.nfc().collect();
                let nfd: String = name.nfd().collect();
                if nfd != *name {
                    out.push(self.home.join(dir).join(&nfd));
                }
                if nfc != *name && nfc != nfd {
                    out.push(self.home.join(dir).join(&nfc));
                }
            }
        }
        Some(out)
    }

    /// Resolve `path` to an existing file, or `None`.
    ///
    /// An existing path resolves to itself first, so a screenshot the user typed
    /// exactly right costs one `is_file` and no search.
    #[must_use]
    pub fn resolve(&self, path: &Path) -> Option<PathBuf> {
        if path.is_file() {
            return Some(path.to_path_buf());
        }
        let variants = self.variants(path)?;
        variants.into_iter().find(|candidate| candidate.is_file())
    }

    /// Resolve, or report every location tried (gh #405 design point 3).
    ///
    /// # Errors
    /// The [`ScreenshotMiss`] listing the candidates, when none exists.
    pub fn resolve_detailed(&self, path: &Path) -> Result<PathBuf, ScreenshotMiss> {
        if path.is_file() {
            return Ok(path.to_path_buf());
        }
        let variants = self.variants(path).unwrap_or_default();
        if let Some(hit) = variants.iter().find(|candidate| candidate.is_file()) {
            return Ok(hit.clone());
        }
        Err(ScreenshotMiss { tried: variants })
    }
}

/// The screenshot resolver rooted at the process's home (gh #405).
///
/// A free function rather than a field on the executor, so no construction site
/// changes for a read-path detail.
#[must_use]
pub fn home_screenshot_lookup() -> ScreenshotLookup {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    ScreenshotLookup::new(&home)
}

/// Resolve `target` by shape when nothing is at it (gh #405 gap 1).
///
/// Returns the path to read plus the miss, when the screenshot search ran and found
/// nothing - so the caller can report the candidates beside its own error.
#[must_use]
pub fn screenshot_or_literal(target: PathBuf) -> (PathBuf, Option<ScreenshotMiss>) {
    if target.is_file() {
        return (target, None);
    }
    match home_screenshot_lookup().resolve_detailed(&target) {
        Ok(found) => (found, None),
        Err(miss) => (target, Some(miss)),
    }
}

/// Whether a resolved path sits inside the workspace root.
pub fn is_inside(path: &Path, root: &Path) -> bool {
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    path.starts_with(&root)
}

/// Instruction filenames, startup's rule (gh #219 mirrors it per
/// directory): an `AGENTS.override.md` replaces its siblings.
const INSTRUCTION_NAMES: &[&str] = &["AGENTS.md", "AGENTS.MD", "CLAUDE.md", "CLAUDE.MD"];

/// Walk from `target` up to (excluding) the workspace `root`,
/// collecting instruction files not yet in `loaded`, nearest first
/// (gh #219, OpenCode's read-time discovery shape). For a file the
/// walk starts at its parent; for a directory at itself. Targets
/// outside the root yield nothing: untrusted directories never
/// inject (the trust model holds).
pub fn discover_instructions(
    target: &Path,
    root: &Path,
    loaded: &std::collections::HashSet<PathBuf>,
) -> Vec<PathBuf> {
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    if !target.starts_with(&root) {
        return Vec::new();
    }
    let mut dir = if target.is_dir() {
        target.to_path_buf()
    } else {
        target
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| target.to_path_buf())
    };
    let mut found = Vec::new();
    while dir.starts_with(&root) && dir != root {
        if let Some(hit) = first_present(&dir)
            && !loaded.contains(&hit)
        {
            found.push(hit);
        }
        match dir.parent() {
            Some(parent) => dir = parent.to_path_buf(),
            None => break,
        }
    }
    found
}

/// The instruction file one directory contributes, if any.
fn first_present(dir: &Path) -> Option<PathBuf> {
    let over = dir.join("AGENTS.override.md");
    if over.is_file() {
        return Some(over);
    }
    INSTRUCTION_NAMES
        .iter()
        .map(|name| dir.join(name))
        .find(|path| path.is_file())
}

/// Lowercase hex SHA-256 of `bytes`: the attachment content address. Public
/// so the core's attach path can address a file by the same function the
/// spill path uses (one hash, no second implementation).
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

#[cfg(test)]
#[path = "paths/screenshot_tests.rs"]
mod screenshot_tests;
