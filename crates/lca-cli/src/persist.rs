//! Config-file setting persistence: the write path the `/theme`,
//! `/thinking`, and `/settings` surfaces share. One dotted key
//! walks or creates its TOML tables; `None` removes the leaf.

/// Persist one setting to the user config file (`~/.lca/config.toml`),
/// preserving every other key and its comments. `None` removes the key, which
/// is how the `/thinking` picker's `unset` is written. This is the write path
/// the `/theme` and `/thinking` pickers share (E2); `ui.fullscreen` keeps its
/// separate `ui.json` state file so a toggle still never rewrites user config.
pub fn persist_setting(key: &str, value: Option<&str>) -> std::io::Result<()> {
    persist_setting_at(&crate::config_file(), key, value)
}

/// [`persist_setting`] against an explicit path (the testable half).
fn persist_setting_at(
    path: &std::path::Path,
    key: &str,
    value: Option<&str>,
) -> std::io::Result<()> {
    use toml_edit::{DocumentMut, value as toml_value};
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let mut doc = text
        .parse::<DocumentMut>()
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err.to_string()))?;
    // Walk every segment (#155): each leading segment names a table
    // that is created on the way down, so `a.b.c = v` nests instead of
    // joining the tail into one flat key. A segment holding a plain
    // value is replaced by a table - a dotted key claims structure.
    let mut segments = key.split('.');
    let leaf = segments.next_back().unwrap_or(key);
    let mut table = doc.as_table_mut();
    for segment in segments {
        let needs_table = !table.get(segment).is_some_and(|item| item.is_table_like());
        if needs_table {
            table[segment] = toml_edit::Item::Table(toml_edit::Table::new());
        }
        // toml_edit has no entry API on Item, so the ensured table is
        // re-borrowed by index.
        #[allow(clippy::expect_used)] // the segment was just ensured as a table above.
        {
            table = table[segment]
                .as_table_mut()
                .expect("segment just ensured as a table");
        }
    }
    match value {
        Some(value) => table[leaf] = toml_value(value),
        None => {
            table.remove(leaf);
        }
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Atomic publish: a sibling temp file, renamed over the target.
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, doc.to_string())?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Verifies: E2 - a persisted `/thinking` value survives a reload (the
    // restart agreement) and the writer preserves other keys and comments;
    // `None` removes the key.
    #[test]
    fn persist_setting_round_trips_and_removes() {
        let root = lca_testkit::scratch_path("lca-persist-setting");
        let path = root.join("config.toml");
        std::fs::create_dir_all(&root).expect("mkdir");
        std::fs::write(&path, "# a comment\nprovider = \"x\"\n").expect("seed");
        persist_setting_at(&path, "thinking", Some("high")).expect("write");
        let text = std::fs::read_to_string(&path).expect("read");
        assert!(text.contains("# a comment"), "comments survive: {text}");
        assert!(text.contains("provider = \"x\""), "keys survive: {text}");
        let input = lca_config::LoadInput {
            user_file: Some(path.clone()),
            ..Default::default()
        };
        let loaded = lca_config::Config::load(&input).expect("load");
        assert_eq!(loaded.thinking(), Some("high"));
        persist_setting_at(&path, "thinking", None).expect("remove");
        let input = lca_config::LoadInput {
            user_file: Some(path.clone()),
            ..Default::default()
        };
        assert_eq!(
            lca_config::Config::load(&input).expect("load").thinking(),
            None
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    // Verifies: E2 - a nested `ui.theme` write lands in section form and
    // round-trips through the loader.
    #[test]
    fn persist_setting_writes_a_nested_key() {
        let root = lca_testkit::scratch_path("lca-persist-theme");
        let path = root.join("config.toml");
        std::fs::create_dir_all(&root).expect("mkdir");
        persist_setting_at(&path, "ui.theme", Some("light")).expect("write");
        let input = lca_config::LoadInput {
            user_file: Some(path.clone()),
            ..Default::default()
        };
        assert_eq!(
            lca_config::Config::load(&input).expect("load").ui_theme(),
            Some("light")
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    // Verifies: #155 - a three-level write nests tables instead of
    // joining the tail into one flat key.
    #[test]
    fn persist_setting_writes_a_three_level_key_as_nested_tables() {
        let root = lca_testkit::scratch_path("lca-persist-deep");
        let path = root.join("config.toml");
        std::fs::create_dir_all(&root).expect("mkdir");
        persist_setting_at(&path, "retry.provider.timeoutMs", Some("5000")).expect("write");
        let text = std::fs::read_to_string(&path).expect("read");
        assert!(
            !text.contains("provider.timeoutMs"),
            "no flat dotted key survives: {text}"
        );
        let doc = text.parse::<toml_edit::DocumentMut>().expect("parse");
        assert_eq!(doc["retry"]["provider"]["timeoutMs"].as_str(), Some("5000"));
        let _ = std::fs::remove_dir_all(&root);
    }

    // Verifies: #155 - a deep write keeps the siblings that already
    // live in the table.
    #[test]
    fn persist_setting_keeps_siblings_when_writing_deep() {
        let root = lca_testkit::scratch_path("lca-persist-siblings");
        let path = root.join("config.toml");
        std::fs::create_dir_all(&root).expect("mkdir");
        std::fs::write(
            &path,
            "[tool]\nresult_limit_bytes = 100\ntimeout_seconds = 30\n",
        )
        .expect("seed");
        persist_setting_at(&path, "tool.timeout_seconds", Some("60")).expect("write");
        // The writer is string-typed by contract, so siblings are
        // asserted at the TOML level, not through the typed loader.
        let text = std::fs::read_to_string(&path).expect("read");
        let doc = text.parse::<toml_edit::DocumentMut>().expect("parse");
        assert_eq!(doc["tool"]["timeout_seconds"].as_str(), Some("60"));
        assert_eq!(doc["tool"]["result_limit_bytes"].as_integer(), Some(100));
        let _ = std::fs::remove_dir_all(&root);
    }

    // Verifies: #155 (review) - a deep write round-trips through the
    // real loader with the sibling intact: value and neighbor both
    // survive write plus reload.
    #[test]
    fn persist_setting_reloads_a_deep_write_with_siblings_intact() {
        let root = lca_testkit::scratch_path("lca-persist-reload");
        let path = root.join("config.toml");
        std::fs::create_dir_all(&root).expect("mkdir");
        std::fs::write(&path, "[shell]\ntool = \"bash\"\n").expect("seed");
        persist_setting_at(&path, "shell.path", Some("/bin/bash")).expect("write");
        let input = lca_config::LoadInput {
            user_file: Some(path.clone()),
            ..Default::default()
        };
        let loaded = lca_config::Config::load(&input).expect("load");
        assert_eq!(loaded.shell_path(), Some("/bin/bash"));
        assert_eq!(loaded.shell_tool(), Some("bash"));
        let _ = std::fs::remove_dir_all(&root);
    }

    // Verifies: #155 (review) - a dotted key that collides with a
    // plain value replaces the value with the structure the key
    // claims; the flat string does not survive beside the table.
    #[test]
    fn persist_setting_replaces_a_plain_value_blocking_a_deep_write() {
        let root = lca_testkit::scratch_path("lca-persist-collide");
        let path = root.join("config.toml");
        std::fs::create_dir_all(&root).expect("mkdir");
        std::fs::write(&path, "retry = \"flat\"\n").expect("seed");
        persist_setting_at(&path, "retry.provider.timeoutMs", Some("5000")).expect("write");
        let text = std::fs::read_to_string(&path).expect("read");
        let doc = text.parse::<toml_edit::DocumentMut>().expect("parse");
        assert_eq!(doc["retry"]["provider"]["timeoutMs"].as_str(), Some("5000"));
        assert!(
            !text.contains("retry = "),
            "the blocking flat value is gone: {text}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    // Verifies: #155 (review) - unsetting a three-level key removes
    // the leaf and keeps the rest of the table (a two-level unset
    // worked before the walk; the third level is what used to stick).
    #[test]
    fn persist_setting_removes_a_nested_key_and_keeps_the_rest() {
        let root = lca_testkit::scratch_path("lca-persist-unset");
        let path = root.join("config.toml");
        std::fs::create_dir_all(&root).expect("mkdir");
        std::fs::write(
            &path,
            "[retry]\n[retry.provider]\ntimeoutMs = \"5\"\nkind = \"x\"\n",
        )
        .expect("seed");
        persist_setting_at(&path, "retry.provider.timeoutMs", None).expect("unset");
        let text = std::fs::read_to_string(&path).expect("read");
        let doc = text.parse::<toml_edit::DocumentMut>().expect("parse");
        assert!(
            doc["retry"]["provider"].get("timeoutMs").is_none(),
            "the leaf is gone: {text}"
        );
        assert_eq!(doc["retry"]["provider"]["kind"].as_str(), Some("x"));
        let _ = std::fs::remove_dir_all(&root);
    }

    // Verifies: #155 - a dotted key that lands inside an existing
    // table descends into it instead of writing beside it.
    #[test]
    fn persist_setting_descends_into_an_existing_table() {
        let root = lca_testkit::scratch_path("lca-persist-table");
        let path = root.join("config.toml");
        std::fs::create_dir_all(&root).expect("mkdir");
        std::fs::write(&path, "[cache]\nnoise_floor_tokens = 5\n").expect("seed");
        persist_setting_at(&path, "cache.noise_floor_tokens", Some("9")).expect("write");
        let text = std::fs::read_to_string(&path).expect("read");
        assert!(
            !text.contains("noise_floor_tokens\""),
            "no quoted flat key beside the table: {text}"
        );
        let doc = text.parse::<toml_edit::DocumentMut>().expect("parse");
        assert_eq!(doc["cache"]["noise_floor_tokens"].as_str(), Some("9"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
