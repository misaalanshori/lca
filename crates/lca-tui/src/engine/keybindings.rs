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

// Gh #405 gap 3: the keybinding rows moved to a sibling file (gate 11's ceiling).
// The reported path is preserved (`engine::keybindings::tests::<row>`), because a
// declared test module keeps its names wherever its body lives (gh #574).
#[cfg(test)]
#[path = "keybindings/tests.rs"]
mod tests;

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
