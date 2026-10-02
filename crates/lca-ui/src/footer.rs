//! The footer, ported from pi's
//! `coding-agent/src/modes/interactive/components/footer.ts`
//! (`pi-tui-re/src_re/agent-components/chrome.md` §1).
//!
//! Owner issue #9: the footer shows the working directory, `~`-shortened,
//! with the git branch. Below it, the session stats (tokens, cache, cost)
//! and the active model with its context use.

use std::path::Path;

use lca_protocol::Usage;
use lca_tui::engine::text::{truncate_to_width, visible_width};

use crate::theme::{Role, StyleFn, Theme};

/// The footer's data.
#[derive(Debug, Clone, Default)]
pub struct Footer {
    /// The working directory.
    pub cwd: String,
    /// The session title.
    pub session: String,
    /// The active model label (`provider/model`).
    pub model: String,
    /// The session's thinking level (`thinking`, R1); `None` when unset
    /// (the provider's own default).
    pub thinking: Option<String>,
    /// Accumulated usage.
    pub usage: Usage,
    /// The model's context window in tokens (0 = unknown).
    pub context_window: u64,
    /// Tokens currently in context (0 = unknown).
    pub context_used: u64,
    /// Extension-provided status segments.
    pub statuses: Vec<String>,
    /// The last finished turn's generation speed, in output tokens per
    /// second of streaming time (the owner's "can we add tok/s to the
    /// footer?" ask). `None` before the first measurable turn.
    pub tok_s: Option<u64>,
    /// Yolo mode is on: the marker is persistent and loud (ADR-0042).
    pub yolo: bool,
}

