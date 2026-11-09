//! Per-project package resource selection (gh #59, ceiling split
//! from `ext.rs`): `.lca/extensions.toml` reads/writes and the `ext
//! config` show/set seam. `ext.rs` keeps the subcommand and dispatch.

use super::not_installed;
use lca_registry::InstallTree;

/// One package's resource selection file (gh #59): project-scoped
/// `.lca/extensions.toml`, `[extensions."<name>"] resources = [...]`.
/// Missing file, missing package, or malformed TOML all mean
/// unfiltered (an optional file must not break discovery).
fn selection_path(project: &std::path::Path) -> std::path::PathBuf {
    project.join(".lca").join("extensions.toml")
}

/// The raw rule strings for `name`, or empty when unfiltered.
pub(crate) fn read_selection(project: &std::path::Path, name: &str) -> Vec<String> {
    let text = std::fs::read_to_string(selection_path(project)).unwrap_or_default();
    if text.trim().is_empty() {
        return Vec::new();
    }
    text.parse::<toml::Value>()
        .ok()
        .and_then(|value| {
            value
                .get("extensions")?
                .get(name)?
                .get("resources")?
                .as_array()
                .cloned()
        })
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Every package's raw rule strings for a project (gh #59): the map
/// the skill roots builder parses into selections.
pub(crate) fn read_all_selections(
    project: &std::path::Path,
) -> std::collections::BTreeMap<String, Vec<String>> {
    let text = std::fs::read_to_string(selection_path(project)).unwrap_or_default();
    if text.trim().is_empty() {
        return Default::default();
    }
    text.parse::<toml::Value>()
        .ok()
        .and_then(|value| value.get("extensions")?.as_table().cloned())
        .map(|tables| {
            tables
                .into_iter()
                .filter_map(|(name, table)| {
                    table.get("resources")?.as_array().cloned().map(|items| {
                        (
                            name,
                            items
                                .iter()
                                .filter_map(|item| item.as_str().map(str::to_string))
                                .collect(),
                        )
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Show or set one installed package's resource selection (gh #59):
/// the working directory owns the project scope. Unknown packages
/// refuse like the other project-scoped commands do.
pub(crate) fn config(tree: &InstallTree, name: &str, filters: &[String]) -> i32 {
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(err) => {
            eprintln!("error: cannot read the working directory: {err}");
            return crate::exit::INTERNAL;
        }
    };
    match tree.entry(name) {
        Ok(Some(_)) => {}
        Ok(None) => {
            not_installed(tree, name);
            return crate::exit::USAGE;
        }
        Err(err) => {
            eprintln!("error: {err}");
            return crate::exit::INTERNAL;
        }
    }
    config_in(&cwd, name, filters)
}

/// Show (`filters` empty) or replace (`filters` given) one package's
/// resource selection, validating rules before anything is written.
/// The testable core of `ExtCmd::Config`: the project is injected.
pub(crate) fn config_in(project: &std::path::Path, name: &str, filters: &[String]) -> i32 {
    if filters.is_empty() {
        match read_selection(project, name) {
            rules if rules.is_empty() => {
                println!("{name}: no selection (everything loads)")
            }
            rules => {
                println!("{name} resource selection:");
                for rule in rules {
                    println!("  {rule}");
                }
            }
        }
        return crate::exit::OK;
    }
    if let Err(err) = lca_tools::skills::parse_resource_selection(filters) {
        eprintln!("error: {err}");
        return crate::exit::USAGE;
    }
    let path = selection_path(project);
    let mut value: toml::Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| text.parse().ok())
        .unwrap_or(toml::Value::Table(Default::default()));
    if !value.is_table() {
        value = toml::Value::Table(Default::default());
    }
    let Some(table) = value.as_table_mut() else {
        return crate::exit::INTERNAL;
    };
    let extensions = table
        .entry("extensions".to_string())
        .or_insert_with(|| toml::Value::Table(Default::default()));
    if !extensions.is_table() {
        *extensions = toml::Value::Table(Default::default());
    }
    let Some(extensions) = extensions.as_table_mut() else {
        return crate::exit::INTERNAL;
    };
    extensions.insert(
        name.to_string(),
        toml::Value::Table(
            [(
                "resources".to_string(),
                toml::Value::Array(
                    filters
                        .iter()
                        .map(|rule| toml::Value::String(rule.clone()))
                        .collect(),
                ),
            )]
            .into_iter()
            .collect(),
        ),
    );
    let text = toml::to_string_pretty(&value).unwrap_or_default();
    if let Some(parent) = path.parent()
        && std::fs::create_dir_all(parent).is_err()
    {
        eprintln!("error: cannot write {}", path.display());
        return crate::exit::INTERNAL;
    }
    match std::fs::write(&path, text) {
        Ok(()) => {
            println!(
                "{name}: {} resource rule{} set",
                filters.len(),
                if filters.len() == 1 { "" } else { "s" }
            );
            crate::exit::OK
        }
        Err(err) => {
            eprintln!("error: cannot write {}: {err}", path.display());
            crate::exit::INTERNAL
        }
    }
}
