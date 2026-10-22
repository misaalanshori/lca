//! Reading **one** configuration layer on its own (#397 tri-state).
//!
//! [`Config::load`](super::Config::load) merges every layer into one answer,
//! which is what the session and `lca config` need. The `/settings` picker
//! needs the opposite: for the row in front of the user, *what does the project
//! file itself say?* Without that the picker cannot show the three states a
//! project override actually has:
//!
//! | State | Project file holds | Meaning |
//! | --- | --- | --- |
//! | inherit | the key is **absent** | use the user file / default |
//! | on | `key = true` | this project turns it on |
//! | off | `key = false` | this project turns it off |
//!
//! Conflating *absent* with *false* is what makes a hand-written project
//! override a one-way door: the picker can write, but it cannot see that the key
//! is there, so it cannot offer to remove it.
//!
//! This reader is deliberately dumb — it flattens the file's scalars to dotted
//! keys and validates nothing. Validation is [`super::Config::load`]'s job, at the
//! layer that knows the vocabulary; a row only needs to know whether a key is
//! present and what it spells.

use std::collections::BTreeMap;
use std::path::Path;

/// Every scalar in one layer, keyed by its dotted name, with values rendered
/// as the strings the picker compares against (TOML booleans and numbers are
/// unquoted, so `ui.color = "never"` and `ui.quiet_startup = false` both come
/// back in the spelling their cycle lists use).
///
/// `None` when the file does not exist — which is the **inherit** state, and
/// deliberately distinct from an empty map (a file that exists but says
/// nothing about this key also inherits, but the distinction is what lets the
/// writer know whether it created the file).
#[must_use]
pub fn read_layer(path: &Path) -> Option<BTreeMap<String, String>> {
    let text = std::fs::read_to_string(path).ok()?;
    // A layer that does not parse is treated as absent here: `Config::load`
    // reports the parse error where the user can act on it, and a picker row
    // must never be the thing that explains a broken TOML file.
    let table: toml::Table = text.parse().ok()?;
    let mut out = BTreeMap::new();
    flatten("", &toml::Value::Table(table), &mut out);
    Some(out)
}

/// Walk the table, recording every scalar under its dotted path. Tables
/// recurse; arrays and inline tables are skipped because no picker row cycles
/// one (the list keys have their own checklists).
fn flatten(prefix: &str, value: &toml::Value, out: &mut BTreeMap<String, String>) {
    let toml::Value::Table(table) = value else {
        return;
    };
    for (key, item) in table {
        let path = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        match item {
            toml::Value::Table(_) => flatten(&path, item, out),
            toml::Value::String(text) => {
                out.insert(path, text.clone());
            }
            toml::Value::Boolean(flag) => {
                out.insert(path, flag.to_string());
            }
            toml::Value::Integer(number) => {
                out.insert(path, number.to_string());
            }
            toml::Value::Float(number) => {
                out.insert(path, format_float(*number));
            }
            _ => {}
        }
    }
}

/// A float in the spelling a cycle list uses: `0.8`, not `0.8f64` or `0.800`.
fn format_float(number: f64) -> String {
    let mut text = format!("{number}");
    if text.contains('.') {
        while text.ends_with('0') {
            text.pop();
        }
        if text.ends_with('.') {
            text.pop();
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layer(tag: &str, body: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("lca-layer-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("config.toml");
        std::fs::write(&path, body).expect("write");
        path
    }

    // Verifies: gh #397 - a key's presence is the thing the picker needs, and
    // a TOML boolean comes back in the spelling a cycle list compares against
    // (`false`, not `Bool(false)`).
    #[test]
    fn a_layer_reports_present_keys_in_cycle_spelling() {
        let path = layer(
            "present",
            "ui.color = \"never\"\nui.quiet_startup = false\ncompaction.threshold = 0.8\n",
        );
        let values = read_layer(&path).expect("the file exists");
        assert_eq!(values.get("ui.color").map(String::as_str), Some("never"));
        assert_eq!(
            values.get("ui.quiet_startup").map(String::as_str),
            Some("false"),
            "a boolean is spelled the way the cycle list spells it"
        );
        assert_eq!(
            values.get("compaction.threshold").map(String::as_str),
            Some("0.8"),
            "a float keeps its value and loses f64 noise"
        );
    }

    // Verifies: gh #397 - a section spells its children with the same dotted
    // keys the rows use, or the picker cannot match them.
    #[test]
    fn a_section_flattens_to_dotted_keys() {
        let path = layer(
            "section",
            "[ui]\ncolor = \"auto\"\nshow_hardware_cursor = true\n",
        );
        let values = read_layer(&path).expect("the file exists");
        assert_eq!(values.get("ui.color").map(String::as_str), Some("auto"));
        assert_eq!(
            values.get("ui.show_hardware_cursor").map(String::as_str),
            Some("true")
        );
    }

    // Verifies: gh #397 - **the inherit state**, which is the one the old
    // two-way toggle could not express: a key that is simply not in the file.
    #[test]
    fn an_absent_key_is_absent_not_false() {
        let path = layer("absent", "ui.color = \"auto\"\n");
        let values = read_layer(&path).expect("the file exists");
        assert!(values.contains_key("ui.color"));
        assert_eq!(
            values.get("ui.quiet_startup"),
            None,
            "inheriting is the absence of the key, which is not `false`"
        );
    }

    // Verifies: gh #397 - a missing file is `None`, distinct from an empty
    // one: the writer uses the difference to decide whether it created it.
    #[test]
    fn a_missing_file_is_none() {
        let path = std::env::temp_dir().join("lca-layer-does-not-exist-397.toml");
        let _ = std::fs::remove_file(&path);
        assert!(read_layer(&path).is_none());
    }

    // Verifies: gh #397 - an unparseable layer reads as absent here rather
    // than panicking or half-reporting: explaining broken TOML belongs to the
    // loader, not to a settings row.
    #[test]
    fn a_broken_file_does_not_panic() {
        let path = layer("broken", "this is not toml = = =\n");
        assert!(read_layer(&path).is_none());
    }
}
