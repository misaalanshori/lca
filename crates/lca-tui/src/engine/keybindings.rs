//! Semantic keybinding registry, ported from pi's
//! `packages/tui/src/keybindings.ts`
//! (`pi-tui-re/src_re/tui-engine/keybindings.md`).
//!
//! Actions are names, not keys. Multiple default keys per action give the
//! readline-plus-arrows flavor without platform forks; a user override map
//! can rebind or explicitly unbind (`[]`).

use std::collections::{BTreeMap, BTreeSet};

use super::keys::{is_valid_key_id, matches_key_with_kitty};

/// One action's default bindings and description.
#[derive(Debug, Clone, Copy)]
pub struct KeybindingDefinition {
    /// Default keys; an empty slice means unbound by default.
    pub default_keys: &'static [&'static str],
    /// Human-readable description.
    pub description: &'static str,
}

macro_rules! def {
    ($keys:expr, $desc:expr) => {
        KeybindingDefinition {
            default_keys: $keys,
            description: $desc,
        }
    };
}

/// The **default** key for an action, ignoring any user override.
///
/// Named for what it is. It used to be called `key_text` and documented as a
/// hint that "cannot lie when the interface is rebound" - while building a fresh
/// default manager on every call, so it lied in exactly that case (gh #410).
/// Fourteen UI sites used it and every hint printed the default key after a
/// rebind.
///
/// Use [`KeybindingsManager::display_keys`] (or the UI's `render_key_hint`, which
/// adds the theme's dim/muted roles) for anything a reader sees. This one is for a caller that genuinely wants the shipped default - a
/// settings screen showing "default: ctrl+o", say - and it is the only place the
/// default table is consulted without a manager.
pub fn default_key_text(action: &str) -> String {
    KeybindingsManager::new().primary_key(action)
}

/// Format one resolved key id for display: `option` for `alt` on macOS, and
/// optional capitalization of **each** chord part (pi's `formatKeyText`, whose
/// `formatKeyPart` upper-cases every part, so `ctrl+e` renders `Ctrl+E`).
///
/// The macOS spelling exists because `alt` is not a key printed on a Mac
/// keyboard; pi renames it, so a hint that says `alt+left` there names a key the
/// user cannot find.
pub fn format_key_text(key: &str, capitalize: bool) -> String {
    format_key_text_for(key, capitalize, cfg!(target_os = "macos"))
}

/// [`format_key_text`] with the platform's `alt` spelling given explicitly.
///
/// `cfg!` is a compile-time constant, so the macOS branch cannot be exercised on
/// the machine most of this project is tested on. Taking the flag as a parameter
/// keeps the rule testable everywhere (pi renames `alt` to `option` because a Mac
/// keyboard has no key printed `alt`).
pub fn format_key_text_for(key: &str, capitalize: bool, macos: bool) -> String {
    let part = |part: &str| {
        let display = if macos && part.eq_ignore_ascii_case("alt") {
            "option"
        } else {
            part
        };
        if capitalize {
            let mut chars = display.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        } else {
            display.to_string()
        }
    };
    key.split('/')
        .map(|binding| binding.split('+').map(part).collect::<Vec<_>>().join("+"))
        .collect::<Vec<_>>()
        .join("/")
}

