//! The footer, ported from pi's
//! `coding-agent/src/modes/interactive/components/footer.ts`
//! (`pi-tui-re/src_re/agent-components/chrome.md` §1).
//!
//! Owner issue #9: the footer shows the working directory, `~`-shortened,
//! with the git branch. Below it, the session stats (tokens, cache, cost)
//! and the active model with its context use.

use std::fmt::Write as _;
use std::path::Path;

use lca_protocol::Usage;
use lca_tui::engine::text::{truncate_to_width, visible_width};

use crate::theme::Theme;

/// The footer's data.
#[derive(Debug, Clone, Default)]
pub struct Footer {
    /// The working directory.
    pub cwd: String,
    /// The session title.
    pub session: String,
    /// The active model label (`provider/model`).
    pub model: String,
    /// Accumulated usage.
    pub usage: Usage,
    /// The model's context window in tokens (0 = unknown).
    pub context_window: u64,
    /// Tokens currently in context (0 = unknown).
    pub context_used: u64,
    /// Extension-provided status segments.
    pub statuses: Vec<String>,
}

/// Shorten a path under `home` to `~`.
pub fn shorten_home(path: &str, home: Option<&str>) -> String {
    if let Some(home) = home
        && !home.is_empty()
        && let Some(rest) = path.strip_prefix(home)
    {
        if rest.is_empty() {
            return "~".to_string();
        }
        if let Some(rest) = rest.strip_prefix('/') {
            return format!("~/{rest}");
        }
        if let Some(rest) = rest.strip_prefix('\\') {
            return format!("~\\{rest}");
        }
    }
    path.to_string()
}

/// The current git branch, if the directory is in a work tree.
pub fn git_branch(dir: &str) -> Option<String> {
    let git = Path::new(dir).join(".git");
    let head = if git.is_dir() {
        std::fs::read_to_string(git.join("HEAD")).ok()?
    } else if git.is_file() {
        // Worktree/submodule: `.git` is a file `gitdir: <path>`.
        let content = std::fs::read_to_string(&git).ok()?;
        let gitdir = content.strip_prefix("gitdir:")?.trim();
        let gitdir_path = if Path::new(gitdir).is_absolute() {
            std::path::PathBuf::from(gitdir)
        } else {
            Path::new(dir).join(gitdir)
        };
        std::fs::read_to_string(gitdir_path.join("HEAD")).ok()?
    } else {
        return None;
    };
    let head = head.trim();
    head.strip_prefix("ref: refs/heads/")
        .map(|b| b.to_string())
        .or_else(|| {
            // Detached HEAD: a short commit hash.
            head.strip_prefix("ref: ").map(|_| head.to_string())
        })
}

impl Footer {
    /// Render the footer to lines.
    pub fn render(&self, width: u16, theme: &Theme) -> Vec<String> {
        let width = width as usize;
        let home = std::env::var("HOME").ok();
        let cwd = shorten_home(&self.cwd, home.as_deref());
        let mut location = (theme.accent)(&cwd);
        if let Some(branch) = git_branch(&self.cwd) {
            location.push_str(&(theme.footer)(" ("));
            location.push_str(&(theme.accent)(&branch));
            location.push_str(&(theme.footer)(")"));
        }
        if !self.session.is_empty() {
            location.push_str(&(theme.footer)(" • "));
            location.push_str(&(theme.footer)(&self.session));
        }
        let mut lines = vec![truncate_to_width(&location, width, "…", false)];

        // Stats line.
        let u = &self.usage;
        let cache = u.cache_read + u.cache_write;
        let cache_pct = if u.input + cache > 0 {
            (cache as f64 / (u.input + cache) as f64 * 100.0).round() as u64
        } else {
            0
        };
        let mut stats = format!(
            "↑{} ↓{} R{} W{} {cache_pct}%",
            u.input, u.output, u.cache_read, u.cache_write
        );
        if u.cost > 0.0 {
            let _ = write!(stats, " • ${:.4}", u.cost);
        }
        if !self.model.is_empty() {
            stats.push_str(" • ");
            stats.push_str(&self.model);
        }
        if self.context_window > 0 {
            let pct =
                (self.context_used as f64 / self.context_window as f64 * 100.0).round() as u64;
            let _ = write!(stats, " • ctx {pct}%");
        }
        lines.push(truncate_to_width(
            &(theme.footer)(&stats),
            width,
            "…",
            false,
        ));

        if !self.statuses.is_empty() {
            let joined = self.statuses.join(" • ");
            lines.push(truncate_to_width(
                &(theme.footer)(&joined),
                width,
                "…",
                false,
            ));
        }
        lines
    }

    /// The footer's height at a width (1-3 lines).
    pub fn height(&self, width: u16, theme: &Theme) -> usize {
        self.render(width, theme).len()
    }

    /// The visible width of the widest line.
    pub fn max_width(&self, theme: &Theme) -> usize {
        self.render(200, theme)
            .iter()
            .map(|l| visible_width(l))
            .max()
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lca_tui::engine::text::strip_terminal_sequences;

    fn strip(lines: &[String]) -> Vec<String> {
        lines.iter().map(|l| strip_terminal_sequences(l)).collect()
    }

    #[test]
    fn cwd_is_shortened_under_home() {
        assert_eq!(shorten_home("/home/u/proj", Some("/home/u")), "~/proj");
        assert_eq!(shorten_home("/home/u", Some("/home/u")), "~");
        assert_eq!(shorten_home("/etc", Some("/home/u")), "/etc");
    }

    #[test]
    fn footer_shows_the_cwd() {
        let f = Footer {
            cwd: "/tmp/proj".into(),
            model: "openai-compatible/gpt".into(),
            ..Default::default()
        };
        let out = strip(&f.render(80, &Theme::plain()));
        assert!(out[0].contains("/tmp/proj"));
        assert!(out[1].contains("openai-compatible/gpt"));
    }

    #[test]
    fn git_branch_is_read_from_head() {
        let dir = std::env::temp_dir().join(format!("lca-footer-{}", std::process::id()));
        let git = dir.join(".git");
        std::fs::create_dir_all(&git).unwrap();
        std::fs::write(git.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        assert_eq!(git_branch(&dir.to_string_lossy()), Some("main".to_string()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stats_line_reports_tokens_and_cache() {
        let f = Footer {
            usage: Usage {
                input: 100,
                output: 10,
                cache_read: 900,
                cache_write: 0,
                cost: 0.5,
                ..Default::default()
            },
            context_window: 1000,
            context_used: 250,
            ..Default::default()
        };
        let out = strip(&f.render(120, &Theme::plain()));
        assert!(out[1].contains("↑100"));
        assert!(out[1].contains("R900"));
        assert!(out[1].contains("90%"));
        assert!(out[1].contains("$0.5000"));
        assert!(out[1].contains("ctx 25%"));
    }
}
