//! Semantic keybinding registry, ported from pi's
//! `packages/tui/src/keybindings.ts`
//! (`pi-tui-re/src_re/tui-engine/keybindings.md`).
//!
//! Actions are names, not keys. Multiple default keys per action give the
//! readline-plus-arrows flavor without platform forks; a user override map
//! can rebind or explicitly unbind (`[]`).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, OnceLock};

use super::keys::matches_key;

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
            def!(&["home", "ctrl+home", "ctrl+a"], "Move to line start"),
        ),
        (
            "tui.editor.cursorLineEnd",
            def!(&["end", "ctrl+end", "ctrl+e"], "Move to line end"),
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
            def!(&["home"], "Scroll viewport to top"),
        ),
        (
            "tui.altScreen.bottom",
            def!(&["end"], "Scroll viewport to bottom"),
        ),
    ]
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
}

impl Default for KeybindingsManager {
    fn default() -> Self {
        Self::new()
    }
}

impl KeybindingsManager {
    /// Build from the default table with no user overrides.
    pub fn new() -> Self {
        Self::with_user_bindings(BTreeMap::new())
    }

    /// Build with a user override map (an empty `Vec` explicitly unbinds).
    pub fn with_user_bindings(user_bindings: BTreeMap<String, Vec<String>>) -> Self {
        let mut manager = Self {
            user_bindings,
            keys_by_action: BTreeMap::new(),
            conflicts: Vec::new(),
        };
        manager.rebuild();
        manager
    }

    fn rebuild(&mut self) {
        self.keys_by_action.clear();
        self.conflicts.clear();

        let mut user_claims: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for (action, keys) in &self.user_bindings {
            if !tui_keybindings().iter().any(|(name, _)| name == action) {
                continue;
            }
            for key in normalize_keys(keys) {
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
            let keys = match self.user_bindings.get(*action) {
                Some(user) => normalize_keys(user),
                None => definition
                    .default_keys
                    .iter()
                    .map(|k| (*k).to_string())
                    .collect(),
            };
            self.keys_by_action.insert((*action).to_string(), keys);
        }
    }

    /// Resolve one action to its keys.
    pub fn keys(&self, action: &str) -> Vec<String> {
        self.keys_by_action.get(action).cloned().unwrap_or_default()
    }

    /// Whether raw input matches the action.
    pub fn matches(&self, data: &str, action: &str) -> bool {
        self.keys(action).iter().any(|key| matches_key(data, key))
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

    /// The resolved action → keys view.
    pub fn resolved_bindings(&self) -> BTreeMap<String, Vec<String>> {
        self.keys_by_action.clone()
    }

    /// User-vs-user conflicts.
    pub fn conflicts(&self) -> &[KeybindingConflict] {
        &self.conflicts
    }

    /// The description for an action, if defined.
    pub fn description(&self, action: &str) -> Option<&'static str> {
        tui_keybindings()
            .iter()
            .find(|(name, _)| *name == action)
            .map(|(_, def)| def.description)
    }
}

static GLOBAL: OnceLock<Mutex<KeybindingsManager>> = OnceLock::new();

/// Install the process-wide keybindings, replacing any previous one.
pub fn set_keybindings(manager: KeybindingsManager) {
    match GLOBAL.get() {
        Some(lock) => *lock.lock().unwrap() = manager,
        None => {
            let _ = GLOBAL.set(Mutex::new(manager));
        }
    }
}

/// Run a closure with the process-wide keybindings (defaults if unset).
pub fn with_keybindings<R>(f: impl FnOnce(&KeybindingsManager) -> R) -> R {
    static DEFAULT: OnceLock<KeybindingsManager> = OnceLock::new();
    match GLOBAL.get() {
        Some(lock) => f(&lock.lock().unwrap()),
        None => f(DEFAULT.get_or_init(KeybindingsManager::new)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