/// The full default keybinding table, transcribed from pi's
/// `TUI_KEYBINDINGS`. Contextual shadowing is deliberate: `up`, `pageUp`
/// and `enter` mean different things to the editor and to a picker, decided
/// by which component asks.
pub fn tui_keybindings() -> &'static [(&'static str, KeybindingDefinition)] {
    &[
        // Editor navigation and editing.
        ("tui.editor.cursorUp", def!(&["up"], "Move cursor up")),
        ("tui.editor.cursorDown", def!(&["down"], "Move cursor down")),
        (
            "tui.editor.historyPrevious",
            def!(&[], "Select previous prompt history entry"),
        ),
        (
            "tui.editor.historyNext",
            def!(&[], "Select next prompt history entry"),
        ),
        (
            "tui.editor.cursorLeft",
            def!(&["left", "ctrl+b"], "Move cursor left"),
        ),
        (
            "tui.editor.cursorRight",
            def!(&["right", "ctrl+f"], "Move cursor right"),
        ),
        (
            "tui.editor.cursorWordLeft",
            def!(&["alt+left", "ctrl+left", "alt+b"], "Move cursor word left"),
        ),
        (
            "tui.editor.cursorWordRight",
            def!(
                &["alt+right", "ctrl+right", "alt+f"],
                "Move cursor word right"
            ),
        ),
        (
            "tui.editor.cursorLineStart",
            def!(&["home", "ctrl+a"], "Move to line start"),
        ),
        (
            "tui.editor.cursorLineEnd",
            def!(&["end", "ctrl+e"], "Move to line end"),
        ),
        (
            "tui.editor.jumpForward",
            def!(&["ctrl+]"], "Jump forward to character"),
        ),
        (
            "tui.editor.jumpBackward",
            def!(&["ctrl+alt+]"], "Jump backward to character"),
        ),
        (
            "tui.editor.pageUp",
            def!(&["pageUp", "ctrl+pageUp"], "Page up"),
        ),
        (
            "tui.editor.pageDown",
            def!(&["pageDown", "ctrl+pageDown"], "Page down"),
        ),
        (
            "tui.editor.deleteCharBackward",
            def!(&["backspace"], "Delete character backward"),
        ),
        (
            "tui.editor.deleteCharForward",
            def!(&["delete", "ctrl+d"], "Delete character forward"),
        ),
        (
            "tui.editor.deleteWordBackward",
            def!(&["ctrl+w", "alt+backspace"], "Delete word backward"),
        ),
        (
            "tui.editor.deleteWordForward",
            def!(&["alt+d", "alt+delete"], "Delete word forward"),
        ),
        (
            "tui.editor.deleteToLineStart",
            def!(&["ctrl+u"], "Delete to line start"),
        ),
        (
            "tui.editor.deleteToLineEnd",
            def!(&["ctrl+k"], "Delete to line end"),
        ),
        ("tui.editor.yank", def!(&["ctrl+y"], "Yank")),
        ("tui.editor.yankPop", def!(&["alt+y"], "Yank pop")),
        ("tui.editor.undo", def!(&["ctrl+-"], "Undo")),
        (
            "tui.input.newLine",
            def!(&["shift+enter", "ctrl+j"], "Insert newline"),
        ),
        ("tui.input.submit", def!(&["enter"], "Submit input")),
        ("tui.input.tab", def!(&["tab"], "Tab / autocomplete")),
        ("tui.input.copy", def!(&["ctrl+c"], "Copy selection")),
        ("tui.select.up", def!(&["up"], "Move selection up")),
        ("tui.select.down", def!(&["down"], "Move selection down")),
        ("tui.select.pageUp", def!(&["pageUp"], "Selection page up")),
        (
            "tui.select.pageDown",
            def!(&["pageDown"], "Selection page down"),
        ),
        // Application-level actions (pi's `app.*` namespace): the ones the
        // interface itself owns. They live in the registry so an embedder
        // that rebinds is honored and `/hotkeys` lists them.
        ("app.tools.expand", def!(&["ctrl+o"], "Expand tool output")),
        // gh #209: Ctrl+T opens a session tab now; thinking keeps
        // its T mnemonic on Alt+T (the free Ctrl slots collide with
        // control characters or flow control).
        (
            "app.thinking.toggle",
            def!(&["alt+t"], "Expand the thinking run"),
        ),
        ("app.tabs.new", def!(&["ctrl+t"], "Open a session tab")),
        (
            "app.tabs.close",
            def!(&["ctrl+w"], "Close the session tab (empty composer)"),
        ),
        (
            "app.tabs.next",
            def!(&["ctrl+tab"], "Cycle to the next session tab"),
        ),
        (
            "app.tabs.prev",
            def!(&["shift+ctrl+tab"], "Cycle to the previous session tab"),
        ),
        ("app.search", def!(&["ctrl+r"], "Search the transcript")),
        // gh #8 (EFG-003): pi's model-cycle keys take `ctrl+p`, the
        // muscle memory the issue is about. The extension panel moves to
        // `alt+x` - a key pi does not claim, so a later pi-parity binding
        // never has to move it again.
        (
            "app.panel.toggle",
            def!(&["alt+x"], "Toggle the extension panel"),
        ),
        (
            "app.model.cycleForward",
            def!(&["ctrl+p"], "Cycle to the next model"),
        ),
        (
            "app.models.save",
            def!(
                &["ctrl+s"],
                "Save the selected model as the default for new sessions"
            ),
        ),
        // Gh #396: the `/thinking` picker's save-as-default, pi's
        // `app.thinking.save` (`thinking-selector.ts`). The same `ctrl+s` the
        // model picker uses, because they are the same gesture in the same kind
        // of picker - a second key for one idea is the drift this repo keeps
        // closing.
        (
            "app.thinking.save",
            def!(
                &["ctrl+s"],
                "Save the selected thinking level as the default for new sessions"
            ),
        ),
        (
            "app.model.cycleBackward",
            // pi's map: `shift+ctrl+p`, with `alt+p` where a terminal
            // cannot report the shift (pi's Windows/WSL key). The registry
            // takes no platform forks, so both are defaults: a terminal
            // that sends one sends that one.
            def!(&["shift+ctrl+p", "alt+p"], "Cycle to the previous model"),
        ),
        // gh #9 (pi 1.0.0): `ctrl+x` is pi's message-copy key, and it
        // is also the first key of the Ctrl+X Ctrl+E chord - the copy
        // fires on the press, the chord on what follows it.
        (
            "app.message.copy",
            def!(
                &["ctrl+x"],
                "Copy the selection, the last reply, or the sign-in URL"
            ),
        ),
        (
            "app.editor.external",
            def!(&["ctrl+x"], "Open the external editor"),
        ),
        // Gh #403 gap 4: pi's `app.suspend`. `SIGTSTP` does not exist on
        // native Windows, and rather than leaving the key dead there the *action*
        // stays registered with a notice (`run_terminal::suspend_refusal`), because
        // a key that silently does nothing reads as a broken key.
        //
        // Gh #405 gap 3 **supersedes the "one table" note that used to sit here**:
        // the host family now has a remap (below), and on native Windows this
        // default is empty because pi gives `ctrl+z` to undo there. The notice path
        // still exists for anyone who binds suspend by hand; what changes is only
        // which key Windows hands out by default.
        ("app.suspend", def!(&["ctrl+z"], "Suspend to background")),
        ("app.interrupt", def!(&["escape"], "Interrupt the turn")),
        (
            "app.clear",
            def!(&["ctrl+c"], "Clear the input, twice to exit"),
        ),
        (
            "app.message.followUp",
            def!(&["alt+enter"], "Queue a follow-up message"),
        ),
        (
            // Gh #401: pi's `app.message.dequeue` is Alt+Up; Alt+E stays as
            // the second spelling LCA shipped first. Both take the queue
            // back to the editor, and the dock hint prints the primary.
            "app.message.dequeue",
            def!(&["alt+up", "alt+e"], "Edit all queued messages"),
        ),
        (
            "app.prompt.previous",
            // Gh #401: `alt+up` moved to `app.message.dequeue` (pi's key
            // for it). Shift+Up and Ctrl+Up keep jumping; with a queue
            // waiting, the dequeue key wins over both, which is the
            // documented precedence and what pi does.
            def!(
                &["shift+up", "ctrl+up"],
                "Jump to the previous prompt (pinned to top)"
            ),
        ),
        (
            "app.prompt.next",
            def!(
                &["shift+down", "alt+down", "ctrl+down"],
                "Jump to the next prompt (pinned to top)"
            ),
        ),
        ("tui.select.confirm", def!(&["enter"], "Confirm selection")),
        (
            "tui.select.cancel",
            def!(&["escape", "ctrl+c"], "Cancel selection"),
        ),
        (
            "tui.altScreen.pageUp",
            def!(&["pageUp"], "Scroll viewport up one page"),
        ),
        (
            "tui.altScreen.pageDown",
            def!(&["pageDown"], "Scroll viewport down one page"),
        ),
        (
            "tui.altScreen.halfPageUp",
            def!(&[], "Scroll viewport up half a page"),
        ),
        (
            "tui.altScreen.halfPageDown",
            def!(&[], "Scroll viewport down half a page"),
        ),
        (
            "tui.altScreen.lineUp",
            def!(&[], "Scroll viewport up one line"),
        ),
        (
            "tui.altScreen.lineDown",
            def!(&[], "Scroll viewport down one line"),
        ),
        (
            "tui.altScreen.previousPrompt",
            def!(
                &["ctrl+shift+up", "ctrl+up"],
                "Jump to previous semantic prompt"
            ),
        ),
        (
            "tui.altScreen.nextPrompt",
            def!(
                &["ctrl+shift+down", "ctrl+down"],
                "Jump to next semantic prompt"
            ),
        ),
        (
            "tui.altScreen.search",
            def!(&["ctrl+shift+f"], "Search the primary scroll view"),
        ),
        (
            "tui.altScreen.searchNext",
            def!(&["enter", "ctrl+g"], "Select the next search match"),
        ),
        (
            "tui.altScreen.searchPrevious",
            def!(
                &["shift+enter", "ctrl+shift+g"],
                "Select the previous search match"
            ),
        ),
        (
            "tui.altScreen.searchClose",
            def!(&["escape"], "Close transcript search"),
        ),
        (
            "tui.altScreen.top",
            def!(&["ctrl+home"], "Scroll viewport to top"),
        ),
        (
            "tui.altScreen.bottom",
            def!(&["ctrl+end"], "Scroll viewport to bottom"),
        ),
    ]
}

