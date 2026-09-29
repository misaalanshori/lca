//! Windows display defect (found in the ConPTY drive, 2026-09-29): the
//! session-start notice rendered the canonical `\\?\C:\...` form, because
//! the record stores the canonicalized working directory and the notice
//! formatted it directly.
//!
//! The fix is a display-only helper (`lca_ui::display_path`). The canonical
//! form stays load-bearing everywhere else: the `fs` scope-escape check and
//! process spawn compare against it, so stripping must not leak into those.
//!
//! Verifies: docs/platform-notes.md (Windows path semantics);
//! docs/conpty-testing-plan.md (the `\\?\` display item).

use lca_ui::display_path;

#[test]
fn verbatim_paths_are_stripped_for_display_only() {
    assert_eq!(
        display_path(r"\\?\C:\Users\me\my project"),
        r"C:\Users\me\my project"
    );
    // A non-ASCII segment survives untouched (only the prefix is stripped).
    assert_eq!(
        display_path(r"\\?\C:\Users\me\projet-été"),
        r"C:\Users\me\projet-été"
    );
    assert_eq!(display_path(r"\\?\UNC\server\share"), r"\\server\share");
    // Already-plain and non-Windows paths are untouched.
    assert_eq!(display_path(r"C:\plain"), r"C:\plain");
    assert_eq!(display_path("/home/me/project"), "/home/me/project");
}
