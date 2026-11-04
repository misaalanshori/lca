//! The `/` completion list and the guards that keep it honest (gh #270).
//!
//! Ceiling split from `commands.rs` (gate 11): the ordered list is a decision, and
//! the guards that read it are the reason it can be trusted, so the two live
//! together rather than growing the file they describe.

/// The completion list's fixed half, in the order `/` shows it (gh #270).
///
/// # Why a literal and not a build-up
///
/// This list used to be `BUILTIN_SLOTS` plus a chain of `names.insert(n, ..)`
/// calls at hand-maintained indices, and the indices **collided**:
/// `insert(7, "/clone")` was followed by `insert(7, "/thinking")`, which shifted
/// `/clone` to 8, and then `insert(8, "/resume")` shifted it again. The result
/// was a scramble no reader could predict from the source - and eight routed
/// commands (`/label`, `/labels`, `/jump`, `/rename`, `/persona`, `/ext`,
/// `/marketplace`, `/quit`) were never inserted at all, so typing `/` could not
/// reach them.
///
/// A positional build-up cannot be reviewed; an ordered literal can be read top
/// to bottom and checked against what the menu shows. So: one list, and the
/// order is a decision recorded here rather than an artefact of arithmetic.
///
/// # The order, and why it is that
///
/// pi's grouping, interface outward (`packages/coding-agent/src/core/
/// slash-commands.ts`): what you read first is what you are most likely to need
/// before you have typed anything, and what you reach for rarely sits further
/// down. Concretely:
///
/// 1. **Answering the interface** - `/help` first, because a user who does not
///    know what to type types `/` and looks. Then the things that change how the
///    interface itself looks and thinks.
/// 2. **The session** - where you are and how to move: rename it, branch it,
///    bookmark it, jump, resume, or go back to the tree.
/// 3. **Its content** - compact it, write it out, read one in.
/// 4. **Model and configuration** - what answers, and under what policy.
/// 5. **Integrations** - MCP servers, extensions, and reloading discovery.
/// 6. **Provider identity** - sign in, sign out, usage.
/// 7. **Attachments** - an image for the next message.
/// 8. **Diagnostics** - `/bug`, which is where a user goes when something broke.
/// 9. **Leaving** - `/exit` and its `/quit` spelling last, so a mistyped Enter
///    does not land on them.
///
/// Aliases sit beside the canonical spelling they alias (`/labels` next to
/// `/label`, `/rename` next to `/name`, `/quit` next to `/exit`,
/// `/marketplace` next to `/extensions`) so completion offers them as a pair
/// and a reader sees the relationship. `/skill` is the umbrella; one row per
/// installed skill is appended at runtime, because only that part is dynamic.
///
/// **Every name here is routed.** The guard in this module's tests reads the
/// router's arms and the host's dispatch arms out of the source and fails if
/// either answers a name this list omits - that direction is the defect this
/// constant fixes, and it is the one #443's guard did not cover.
pub(crate) const COMPLETION_NAMES: &[&str] = &[
    // 1. Answering the interface.
    "help",
    "hotkeys",
    "fullscreen",
    "theme",
    "thinking",
    // 2. The session: where it is, how to move.
    "session",
    "tree",
    "fork",
    "clone",
    "resume",
    "label",
    "labels",
    "jump",
    "name",
    "rename",
    // 3. Its content.
    "compact",
    "export",
    "import",
    // 4. Model and configuration.
    "model",
    "scoped-models",
    "settings",
    "persona",
    "permissions",
    "grants",
    "trust",
    // 5. Integrations.
    "mcp",
    "extensions",
    "marketplace",
    "ext",
    "reload",
    "changelog",
    // 6. Provider identity.
    "login",
    "logout",
    "usage",
    // 7. Attachments.
    "attach",
    "paste",
    // 8. Diagnostics.
    "bug",
    // 9. Leaving.
    "exit",
    "quit",
    // 10. The skill umbrella; per-skill rows are appended at runtime.
    "skill",
];

/// Every slash name the host's [`Ui::dispatch_command`] answers itself,
/// without the extension registry. One list, three readers: the router's
/// allowlist (`slash_commands`), `/help`'s description table (which must
/// carry a line for each), and the guard that keeps the two honest
/// (gh #443).
///
/// **Gh #443: this is the fix for the class, not one instance.** The
/// router (`Chat::dispatch_command`) admits a name only when it is in
/// `slash_commands`, so a handler that is not registered is dead code
/// that `/help` still advertises - `/bug`, `/export`, and `/import` were
/// exactly that, answering "unknown command" while their implementations
/// sat unreachable. Listing them here registers them and keeps the guard
/// (in this module's tests) able to prove no future handler is orphaned.
///
/// The list is deliberately *not* the same as `slash_commands()`: that
/// one also carries extension-registered commands and `/skill:<name>`
/// instances, which no static table can enumerate.
///
/// **Gh #270:** this is no longer the registration mechanism - `COMPLETION_NAMES`
/// is, because `slash_commands()` is built from it. What survives here is the
/// *specification* of which commands the host answers itself, which is the half
/// gh #443's guards and `tests/regressions/443-*` read: "every name the host
/// dispatches is registered" and "every name registered has a dispatch arm" are
/// still the two statements that keep a handler from becoming dead code. It is
/// test-only now so it cannot drift back into being a second completion list.
#[cfg(test)]
pub(crate) const HANDLED_NAMES: &[&str] = &[
    "attach",
    "bug",
    "compact",
    "export",
    "import",
    "login",
    "logout",
    "model",
    "paste",
    "permissions",
    "session",
    "settings",
    "skill",
    "usage",
];

#[cfg(test)]
#[path = "commands_registration_tests.rs"]
mod registration_tests;

/// The built-in slash slots the interface itself claims; the spec's
/// sixth built-in, `/stats`, arrives from the native hooks extension
/// that holds the stats source (ADR-0013).
///
/// Gh #270: this is no longer the completion list's source - `COMPLETION_NAMES`
/// is, because a positional build-up over this array is what scrambled the menu.
/// The array stays as the **spec's** named set (the SRDD claims these slots), and
/// its only remaining reader is the row that asserts they are claimed, so it is
/// test-only rather than deleted: dropping it would drop the claim, not just the
/// implementation.
#[cfg(test)]
pub(crate) const BUILTIN_SLOTS: [&str; 8] = [
    "login", "logout", "usage", "model", "compact", "attach", "paste", "session",
];