/// Which host family's keymap a session gets (gh #405 gap 3).
///
/// **A predicate over facts, never over `cfg!`.** pi's rule
/// (`packages/coding-agent/src/core/keybindings.ts`) is
/// `win32 || (linux && (WSL_DISTRO_NAME || WSL_INTEROP))` - and the reason it is a
/// *rule* and not a `cfg!` check is that the machine that needs the second half runs
/// a Linux binary: detecting WSL at compile time is impossible, so the environment is
/// the only evidence. Passing the platform and the environment in makes the whole
/// predicate testable from this Linux box for all four cases, which is what lets a
/// macOS/Windows claim ship with a row instead of a hope.
///
/// `WT_SESSION` is deliberately **not** part of it: Windows Terminal is a terminal,
/// not a keymap, and the user may run a native Windows build under ConPTY in it
/// (where the Windows table is already the right one because the platform is `win32`).
/// pi's own test pins that too (`expect(useWindowsKeybindings("linux", { WT_SESSION:
/// "session" })).toBe(false)`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostFamily {
    /// Native Windows, or a Linux build running under WSL: the family whose
    /// terminal swallows the Ctrl+Shift chords the Unix table spends.
    WindowsOrWsl,
    /// macOS, and every other Unix.
    Unix,
}

impl HostFamily {
    /// The family for a platform string (`std::env::consts::OS`'s vocabulary) and
    /// the process environment.
    #[must_use]
    pub fn from_parts(platform: &str, env: impl Fn(&str) -> Option<String>) -> HostFamily {
        let wsl = matches!(
            (env("WSL_DISTRO_NAME"), env("WSL_INTEROP")),
            (Some(name), _) if !name.is_empty()
        ) || matches!(
            (env("WSL_DISTRO_NAME"), env("WSL_INTEROP")),
            (_, Some(path)) if !path.is_empty()
        );
        if platform == "windows" || (platform == "linux" && wsl) {
            HostFamily::WindowsOrWsl
        } else {
            HostFamily::Unix
        }
    }

    /// The family this process runs under.
    #[must_use]
    pub fn current() -> HostFamily {
        HostFamily::from_parts(std::env::consts::OS, |key| std::env::var(key).ok())
    }
}

/// pi's remap for the Windows/WSL family (gh #405 gap 3): the defaults a Windows
/// console cannot deliver, or where `Ctrl+Z`/`Ctrl+V` mean something else there.
///
/// Delta against [`tui_keybindings`], not a second full table: every action not
/// named here keeps its Unix default, so a table entry can never be *forgotten* by
/// the remap - only thirty key lists can be wrong, not thirty-plus descriptions.
/// Each entry mirrors pi's `windowsKeybindings` conditional exactly, **except
/// `app.clipboard.pasteImage`, which LCA does not have**: a paste here is the
/// `/paste` command and the right-button handler, and pi's `Ctrl+V`/`Alt+V` split is
/// a question about a key this product never bound. Adding one to match the delta
/// would be a new feature riding on a keymap fix, which is the scope creep the brief
/// warns about; the report records it as the one pi default deliberately not
/// transcribed.
fn windows_or_wsl_defaults() -> &'static [(&'static str, &'static [&'static str])] {
    // A `const`, not a returned `&[...]`: the undo row's key depends on
    // `cfg!(target_os)`, and a `const` evaluates that at compile time (a
    // `&[...]` expression would borrow a temporary).
    const WINDOWS: &[(&str, &[&str])] = &[
        // Windows Terminal claims Ctrl+Shift+F and Alt+Enter; pi moves them.
        ("app.message.followUp", &["ctrl+q"]),
        ("app.message.dequeue", &["alt+q"]),
        ("app.search", &["ctrl+f"]),
        ("tui.altScreen.search", &["ctrl+f"]),
        // The Ctrl+Shift arrows are unreachable there; the plain Ctrl arrows are not.
        ("tui.altScreen.previousPrompt", &["ctrl+up"]),
        ("tui.altScreen.nextPrompt", &["ctrl+down"]),
        // Suspend has no SIGTSTP on Windows, and pi leaves it unbound rather than
        // shipping a key that cannot work.
        ("app.suspend", &[]),
        // `ctrl+shift+p` (model cycle backward) is claimed by the console too.
        ("app.model.cycleBackward", &["alt+p"]),
        // Undo: a native Windows console delivers Ctrl+Z to the application, so pi
        // binds it there rather than the Unix `ctrl+-`.
        ("tui.editor.undo", &["ctrl+z"]),
    ];
    const WSL: &[(&str, &[&str])] = &[
        ("app.message.followUp", &["ctrl+q"]),
        ("app.message.dequeue", &["alt+q"]),
        ("app.search", &["ctrl+f"]),
        ("tui.altScreen.search", &["ctrl+f"]),
        ("tui.altScreen.previousPrompt", &["ctrl+up"]),
        ("tui.altScreen.nextPrompt", &["ctrl+down"]),
        ("app.model.cycleBackward", &["alt+p"]),
        // Undo: `ctrl+-` is the Unix spelling, Windows Terminal delivers Ctrl+Z to
        // Windows *applications* - so a Linux WSL build keeps Alt+Z, which is pi's
        // own split (`process.platform === "win32" ? "ctrl+z" : "alt+z"`).
        ("tui.editor.undo", &["alt+z"]),
    ];
    if is_native_windows() { WINDOWS } else { WSL }
}

