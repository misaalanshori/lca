//! The startup header (gh #131), split from `mod.rs` (the
//! 1,200-line ceiling): version line plus loaded-resource listing,
//! with the `ui.quiet_startup` layers.

/// The startup header lines (gh #131, pi's header shape): the version
/// line, then the loaded resources (context paths that exist, plus
/// extension/skill/template counts). `quiet` follows `ui.quiet_startup`:
/// `"false"` shows all, `"header"` keeps the version line only,
/// `"true"` hides everything. Pure for the test rows.
pub(super) fn startup_header(
    product: &str,
    abi: &str,
    quiet: &str,
    context: &[std::path::PathBuf],
    extensions: usize,
    skills: usize,
    templates: usize,
) -> Vec<String> {
    if quiet == "true" {
        return Vec::new();
    }
    let mut lines = vec![format!("lca {product} (abi {abi})")];
    if quiet == "header" {
        return lines;
    }
    if !context.is_empty() {
        let paths = context
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(format!("context: {paths}"));
    }
    lines.push(format!(
        "{extensions} {}, {skills} {}, {templates} {}",
        plural("extension", extensions),
        plural("skill", skills),
        plural("template", templates),
    ));
    lines
}

/// `1 extension` / `2 extensions`.
fn plural(word: &str, count: usize) -> String {
    if count == 1 {
        word.to_string()
    } else {
        format!("{word}s")
    }
}

#[cfg(test)]
mod header_tests {
    use super::startup_header;

    fn paths(names: &[&str]) -> Vec<std::path::PathBuf> {
        names.iter().map(std::path::PathBuf::from).collect()
    }

    // Verifies: gh #131 - the header lists AGENTS.md paths when present,
    // with the resource counts behind them.
    #[test]
    fn the_header_lists_context_paths_and_counts() {
        let lines = startup_header(
            "0.6.0",
            "0.6",
            "false",
            &paths(&["/home/u/.lca/AGENTS.md", "/w/.lca/AGENTS.md"]),
            3,
            2,
            1,
        );
        let text = lines.join("\n");
        assert!(
            text.contains("lca 0.6.0") && text.contains("abi 0.6"),
            "version line first: {text}"
        );
        assert!(
            text.contains("/home/u/.lca/AGENTS.md") && text.contains("/w/.lca/AGENTS.md"),
            "both context paths list: {text}"
        );
        assert!(
            text.contains("3 extensions")
                && text.contains("2 skills")
                && text.contains("1 template"),
            "counts follow: {text}"
        );
    }

    // Verifies: gh #131 - quietStartup hides in layers: "header" keeps
    // the version line only, true hides everything.
    #[test]
    fn quiet_startup_hides_in_layers() {
        let context = paths(&["/home/u/.lca/AGENTS.md"]);
        let header = startup_header("0.6.0", "0.6", "header", &context, 1, 1, 1).join("\n");
        assert!(header.contains("lca 0.6.0"), "version stays: {header}");
        assert!(!header.contains("AGENTS.md"), "resources hide: {header}");
        assert!(
            startup_header("0.6.0", "0.6", "true", &context, 1, 1, 1).is_empty(),
            "true hides everything"
        );
        assert!(
            startup_header("0.6.0", "0.6", "false", &[], 0, 0, 0)
                .join("\n")
                .contains("lca 0.6.0"),
            "an empty resource set still headers"
        );
    }
}
