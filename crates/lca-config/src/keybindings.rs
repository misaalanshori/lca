//! User keybindings file loading (gh #66), split from the main
//! configuration module at the file-size ceiling (gate 11).

use std::collections::BTreeMap;
use std::path::Path;

use super::{ConfigError, read_table, type_name};

/// Load a user keybindings file (gh #66): action names to one key, a
/// list of keys, or an empty list (which disables the action). A value
/// of any other shape names its action in the error, so a typo fails
/// loud instead of silently doing nothing.
///
/// # Errors
///
/// Returns [`ConfigError`] when the file cannot be read, is not valid
/// TOML, or holds a non-string entry.
pub fn load_keybindings_file(path: &Path) -> Result<BTreeMap<String, Vec<String>>, ConfigError> {
    const LABEL: &str = "user keybindings";
    let table = read_table(path, LABEL)?;
    let mut bindings = BTreeMap::new();
    flatten_keybindings(&table, String::new(), LABEL, &mut bindings)?;
    Ok(bindings)
}

/// Flatten one TOML level: dotted keys (`app.clear = …`) and section
/// tables (`[app]`) nest identically, so both spell the same action.
fn flatten_keybindings(
    table: &toml::Table,
    prefix: String,
    label: &str,
    bindings: &mut BTreeMap<String, Vec<String>>,
) -> Result<(), ConfigError> {
    for (key, value) in table {
        let action = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        match value {
            toml::Value::Table(nested) => {
                flatten_keybindings(nested, action, label, bindings)?;
            }
            toml::Value::String(single) => {
                bindings.insert(action, vec![single.clone()]);
            }
            toml::Value::Array(keys) => {
                let keys = keys
                    .iter()
                    .enumerate()
                    .map(|(index, key)| match key {
                        toml::Value::String(key) => Ok(key.clone()),
                        other => Err(ConfigError::InvalidValue {
                            key: action.clone(),
                            label: label.to_string(),
                            reason: format!(
                                "entry {index} is {}, expected a key string",
                                type_name(other)
                            ),
                        }),
                    })
                    .collect::<Result<Vec<String>, ConfigError>>()?;
                bindings.insert(action, keys);
            }
            other => {
                return Err(ConfigError::InvalidValue {
                    key: action.clone(),
                    label: label.to_string(),
                    reason: format!(
                        "expected a key string or a list of key strings, got {}",
                        type_name(other)
                    ),
                });
            }
        }
    }
    Ok(())
}