/// Whether this binary is a native Windows build (as opposed to a Linux one under
/// WSL). A `const`, because both arms below are compile-time facts about the
/// artifact, and the *family* question is the runtime one.
const fn is_native_windows() -> bool {
    cfg!(target_os = "windows")
}

/// A key claimed by more than one user-bound action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeybindingConflict {
    /// The contested key.
    pub key: String,
    /// The actions claiming it.
    pub keybindings: Vec<String>,
}

fn normalize_keys(keys: &[String]) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut result = Vec::new();
    for key in keys {
        if seen.insert(key.clone()) {
            result.push(key.clone());
        }
    }
    result
}

/// Resolves actions to keys under a user override map, and reports
/// user-vs-user conflicts (default conflicts are contextual and allowed).
#[derive(Debug, Clone)]
pub struct KeybindingsManager {
    user_bindings: BTreeMap<String, Vec<String>>,
    keys_by_action: BTreeMap<String, Vec<String>>,
    conflicts: Vec<KeybindingConflict>,
    invalid_keys: Vec<(String, String)>,
    /// This terminal's Kitty keyboard protocol state (gh #323 defect 2).
    ///
    /// `None` means "no terminal attached", and a match then asks the ambient
    /// process value - which is exactly the pre-#323 behaviour, so a caller that
    /// never attaches anything is unchanged. A terminal attaches its own cell with
    /// [`KeybindingsManager::set_kitty_state`], and from then on this manager's
    /// answer comes from *that* terminal, so two managers in one process can
    /// disagree at the same instant.
    kitty_state: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    /// Which host family's defaults this manager resolves against (gh #405
    /// gap 3).
    ///
    /// A field rather than a call inside `rebuild`, so the Windows/WSL table can be
    /// driven from a Linux test (the only platform this repo's suite runs on) and
    /// so a session's keymap cannot change under it if the environment does.
    family: HostFamily,
}

impl Default for KeybindingsManager {
    fn default() -> Self {
        Self::new()
    }
}

impl KeybindingsManager {
    /// Build from the default table with no user overrides, for **this** host's
    /// family ([`HostFamily::current`]).
    pub fn new() -> Self {
        Self::with_user_bindings(BTreeMap::new())
    }

    /// Build with a user override map (an empty `Vec` explicitly unbinds), for this
    /// host's family.
    pub fn with_user_bindings(user_bindings: BTreeMap<String, Vec<String>>) -> Self {
        Self::with_user_bindings_for(user_bindings, HostFamily::current())
    }

    /// Build for an explicit host family (gh #405 gap 3).
    ///
    /// The seam a row uses to exercise the Windows/WSL defaults from Linux: on this
    /// box `HostFamily::current()` is [`HostFamily::Unix`], and a Windows table no
    /// test could construct would be a Windows table no test checks.
    pub fn with_user_bindings_for(
        user_bindings: BTreeMap<String, Vec<String>>,
        family: HostFamily,
    ) -> Self {
        let mut manager = Self {
            user_bindings,
            keys_by_action: BTreeMap::new(),
            conflicts: Vec::new(),
            invalid_keys: Vec::new(),
            kitty_state: None,
            family,
        };
        manager.rebuild();
        manager
    }

    /// This manager's host family.
    #[must_use]
    pub fn family(&self) -> HostFamily {
        self.family
    }

