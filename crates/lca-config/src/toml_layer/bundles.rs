//! Key family: the two bundled collections, `[personas]` and
//! `extensions.catalogs`.
//!
//! Gh #545 phase 2: moved out of `apply_toml_layer`'s one 612-line closure.
//! Both arms validate *shape* only - whether a bundle may apply is a session
//! fact (the model, the level, the tool registry, and the project's trust),
//! which belongs to the caller.

use crate::type_name;
use crate::{ConfigError, TypedValue};

/// The keys this family owns. `toml_layer/mod.rs`'s dispatch table routes each of
/// them here, and `tests/keys_agree.rs` pins that the two lists name one set.
pub(super) const KEYS: &[&str] = &["personas", "extensions.catalogs"];

/// Type one of [`KEYS`]. The arms are the ones the closure held, verbatim except
/// that `this.apply(...)` became the return: the dispatch in `toml_layer/mod.rs`
/// is the single place that applies a typed value.
pub(super) fn parse(
    key: &str,
    value: &toml::Value,
    label: &str,
) -> Result<TypedValue, ConfigError> {
    let invalid = |reason: String| ConfigError::InvalidValue {
        key: key.to_string(),
        label: label.to_string(),
        reason,
    };
    match key {
        "personas" => {
            // Gh #412: shape-validated here; whether one may *apply* is a
            // session fact (the model, the level for that model, the tool
            // registry) and an untrusted project's personas never apply at
            // all - both belong to the caller, not the loader.
            let personas = crate::persona::parse_personas(value).map_err(&invalid)?;
            Ok(TypedValue::Personas(personas))
        }
        "extensions.catalogs" => {
            let list = value.as_array().ok_or_else(|| {
                invalid(format!(
                    "expected a list of catalog tables, got {}",
                    type_name(value)
                ))
            })?;
            let mut catalogs = Vec::new();
            for entry in list {
                let table = entry.as_table().ok_or_else(|| {
                    invalid(format!(
                        "each catalog must be a table, got {}",
                        type_name(entry)
                    ))
                })?;
                let field = |name: &str| {
                    table
                        .get(name)
                        .and_then(|value| value.as_str())
                        .map(str::to_string)
                        .ok_or_else(|| invalid(format!("each catalog needs `{name}`")))
                };
                let id = field("id")?;
                // Gh #348: the id names a cache file, and a
                // *trusted* project file can set it (FR-PERM-9
                // gates the file, not this field), so it is
                // untrusted input joined onto a path. Refusing
                // here means a hostile configuration never gets
                // as far as a refresh that would write outside
                // the cache directory. The same rule lives in
                // `lca_registry::is_valid_catalog_id` (this crate
                // has no dependency on the registry); a
                // regression row pins the two together.
                if !crate::keys::is_valid_catalog_id(&id) {
                    return Err(invalid(format!(
                        "catalog id `{id}`: {}",
                        crate::keys::CATALOG_ID_RULE
                    )));
                }
                catalogs.push(crate::keys::CustomCatalog {
                    id,
                    name: field("name")?,
                    url: field("url")?,
                });
            }
            Ok(TypedValue::Catalogs(catalogs))
        }
        // Unreachable by construction - `KEYS` names exactly the arms above, and
        // the dispatch routes only those keys here (`tests/keys_agree.rs` pins the
        // two lists against each other) - but it carries the closure's own copy so
        // that a mis-route reports what an unknown key always has, rather than a
        // message nobody has ever seen.
        _ => Err(invalid("unknown configuration key".to_string())),
    }
}