/// A token count in pi's compact form: `999`, `2.6k`, `1.2M` - the
/// owner's own paste showed `↑121194`, which no one reads at a glance.
pub fn compact(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1000..=999_999 => format!("{:.1}k", f64::from(n as u32) / 1_000.0),
        _ => format!("{:.1}M", n as f64 / 1_000_000.0),
    }
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
    /// The style a thinking level wears: its own `thinking*` role, or the
    /// footer's muted color for a level outside the vocabulary.
    fn thinking_style(theme: &Theme, level: &str) -> StyleFn {
        let role = match level {
            "off" => Role::ThinkingOff,
            "minimal" => Role::ThinkingMinimal,
            "low" => Role::ThinkingLow,
            "medium" => Role::ThinkingMedium,
            "high" => Role::ThinkingHigh,
            "xhigh" => Role::ThinkingXhigh,
            "max" => Role::ThinkingMax,
            _ => return theme.footer.clone(),
        };
        theme.role(role)
    }

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
        // ADR-0042: yolo is never quiet. Its own line, error role, every
        // frame, for as long as the mode is on.
        if self.yolo {
            lines.push(truncate_to_width(
                &(theme.error)("YOLO: every permission prompt auto-approved (--yolo)"),
                width,
                "…",
                false,
            ));
        }

        // Stats line.
        let u = &self.usage;
        let cache = u.cache_read + u.cache_write;
        // One decimal, pi's precision (`CH60.9%`): a whole percent hides
        // the difference between 89 and 91, which is what the number is
        // there to show (owner issue: "is our cache hit % rounded to full
        // integer? Pi has at least once decimal precision").
        let cache_pct = if u.input + cache > 0 {
            (cache as f64 / (u.input + cache) as f64 * 100.0 * 10.0).round() / 10.0
        } else {
            0.0
        };
        let mut stats = String::new();
        let muted = theme.footer.clone();
        stats.push_str(&muted(&format!(
            "↑{} ↓{} R{} W{} {cache_pct:.1}%",
            compact(u.input),
            compact(u.output),
            compact(u.cache_read),
            compact(u.cache_write)
        )));
        if let Some(rate) = self.tok_s {
            stats.push_str(&muted(" • "));
            stats.push_str(&muted(&format!("{rate} tok/s")));
        }
        if u.cost > 0.0 {
            stats.push_str(&muted(&format!(" • ${:.4}", u.cost)));
        }
        if !self.model.is_empty() {
            stats.push_str(&muted(" • "));
            stats.push_str(&muted(&self.model));
        }
        if let Some(thinking) = &self.thinking {
            // pi colors the thinking level with its own role (the seven
            // `thinking*` tokens) - a level you can see at a glance.
            stats.push_str(&muted(" • "));
            stats.push_str(&Self::thinking_style(theme, thinking)(thinking));
        }
        if self.context_window > 0 {
            let pct =
                (self.context_used as f64 / self.context_window as f64 * 100.0).round() as u64;
            // chrome.md §1: the context share is colorized by threshold -
            // over 90% error, over 70% warning.
            let share = theme.role(if pct > 90 {
                Role::Error
            } else if pct > 70 {
                Role::Warning
            } else {
                Role::Muted
            });
            stats.push_str(&muted(" • ctx "));
            stats.push_str(&share(&format!("{pct}%")));
        } else {
            // E4: no known window is an honest unknown, never `0%`.
            stats.push_str(&muted(" • ctx ?"));
        }
        lines.push(truncate_to_width(&stats, width, "…", false));

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
        // One decimal, pi's precision (the owner asked: "is our cache hit
        // % rounded to full integer? Pi has at least once decimal").
        assert!(out[1].contains("90.0%"), "{}", out[1]);
        assert!(out[1].contains("$0.5000"));
        assert!(out[1].contains("ctx 25%"));
    }

    // Verifies: FR-UI-20's numbers in pi's compact form (the owner's own
    // paste showed `↑121194`).
    #[test]
    fn token_counts_render_compact() {
        assert_eq!(compact(0), "0");
        assert_eq!(compact(999), "999");
        assert_eq!(compact(1_000), "1.0k");
        assert_eq!(compact(2_640), "2.6k");
        assert_eq!(compact(121_194), "121.2k");
        assert_eq!(compact(1_300_000), "1.3M");
    }

    // Verifies: the owner's "can we add tok/s to the footer?" ask - the
    // last turn's generation speed rides the stats line when there is one,
    // and no number is invented before the first measurable turn.
    #[test]
    fn the_footer_shows_the_last_turn_tokens_per_second() {
        let f = Footer {
            usage: Usage {
                input: 10,
                output: 40,
                ..Default::default()
            },
            tok_s: Some(142),
            ..Default::default()
        };
        let out = strip(&f.render(140, &Theme::plain()));
        assert!(out[1].contains("142 tok/s"), "{}", out[1]);
        let quiet = strip(&Footer::default().render(140, &Theme::plain()));
        assert!(!quiet[1].contains("tok/s"), "{}", quiet[1]);
    }

    // Verifies: FR-UI-20 (E4) - an unknown context window reads `ctx ?`,
    // never a fabricated `0%`.
    #[test]
    fn stats_line_reports_an_unknown_context_window() {
        let f = Footer {
            model: "p/m".into(),
            context_window: 0,
            context_used: 0,
            ..Default::default()
        };
        let out = strip(&f.render(120, &Theme::plain()));
        assert!(out[1].contains("ctx ?"), "{}", out[1]);
        assert!(!out[1].contains("ctx 0%"), "{}", out[1]);
    }

    #[test]
    fn stats_line_reports_the_thinking_level() {
        let f = Footer {
            model: "p/m".into(),
            thinking: Some("high".into()),
            ..Default::default()
        };
        let out = strip(&f.render(120, &Theme::plain()));
        assert!(out[1].contains("p/m • high"), "{}", out[1]);
    }

    // Verifies: FR-PERM-26 (ADR-0042) - yolo is loud: its own footer line, error role,
    // every frame while the mode is on, and absent when it is off.
    #[test]
    fn the_footer_shows_the_yolo_marker_only_when_yolo_is_on() {
        let theme = Theme::plain();
        let mut footer = Footer {
            cwd: "/w".to_string(),
            yolo: true,
            ..Default::default()
        };
        let lines = footer.render(80, &theme);
        assert!(
            lines
                .iter()
                .any(|line| line.contains("YOLO: every permission prompt auto-approved")),
            "{lines:?}"
        );
        footer.yolo = false;
        let lines = footer.render(80, &theme);
        assert!(!lines.iter().any(|line| line.contains("YOLO")), "{lines:?}");
    }
}