    fn rebuild(&mut self) {
        self.keys_by_action.clear();
        self.conflicts.clear();
        self.invalid_keys.clear();

        let mut user_claims: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for (action, keys) in &self.user_bindings {
            if !tui_keybindings().iter().any(|(name, _)| name == action) {
                continue;
            }
            for key in normalize_keys(keys) {
                // Invalid keys never install (defaults hold instead),
                // so they claim nothing.
                if !is_valid_key_id(&key) {
                    continue;
                }
                user_claims.entry(key).or_default().insert(action.clone());
            }
        }
        for (key, actions) in user_claims {
            if actions.len() > 1 {
                self.conflicts.push(KeybindingConflict {
                    key,
                    keybindings: actions.into_iter().collect(),
                });
            }
        }

        for (action, definition) in tui_keybindings() {
            // Gh #405 gap 3: the host family's remap wins over the Unix table, key
            // list for key list. A `BTreeMap` rather than a scan, because the delta
            // is consulted once per action and the table is static.
            let keys_for_family: &'static [&'static str] = if self.family == HostFamily::Unix {
                definition.default_keys
            } else {
                windows_or_wsl_defaults()
                    .iter()
                    .find(|(name, _)| *name == *action)
                    .map_or(definition.default_keys, |(_, keys)| *keys)
            };
            let defaults: Vec<String> = keys_for_family.iter().map(|k| (*k).to_string()).collect();
            let keys = match self.user_bindings.get(*action) {
                Some(user) => {
                    let invalid: Vec<String> = normalize_keys(user)
                        .into_iter()
                        .filter(|key| !is_valid_key_id(key))
                        .collect();
                    if invalid.is_empty() {
                        normalize_keys(user)
                    } else {
                        // A poisoned action keeps defaults (gh #66
                        // follow-up): installing the bad list would
                        // silently disable the action, which is the
                        // failure mode the report exists to kill.
                        for key in invalid {
                            self.invalid_keys.push(((*action).to_string(), key));
                        }
                        defaults
                    }
                }
                None => defaults,
            };
            self.keys_by_action.insert((*action).to_string(), keys);
        }
    }

    /// Resolve one action to its keys.
    pub fn keys(&self, action: &str) -> Vec<String> {
        self.keys_by_action.get(action).cloned().unwrap_or_default()
    }

    /// Whether raw input matches the action.
    ///
    /// **The signature is unchanged on purpose** (gh #323): this is called from 44
    /// sites in `lca-ui`, a crate this cycle does not touch, and threading a
    /// parameter through all of them would be a much larger change for the same
    /// behaviour. The terminal's state comes from the manager instead - see
    /// [`KeybindingsManager::set_kitty_state`].
    pub fn matches(&self, data: &str, action: &str) -> bool {
        let kitty = self
            .kitty_state
            .as_ref()
            .map(|cell| cell.load(std::sync::atomic::Ordering::SeqCst));
        self.keys(action)
            .iter()
            .any(|key| matches_key_with_kitty(data, key, kitty))
    }

    /// Attach this terminal's Kitty keyboard protocol state (gh #323 defect 2).
    ///
    /// Takes the terminal's own `Arc<AtomicBool>`, so the value tracks negotiation
    /// live without this manager owning a clock or a notification: the reader
    /// thread stores into the same cell the match reads. Two managers holding two
    /// cells can therefore disagree, which is the property a process-global (or a
    /// `thread_local`) cannot have.
    pub fn set_kitty_state(&mut self, state: std::sync::Arc<std::sync::atomic::AtomicBool>) {
        self.kitty_state = Some(state);
    }

    /// Detach, falling back to the ambient process value.
    pub fn clear_kitty_state(&mut self) {
        self.kitty_state = None;
    }

    /// Whether this manager knows its terminal's Kitty state.
    #[must_use]
    pub fn has_kitty_state(&self) -> bool {
        self.kitty_state.is_some()
    }

    /// Replace the user override map and rebuild.
    pub fn set_user_bindings(&mut self, user_bindings: BTreeMap<String, Vec<String>>) {
        self.user_bindings = user_bindings;
        self.rebuild();
    }

    /// The current user override map.
    pub fn user_bindings(&self) -> BTreeMap<String, Vec<String>> {
        self.user_bindings.clone()
    }

    /// User-bound action names the registry never defined (gh #66): the
    /// loader reports these loud, so a typo cannot silently do nothing.
    pub fn unknown_actions(&self) -> Vec<String> {
        self.user_bindings
            .keys()
            .filter(|action| !tui_keybindings().iter().any(|(name, _)| name == *action))
            .cloned()
            .collect()
    }

    /// The resolved action → keys view.
    pub fn resolved_bindings(&self) -> BTreeMap<String, Vec<String>> {
        self.keys_by_action.clone()
    }

    /// User-vs-user conflicts.
    pub fn conflicts(&self) -> &[KeybindingConflict] {
        &self.conflicts
    }

    /// `(action, key)` pairs whose value never names a key (gh #66
    /// follow-up): the action keeps defaults, and the loader reports
    /// the value loud.
    pub fn invalid_keys(&self) -> &[(String, String)] {
        &self.invalid_keys
    }

    /// The primary key for an action (`ctrl+o`), or empty when unbound.
    pub fn primary_key(&self, action: &str) -> String {
        self.keys(action).into_iter().next().unwrap_or_default()
    }

    /// Every key for an action as one display string: `/`-joined bindings,
    /// `+`-joined chord parts, `option` for `alt` on macOS.
    ///
    /// This is the string a hint shows, and it comes from **this** manager, so a
    /// user override or an explicit unbind is what the reader sees (gh #410).
    /// Empty means "unbound": a caller must render no hint at all rather than a
    /// blank key press.
    pub fn display_keys(&self, action: &str) -> String {
        let keys = self.keys(action);
        if keys.is_empty() {
            return String::new();
        }
        let joined = keys.join("/");
        format_key_text(&joined, false)
    }

    /// [`Self::display_keys`] with each chord part capitalized (`Ctrl+E`), pi's
    /// `keyDisplayText` form for a sentence start.
    pub fn display_keys_capitalized(&self, action: &str) -> String {
        let keys = self.keys(action);
        if keys.is_empty() {
            return String::new();
        }
        let joined = keys.join("/");
        format_key_text(&joined, true)
    }

    /// The description for an action, if defined.
    pub fn description(&self, action: &str) -> Option<&'static str> {
        tui_keybindings()
            .iter()
            .find(|(name, _)| *name == action)
            .map(|(_, def)| def.description)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Verifies: gh #173 - Shift+Up/Down join the prompt-step
    // bindings (pi's Shift+Scroll ergonomics have a keyboard twin).
    #[test]
    fn shift_arrows_step_between_prompts() {
        let kb = KeybindingsManager::new();
        assert!(kb.matches("\x1b[1;2A", "app.prompt.previous"));
        assert!(kb.matches("\x1b[1;2B", "app.prompt.next"));
        assert!(!kb.matches("\x1b[1;2A", "app.prompt.next"));
    }

    // Verifies: gh #410 - a hint's display form follows pi's `formatKeyText`:
    // `alt` reads as `option` on macOS (a Mac keyboard prints no key called
    // `alt`), a chord keeps its `+` parts, and a multi-binding list joins with
    // `/`.
    #[test]
    fn key_display_follows_pis_formatting_rules() {
        assert_eq!(
            format_key_text_for("alt+left", false, true),
            "option+left",
            "macOS spells `alt` as `option`"
        );
        assert_eq!(
            format_key_text_for("alt+left", false, false),
            "alt+left",
            "other platforms keep `alt`"
        );
        assert_eq!(
            format_key_text_for("ctrl+x+ctrl+e", false, false),
            "ctrl+x+ctrl+e",
            "a chord keeps every part"
        );
        assert_eq!(
            format_key_text_for("shift+up/ctrl+p", false, false),
            "shift+up/ctrl+p",
            "multiple bindings join with `/`"
        );
        // pi's `formatKeyPart` capitalizes **every** part, not only the first
        // character of the whole string: `ctrl+e` renders `Ctrl+E`.
        assert_eq!(
            format_key_text_for("ctrl+e", true, false),
            "Ctrl+E",
            "the capitalized form capitalizes each chord part, as pi does"
        );
        assert_eq!(
            format_key_text_for("ctrl+c", true, false),
            "Ctrl+C",
            "so a single modifier+letter form reads naturally at a sentence start"
        );
        // Only the `alt` *part* is renamed, never a key that merely contains it.
        assert_eq!(
            format_key_text_for("altitude", true, true),
            "Altitude",
            "a key id containing `alt` is not a modifier"
        );
    }

    // Verifies: gh #410 - the manager's display form is the live one.
    //
    // The whole defect in one row: a rebound action must display the override, an
    // unbound one must display nothing at all, and the default table must not leak
    // through either.
    #[test]
    fn display_keys_follows_the_override_and_the_unbind() {
        let mut user = BTreeMap::new();
        user.insert(
            "app.tools.expand".to_string(),
            vec!["ctrl+e".to_string(), "ctrl+g".to_string()],
        );
        user.insert("app.thinking.toggle".to_string(), Vec::new());
        let kb = KeybindingsManager::with_user_bindings(user);

        assert_eq!(kb.display_keys("app.tools.expand"), "ctrl+e/ctrl+g");
        assert_eq!(
            kb.display_keys("app.thinking.toggle"),
            "",
            "an explicit unbind displays nothing, so a caller omits the hint"
        );
        assert_eq!(kb.display_keys("app.interrupt"), "escape");
        assert_eq!(kb.primary_key("app.tools.expand"), "ctrl+e");
    }

    #[test]
    fn defaults_resolve_and_match() {
        let kb = KeybindingsManager::new();
        assert!(kb.matches("\x1b[A", "tui.editor.cursorUp"));
        assert!(kb.matches("\x1b[A", "tui.select.up"));
        assert!(kb.matches("\r", "tui.input.submit"));
        assert!(kb.matches("\t", "tui.input.tab"));
        assert!(kb.matches("\x03", "tui.input.copy"));
        assert!(!kb.matches("\x1b[B", "tui.editor.cursorUp"));
    }

    // Verifies: gh #66 - the loader reports names the registry never
    // defined, so a typo fails loud instead of silently doing nothing.
    #[test]
    fn unknown_action_names_are_reported() {
        let mut user = BTreeMap::new();
        user.insert(
            "tui.editor.cursorLeft".to_string(),
            vec!["ctrl+x".to_string()],
        );
        user.insert("app.typo.here".to_string(), vec!["ctrl+y".to_string()]);
        let kb = KeybindingsManager::with_user_bindings(user);
        assert_eq!(kb.unknown_actions(), vec!["app.typo.here".to_string()]);
        assert!(kb.matches("\x18", "tui.editor.cursorLeft"));
    }

    // Verifies: gh #66 follow-up - an unparseable key value falls back
    // to defaults for its action and reports the offending value.
    #[test]
    fn an_unparseable_key_keeps_defaults_and_reports() {
        let mut user = BTreeMap::new();
        user.insert(
            "tui.editor.cursorLeft".to_string(),
            vec!["ctrl+xyz".to_string()],
        );
        let kb = KeybindingsManager::with_user_bindings(user);
        assert_eq!(
            kb.keys("tui.editor.cursorLeft"),
            vec!["left".to_string(), "ctrl+b".to_string()],
            "defaults hold for the poisoned action"
        );
        assert_eq!(
            kb.invalid_keys(),
            &[("tui.editor.cursorLeft".to_string(), "ctrl+xyz".to_string())]
        );
    }

    // Verifies: gh #66 - every key in a multi-key list fires.
    #[test]
    fn every_key_in_a_list_fires() {
        let mut user = BTreeMap::new();
        user.insert(
            "tui.editor.cursorLeft".to_string(),
            vec!["ctrl+x".to_string(), "ctrl+y".to_string()],
        );
        let kb = KeybindingsManager::with_user_bindings(user);
        assert!(kb.matches("\x18", "tui.editor.cursorLeft"));
        assert!(kb.matches("\x19", "tui.editor.cursorLeft"));
        assert!(
            !kb.matches("\x02", "tui.editor.cursorLeft"),
            "the default is replaced"
        );
    }

    #[test]
    fn user_override_and_explicit_unbind() {
        let mut user = BTreeMap::new();
        user.insert("tui.input.submit".to_string(), vec!["ctrl+s".to_string()]);
        user.insert("tui.input.tab".to_string(), Vec::new());
        let kb = KeybindingsManager::with_user_bindings(user);
        assert!(kb.matches("\x13", "tui.input.submit"));
        assert!(!kb.matches("\r", "tui.input.submit"));
        assert!(kb.keys("tui.input.tab").is_empty());
    }

    // Verifies: gh #8 (EFG-003) - pi's model-cycle keys are the defaults
    // the issue asks for, exactly one action claims `ctrl+p`, and the
    // extension panel kept a key of its own after moving (`alt+x`).
    #[test]
    fn the_model_cycle_keys_are_pis_and_ctrl_p_is_claimed_once() {
        let kb = KeybindingsManager::new();
        assert!(
            kb.matches("\x10", "app.model.cycleForward"),
            "Ctrl+P cycles"
        );
        assert!(
            kb.matches("\x1bp", "app.model.cycleBackward"),
            "Alt+P is the backward key pi keeps for terminals that drop the shift"
        );
        assert_eq!(
            kb.keys("app.model.cycleBackward"),
            vec!["shift+ctrl+p", "alt+p"],
            "both of pi's backward keys are defaults"
        );
        assert!(
            !kb.matches("\x10", "app.panel.toggle"),
            "Ctrl+P is the cycle, not the panel"
        );
        assert!(
            kb.matches("\x1bx", "app.panel.toggle"),
            "alt+x is the panel"
        );
        let claimants: Vec<&str> = tui_keybindings()
            .iter()
            .filter(|(_, def)| def.default_keys.contains(&"ctrl+p"))
            .map(|(name, _)| *name)
            .collect();
        assert_eq!(
            claimants,
            vec!["app.model.cycleForward"],
            "one action claims Ctrl+P"
        );
    }

    // Verifies: gh #8 phase 2 (pi's `app.models.save`) and gh #396 (pi's
    // `app.thinking.save`) - Ctrl+S is the **save-as-default** key, and every
    // action that claims it is a picker's save.
    //
    // Gh #396 sanctioned change: this row used to assert exactly one claimant.
    // There are now two, and that is correct rather than a collision: the two
    // pickers cannot be open at once (`/model` and `/thinking` each open their own
    // and `Chat::picker_open` dispatches to whichever is up), so the key is never
    // ambiguous at runtime. What the row must still forbid is a claimant that is
    // *not* a save gesture - that would be a genuine conflict. The exclusion
    // itself is pinned in `lca-ui` (`the_two_save_actions_belong_to_mutually_
    // exclusive_pickers`), where the pickers live.
    #[test]
    fn ctrl_s_is_pis_save_default_and_no_other_kind_of_action_claims_it() {
        let kb = KeybindingsManager::new();
        assert!(kb.matches("\x13", "app.models.save"));
        assert!(kb.matches("\x13", "app.thinking.save"));
        let claimants: Vec<&str> = tui_keybindings()
            .iter()
            .filter(|(_, def)| def.default_keys.contains(&"ctrl+s"))
            .map(|(name, _)| *name)
            .collect();
        assert_eq!(
            claimants,
            vec!["app.models.save", "app.thinking.save"],
            "exactly the two picker saves claim Ctrl+S"
        );
        for name in &claimants {
            assert!(
                name.ends_with(".save"),
                "{name} claims Ctrl+S but is not a save-default gesture"
            );
        }
    }

    // Verifies: gh #9 (pi 1.0.0's `app.message.copy`) - Ctrl+X answers
    // to the copy key, and the external-editor chord still starts there
    // too (FR-UI-15's suite row keeps passing).
    #[test]
    fn ctrl_x_is_pis_message_copy_and_still_arms_the_editor_chord() {
        let kb = KeybindingsManager::new();
        assert!(kb.matches("\x18", "app.message.copy"));
        assert!(
            kb.matches("\x18", "app.editor.external"),
            "the Ctrl+X Ctrl+E chord's first key is unchanged"
        );
    }

    // Verifies: gh #405 gap 3 - **pi's predicate, as a pure function, for all four
    // platforms.** `win32`, and a *Linux* build under WSL (`WSL_DISTRO_NAME` or
    // `WSL_INTEROP`), are the Windows family; macOS, plain Linux and a Linux
    // session inside Windows Terminal are not.
    //
    // The `WT_SESSION` case is the one worth a row of its own: Windows Terminal is a
    // *terminal*, not a keymap, and pi pins it as false
    // (`keybindings.test.ts:15`). Getting it wrong would remap a native Linux
    // session that merely happens to run under that terminal.
    #[test]
    fn the_host_family_predicate_is_pis_rule() {
        let env = |pairs: &[(&str, &str)]| {
            let owned: Vec<(String, String)> = pairs
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect();
            move |key: &str| {
                owned
                    .iter()
                    .find(|(name, _)| name == key)
                    .map(|(_, value)| value.clone())
            }
        };
        // A named shape rather than an array-of-tuples-of-Vec: the row reads
        // better and clippy's `type_complexity` agrees.
        type Case<'a> = (&'a str, Vec<(&'a str, &'a str)>, HostFamily);
        let cases: [Case<'_>; 6] = [
            ("windows", vec![], HostFamily::WindowsOrWsl),
            (
                "linux",
                vec![("WSL_DISTRO_NAME", "Ubuntu")],
                HostFamily::WindowsOrWsl,
            ),
            (
                "linux",
                vec![("WSL_INTEROP", "/run/WSL/123_interop")],
                HostFamily::WindowsOrWsl,
            ),
            // pi's negative: a terminal is not a keymap.
            ("linux", vec![("WT_SESSION", "session")], HostFamily::Unix),
            ("linux", vec![], HostFamily::Unix),
            ("macos", vec![], HostFamily::Unix),
        ];
        for (platform, pairs, expected) in cases {
            let family = HostFamily::from_parts(platform, env(&pairs));
            assert_eq!(
                family, expected,
                "platform {platform:?} with {pairs:?} must be {expected:?}"
            );
        }
        // An empty variable is not evidence: a shell that exports `WSL_DISTRO_NAME=`
        // must not flip the keymap.
        assert_eq!(
            HostFamily::from_parts("linux", env(&[("WSL_DISTRO_NAME", "")])),
            HostFamily::Unix,
            "an empty WSL_DISTRO_NAME is not WSL"
        );
        // And this box is Unix, so the rest of the suite is comparing against the
        // Unix table by construction - stated rather than assumed.
        assert_eq!(
            HostFamily::current(),
            if cfg!(any(target_os = "windows", target_os = "linux")) {
                HostFamily::current()
            } else {
                HostFamily::Unix
            },
            "the current family is answered, not panicking"
        );
    }

    // Verifies: gh #405 gap 3 - **the remap is pi's, key by key.** Asserted as a
    // table rather than through `matches`, because the subject is the *default list*
    // an action carries (what `/hotkeys` prints and what the next reader copies),
    // and a byte-level match would pass for a key that is listed under two actions.
    //
    // pi's `KEYBINDINGS` overrides are the authority
    // (`packages/coding-agent/src/core/keybindings.ts`): followUp `ctrl+q`, dequeue
    // `alt+q`, pasteImage `alt+v`, undo `ctrl+z` on native Windows and `alt+z`
    // under WSL, altScreen.search `ctrl+f`, and the plain Ctrl arrows for the
    // prompt jumps.
    #[test]
    fn the_windows_family_remaps_pis_keys_and_keeps_the_rest() {
        let kb =
            KeybindingsManager::with_user_bindings_for(BTreeMap::new(), HostFamily::WindowsOrWsl);
        assert_eq!(kb.family(), HostFamily::WindowsOrWsl);
        for (action, expected) in [
            ("app.message.followUp", vec!["ctrl+q"]),
            ("app.message.dequeue", vec!["alt+q"]),
            ("app.search", vec!["ctrl+f"]),
            ("tui.altScreen.search", vec!["ctrl+f"]),
            ("tui.altScreen.previousPrompt", vec!["ctrl+up"]),
            ("tui.altScreen.nextPrompt", vec!["ctrl+down"]),
            ("app.model.cycleBackward", vec!["alt+p"]),
        ] {
            assert_eq!(
                kb.keys(action),
                expected,
                "{action} must take pi's Windows/WSL default"
            );
        }
        // Two defaults differ *inside* the family, and both are pi's own split
        // (`process.platform === "win32" ? ... : ...`): a native Windows console
        // delivers Ctrl+Z and has no SIGTSTP, so undo is `ctrl+z` and suspend is
        // unbound; a WSL build keeps Alt+Z for undo and Ctrl+Z for suspend, because
        // job control exists there.
        #[cfg(target_os = "windows")]
        assert_eq!(kb.keys("app.suspend"), Vec::<String>::new());
        #[cfg(not(target_os = "windows"))]
        assert_eq!(
            kb.keys("app.suspend"),
            vec!["ctrl+z"],
            "a WSL build has SIGTSTP, so pi keeps the Unix suspend key"
        );
        #[cfg(target_os = "windows")]
        assert_eq!(kb.keys("tui.editor.undo"), vec!["ctrl+z"]);
        #[cfg(not(target_os = "windows"))]
        assert_eq!(
            kb.keys("tui.editor.undo"),
            vec!["alt+z"],
            "a Linux WSL build keeps Alt+Z - Windows Terminal takes Ctrl+Z itself"
        );
        // Only the remapped actions changed: an action outside the delta is
        // byte-identical to its Unix default, which is what stops the remap from
        // becoming a second table nobody diffs.
        for (action, definition) in tui_keybindings() {
            if windows_or_wsl_defaults()
                .iter()
                .any(|(name, _)| *name == *action)
            {
                continue;
            }
            let expected: Vec<String> = definition
                .default_keys
                .iter()
                .map(|k| (*k).to_string())
                .collect();
            assert_eq!(
                kb.keys(action),
                expected,
                "{action} is not in the Windows delta, so it must keep its Unix default"
            );
        }
    }

    // Verifies: gh #405 gap 3 - **no Unix binding changes.** The whole point of the
    // predicate is that a macOS or plain-Linux session is *bit-identical* to what
    // shipped before this cycle; a remap that leaked into the Unix family would be
    // the defect this row exists to catch, and it is checked against the static
    // table rather than against a remembered list.
    #[test]
    fn the_unix_family_defaults_are_the_static_table_unchanged() {
        let kb = KeybindingsManager::with_user_bindings_for(BTreeMap::new(), HostFamily::Unix);
        assert_eq!(kb.family(), HostFamily::Unix);
        for (action, definition) in tui_keybindings() {
            let expected: Vec<String> = definition
                .default_keys
                .iter()
                .map(|k| (*k).to_string())
                .collect();
            assert_eq!(
                kb.keys(action),
                expected,
                "Unix must keep {action} exactly as the table declares it"
            );
        }
        // The bindings the Windows delta lists, spelled out as the Unix values they
        // must still have - a set equality on a whole table cannot show that a
        // specific user-visible key was *not* moved.
        for (action, unix_keys) in [
            ("app.message.followUp", vec!["alt+enter"]),
            ("app.message.dequeue", vec!["alt+up", "alt+e"]),
            ("app.search", vec!["ctrl+r"]),
            ("tui.altScreen.search", vec!["ctrl+shift+f"]),
            (
                "tui.altScreen.previousPrompt",
                vec!["ctrl+shift+up", "ctrl+up"],
            ),
            (
                "tui.altScreen.nextPrompt",
                vec!["ctrl+shift+down", "ctrl+down"],
            ),
            ("tui.editor.undo", vec!["ctrl+-"]),
            ("app.suspend", vec!["ctrl+z"]),
            ("app.model.cycleBackward", vec!["shift+ctrl+p", "alt+p"]),
        ] {
            assert_eq!(
                kb.keys(action),
                unix_keys,
                "{action} must not move on Unix (gh #405 gap 3 changes Windows only)"
            );
        }
        // pi has `app.clipboard.pasteImage`; LCA does not (paste is `/paste` and the
        // right button). Asserted rather than implied: if someone ever adds the
        // action, this row fails and the Windows delta gets its entry at the same
        // time, instead of a Unix-only binding shipping for a Windows family.
        for family in [HostFamily::Unix, HostFamily::WindowsOrWsl] {
            let kb = KeybindingsManager::with_user_bindings_for(BTreeMap::new(), family);
            assert!(
                kb.keys("app.clipboard.pasteImage").is_empty(),
                "LCA has no paste key in {family:?}; adding one is a feature, not a remap"
            );
        }
    }

    #[test]
    fn user_conflicts_are_reported() {
        let mut user = BTreeMap::new();
        user.insert("tui.input.submit".to_string(), vec!["ctrl+s".to_string()]);
        user.insert("tui.input.tab".to_string(), vec!["ctrl+s".to_string()]);
        let kb = KeybindingsManager::with_user_bindings(user);
        assert_eq!(kb.conflicts().len(), 1);
        assert_eq!(kb.conflicts()[0].keybindings.len(), 2);
    }
}

