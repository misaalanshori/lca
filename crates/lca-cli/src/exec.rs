//! Split from `lib.rs` for the workspace file ceiling (gate 11,
//! #158). Behaviour unchanged.

use lca_config::Config;
use lca_tools::NativeOps;

/// The `shell` tool's interpreter for this configuration (ADR-0041): the
/// config's `shell.tool`/`shell.path` through the ladder. `Err` carries the
/// message to surface when a configured interpreter is missing.
pub fn resolve_shell(config: &Config) -> Result<lca_tools::Shell, String> {
    lca_tools::shell::resolve(config.shell_tool().unwrap_or("auto"), config.shell_path())
}

/// The desktop tool backend for this configuration: the resolved shell, or
/// a backend whose every call fails with the resolution error - the
/// "never a silent fallback" half of ADR-0041.
pub fn native_ops(config: &Config) -> NativeOps {
    use lca_tools::ShellProbe as _;
    let fallback = || lca_tools::Shell::fallback(lca_tools::shell::Real.os());
    // Gh #133: the prefix rides the resolved shell, so every command
    // the backend runs carries it, whichever transport applies.
    let prefix = config.shell_command_prefix().map(str::to_string);
    match resolve_shell(config) {
        Ok(mut shell) => {
            shell.command_prefix = prefix;
            NativeOps::new(shell)
        }
        Err(err) => NativeOps::broken(fallback(), err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Verifies: ADR-0041 - a configured interpreter that is missing breaks
    // the backend loudly (every call fails with the resolution message),
    // and the auto ladder always resolves to something runnable.
    #[test]
    fn a_missing_shell_path_breaks_the_backend_loudly() {
        let root = lca_testkit::scratch_path("lca-shell-missing");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("mkdir");
        let file = root.join("config.toml");
        std::fs::write(&file, "shell.path = \"/definitely/not/a/shell\"\n").expect("write");
        let config = Config::load(&lca_config::LoadInput {
            user_file: Some(file),
            ..Default::default()
        })
        .expect("load");
        let ops = native_ops(&config);
        let error = ops.error().expect("the backend is broken");
        assert!(error.contains("does not name a file"), "{error}");

        let auto = native_ops(&Config::defaults());
        assert!(auto.error().is_none(), "auto resolves on every host");
        assert!(!auto.shell().program.is_empty());
    }
}
