//! The TOML file layer: reading one TOML table into typed values.
//!
//! **The module map.** One parser per key family, one table that names them,
//! one place that applies: a key is known in exactly one file, and adding a key
//! is one arm plus one entry in that family's `KEYS` - never a third list.
//! That is the whole point of this split (gh #545 phase 2): the closure this
//! replaces knew each key twice (a `match` arm and a string in a 65-entry apply
//! loop) while `refuse_unknown_keys` walked a third list, and the file's own
//! comments recorded the cost twice - "a key missing from either loads as
//! silence" (gh #272). `tests/keys_agree.rs` now makes that mechanical.
//!
//! | family | keys | the rule it owns |
//! |---|---|---|
//! | [`model`] | `provider`, `model`, `models.*`, `thinking`, `thinking.budgets` | which model, at what level |
//! | [`bundles`] | `personas`, `extensions.catalogs` | shape-validated bundles |
//! | [`choices`] | closed lists and documented string shapes | one word, or a documented spelling |
//! | [`counts`] | every integer, count, and fraction | a number within its range |
//! | [`strings`] | `ui.theme`, `shell.path`, `ui.color`, ... | free text, no vocabulary |
//! | [`enums`] | one-word vocabularies | one word from a named set |
//! | [`switches`] | every boolean | a boolean, never a coercion |
//!
//! `permissions.proposals` is the one table key this layer does **not** route:
//! `Config::load` reads it beside the project-trust check it belongs to (a
//! project file's proposals are only honoured for a trusted project, FR-PERM-9),
//! so routing it here would split that rule in two.

mod bundles;
mod choices;
mod counts;
mod enums;
mod model;
mod strings;
mod switches;

use super::is_documented_key;
use super::{Config, ConfigError, MergeSource, TypedValue, label_for, table_value};

/// The single list: every key family this layer applies, with the parser that
/// types its keys.
///
/// **This table is the allowlist.** A key is applied only if a family's
/// [`KEYS`] names it, so there is no second copy to drift from and no third list
/// to forget: adding a key is one arm in a family parser plus that key in the
/// same family's `KEYS`. `tests/keys_agree.rs` pins the union of these lists
/// against `KNOWN_KEYS`/`TABLE_KEYS` and pins that no key has two owners.
///
/// Before the split (gh #545 phase 2) the same vocabulary lived in three places:
/// this 65-entry loop, the `match` arm that typed the value, and the walk that
/// decided what was *known*. The middle one is now the family parser; the other
/// two are one list.
const FAMILIES: &[(&[&str], Parser)] = &[
    (model::KEYS, model::parse),
    (bundles::KEYS, bundles::parse),
    (choices::KEYS, choices::parse),
    (counts::KEYS, counts::parse),
    (strings::KEYS, strings::parse),
    (enums::KEYS, enums::parse),
    (switches::KEYS, switches::parse),
];

/// A family parser: the key it was routed, the TOML value, and the layer's
/// label for the error it may build.
type Parser = fn(&str, &toml::Value, &str) -> Result<TypedValue, ConfigError>;

impl Config {
    pub(super) fn apply_toml_layer(
        &mut self,
        table: &toml::Table,
        source: MergeSource,
    ) -> Result<(), ConfigError> {
        let label = label_for(source);
        // Gh #273: refuse a key this build does not know, before applying
        // anything. This layer looks up a fixed list, so a key outside it was
        // never seen at all - the `other =>` arm was unreachable and a typo
        // loaded clean. A user who writes a setting expects either the setting
        // or an error, never silence.
        refuse_unknown_keys(table, "", label)?;
        for (keys, parse) in FAMILIES {
            for key in keys.iter().copied() {
                let Some(value) = table_value(table, key) else {
                    continue;
                };
                // `provider` doubles as a section: `[provider] retry_limit = N`
                // cannot coexist with the `provider = "name"` leaf in one TOML
                // document, so a table form carries only its children. `thinking`
                // doubles the same way for `[thinking.budgets]`.
                if (key == "provider" || key == "thinking") && value.is_table() {
                    continue;
                }
                let typed = parse(key, value, label)?;
                self.apply(key.to_string(), typed, source)?;
            }
        }
        Ok(())
    }
}

/// Refuse a leaf the configuration vocabulary does not contain (gh #273).
///
/// Walks the parsed table and builds each leaf's dotted path from the keys
/// that lead to it, so a table written in section form (`[ui] colur = ...`)
/// and one written dotted (`ui.colur = ...`) are judged identically. A path
/// is acceptable when it is a documented key, or when it lies *inside* a
/// documented table ([`TABLE_KEYS`]) - those subtrees are user-shaped
/// (level names, model ids, catalog entries) and the key's own parser
/// validates them.
///
/// Descending past an undocumented table name costs nothing and buys a
/// better message: `[net] proxy_url = "..."` reports `net.proxy_url`, the
/// path the user actually typed, instead of the bare `net`. A table whose
/// children are all documented is a section by construction - `path.leaf`
/// can only match a known key if `path` is a real prefix - so no separate
/// section test is needed; only an *empty* undocumented table has to be
/// reported on its own name.
///
/// `label` names the layer in the error, so a project-file refusal says
/// "project file" and not "default".
pub(super) fn refuse_unknown_keys(
    table: &toml::Table,
    prefix: &str,
    label: &str,
) -> Result<(), ConfigError> {
    let unknown = |key: String| ConfigError::UnknownKey {
        key,
        label: label.to_string(),
    };
    for (name, value) in table {
        let path = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}.{name}")
        };
        match value {
            // A documented table's contents are the key's own vocabulary:
            // stop here and let its parser judge (`[thinking.budgets] low =
            // 3000` is not "an unknown level" to this walk).
            toml::Value::Table(_) if crate::keys::TABLE_KEYS.contains(&path.as_str()) => continue,
            toml::Value::Table(children) => {
                // An empty table is legal TOML and changes nothing, so it is
                // accepted when its name heads documented keys (`[models]`
                // with nothing under it). A name that heads nothing is a
                // misspelling of a section, and that is worth reporting.
                if children.is_empty() {
                    if crate::keys::is_section_head(&path) {
                        continue;
                    }
                    return Err(unknown(path));
                }
                refuse_unknown_keys(children, &path, label)?;
            }
            _ => {
                if !is_documented_key(&path) {
                    return Err(unknown(path));
                }
            }
        }
    }
    Ok(())
}