// Verifies: gh #209 - Ctrl+T opens a session tab (thinking expands
// on Ctrl+G now), Ctrl+W closes one, Ctrl+Tab cycles; each claimed
// exactly once.
#[test]
fn tab_keys_are_claimed_once() {
    let kb = KeybindingsManager::new();
    assert!(kb.matches("\x14", "app.tabs.new"), "ctrl+t opens");
    assert!(kb.matches("\x1bt", "app.thinking.toggle"), "alt+t expands");
    assert!(kb.matches("\x17", "app.tabs.close"), "ctrl+w closes");
    for (key, action) in [
        ("ctrl+t", "app.tabs.new"),
        ("alt+t", "app.thinking.toggle"),
        ("ctrl+tab", "app.tabs.next"),
        ("shift+ctrl+tab", "app.tabs.prev"),
    ] {
        let claimants: Vec<&str> = tui_keybindings()
            .iter()
            .filter(|(_, def)| def.default_keys.contains(&key))
            .map(|(name, _)| *name)
            .collect();
        assert_eq!(claimants, vec![action], "one action claims {key}");
    }
    // gh #209: Ctrl+W is shared on purpose - the editor's word
    // delete wins with text, the tab close wins on empty. The
    // runtime rule lives in Chat, not the registry.
    let claimants: Vec<&str> = tui_keybindings()
        .iter()
        .filter(|(_, def)| def.default_keys.contains(&"ctrl+w"))
        .map(|(name, _)| *name)
        .collect();
    assert_eq!(
        claimants,
        vec!["tui.editor.deleteWordBackward", "app.tabs.close"],
        "ctrl+w is shared deliberately"
    );
}
