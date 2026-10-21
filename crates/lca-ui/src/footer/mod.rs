//! The footer, ported from pi's
//! `coding-agent/src/modes/interactive/components/footer.ts`
//! (`pi-tui-re/src_re/agent-components/chrome.md` §1).
//!
//! Owner issue #9: the footer shows the working directory, `~`-shortened,
//! with the git branch. Below it, the session stats (tokens, cache, cost)
//! and the active model with its context use. Cost policy (issue #17): the
//! cost segment appears once usage has been measured (a measured $0.0000
//! is a free model, informative); before that it stays hidden as noise.

pub mod cache;
pub mod git;

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
    /// The git branch, read on a cadence rather than per frame (gh #389
    /// gap 2). `render` only ever reads this field.
    pub git: cache::GitCache,
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

/// Shorten a path under `home` to `~`, at a **path boundary only**.
///
/// The naive `strip_prefix(home)` shortens `/home/u2/x` to `~2/x` under a
/// `HOME` of `/home/u` — a different user's home rendered as if it were yours,
/// which is both wrong and confusing. Only a true prefix counts: either the
/// whole path, or a prefix followed by a separator.
pub fn shorten_home(path: &str, home: Option<&str>) -> String {
    let Some(home) = home.filter(|home| !home.is_empty()) else {
        return path.to_string();
    };
    let Some(rest) = path.strip_prefix(home) else {
        return path.to_string();
    };
    if rest.is_empty() {
        return "~".to_string();
    }
    if let Some(rest) = rest.strip_prefix('/') {
        return format!("~/{rest}");
    }
    if let Some(rest) = rest.strip_prefix('\\') {
        return format!("~\\{rest}");
    }
    // `$HOME2/x`, `$HOME-old` and friends are not under `$HOME`.
    path.to_string()
}

/// A model label without its `provider/` prefix: the first rung of the
/// degradation ladder's fallback (`p/m` → `m`), unchanged when there is no
/// prefix to drop.
fn bare_model(model: &str) -> &str {
    model.split_once('/').map_or(model, |(_, bare)| bare)
}

/// The home directory to shorten against.
///
/// `HOME` first, then `USERPROFILE`: on native Windows (`pwsh`, `powershell`,
/// `cmd`) `HOME` is unset and `USERPROFILE` holds the real value, so a
/// `HOME`-only lookup leaves every path unshortened there (gh #330 / #389
/// gap 4).
#[must_use]
pub fn home_dir() -> Option<String> {
    ["HOME", "USERPROFILE"]
        .into_iter()
        .filter_map(|key| std::env::var(key).ok())
        .find(|value| !value.is_empty())
}

/// The current git branch, if the directory is in a work tree.
///
/// Walks up to the repository root and handles worktree/submodule indirection,
/// a detached HEAD and `reftable` (gh #389 gap 1). **Not for the frame path**:
/// `Footer::render` reads the cache instead, because this does filesystem I/O
/// and may spawn `git`.
pub fn git_branch(dir: &str) -> Option<String> {
    git::resolve_branch(Path::new(dir), Some(&default_symbolic_ref()))
}

/// The shared `git` backstop used when nothing injected one.
pub(super) fn default_symbolic_ref() -> git::SymbolicRef {
    std::sync::Arc::new(git::symbolic_ref_via_git)
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
        let cwd = shorten_home(&self.cwd, home_dir().as_deref());
        let mut location = (theme.accent)(&cwd);
        // gh #389 gap 2: a field read. The branch is refreshed on the tick's
        // cadence, never here - this function runs on every streaming delta.
        if let Some(branch) = self.git.branch() {
            location.push_str(&(theme.footer)(" ("));
            location.push_str(&(theme.accent)(branch));
            location.push_str(&(theme.footer)(")"));
        }
        if !self.session.is_empty() {
            location.push_str(&(theme.footer)(" • "));
            location.push_str(&(theme.footer)(&self.session));
        }
        // ADR-0042: yolo is never quiet, but it no longer costs a
        // whole row (gh #244): a compact error-role badge rides the
        // location line every frame while the mode is on, with the
        // full explanation one hover away.
        if self.yolo {
            location.push_str(&(theme.footer)(" • "));
            location.push_str(&(theme.error)("[YOLO]"));
        }
        let mut lines = vec![truncate_to_width(&location, width, "…", false)];

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
        let muted = theme.footer.clone();
        // Gh #389 gap 3: **zero-noise.** Each counter appears only when it
        // carries information. A fresh session used to read
        // `↑0 ↓0 R0 W0 0.0%`, which trains people to stop reading the
        // footer - the exact opposite of what an instrument cluster is for.
        // The cache percentage is a *ratio* of the same counters, so it goes
        // when they are all zero too.
        let mut stats_cells: Vec<String> = Vec::new();
        let upto = |label: &str, n: u64| (n > 0).then(|| muted(&format!("{label}{}", compact(n))));
        stats_cells.extend(upto("↑", u.input));
        stats_cells.extend(upto("↓", u.output));
        stats_cells.extend(upto("R", u.cache_read));
        stats_cells.extend(upto("W", u.cache_write));
        // The ratio is shown when there has been cache traffic. A `0.0%` with
        // no cache at all is noise; a `0.0%` *after* cache traffic (an expired
        // prompt cache) is the informative case, and stays.
        if cache > 0 {
            stats_cells.push(muted(&format!("{cache_pct:.1}%")));
        }
        if let Some(rate) = self.tok_s {
            stats_cells.push(muted(&format!("{rate} tok/s")));
        }
        // gh #17: the cost segment appears once usage has been measured -
        // a measured `$0.0000` (a free model) is informative; silence
        // before any usage is noise and stays.
        let measured = u.input + u.output + u.cache_read + u.cache_write + u.cache_write_1h > 0;
        if u.cost > 0.0 || measured {
            stats_cells.push(muted(&format!("${:.4}", u.cost)));
        }

        // The right-hand segments (model, thinking, context) build over a
        // label pi's ladder may shorten first (chrome.md footer.ts
        // "graceful degradation"): full label → drop the provider prefix
        // → drop the context cell → truncate. At 80 columns (NFR-26) the
        // tail is the glanceable part, and the prefix is the most
        // redundant piece of it, so it goes before anything else does.
        let context_cell = || -> Option<String> {
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
                Some(format!("{}{}", muted("ctx "), share(&format!("{pct}%"))))
            } else {
                // E4: no known window is an honest unknown, never `0%`.
                Some(muted("ctx ?"))
            }
        };
        let tail_for = |model: &str, with_context: bool| -> Vec<String> {
            let mut tail = Vec::new();
            if !model.is_empty() {
                tail.push(muted(model));
            }
            if let Some(thinking) = &self.thinking {
                // pi colors the thinking level with its own role (the seven
                // `thinking*` tokens) - a level you can see at a glance.
                tail.push(Self::thinking_style(theme, thinking)(thinking));
            }
            if with_context && let Some(cell) = context_cell() {
                tail.push(cell);
            }
            tail
        };
        // Gh #389 gap 5: the ladder, in pi's order - the least informative
        // cell goes first, and the row is never allowed to exceed `width` with
        // a blind truncation that would cut a cell in half.
        //
        // `[full model, context]` → `[bare model, context]` (drop the
        // provider prefix, which `git`/`ctx` cells make redundant at a
        // glance) → `[bare model]` (drop the context cell) → truncate as a
        // last resort with an ellipsis, so a cut is visibly a cut.
        // Gh #389 gap 5: the ladder, in pi's order - the least informative
        // cell goes first, and the row is never allowed to exceed `width` with
        // a blind truncation that would cut a cell in half.
        //
        //   1. full model + context      (everything fits)
        //   2. bare model + context      (drop the provider prefix)
        //   3. bare model                (drop the context cell)
        //   4. ...and only then cut, with an ellipsis, so a cut looks like one
        let stats = stats_cells.join(&muted(" • "));
        let render_row = |tail: &[String]| -> String {
            let tail = tail.join(&muted(" • "));
            // The zero-noise rule applies to the join too: no dangling ` • `
            // when the stats half is empty (a fresh session).
            match (stats.is_empty(), tail.is_empty()) {
                (_, true) => stats.clone(),
                (true, false) => tail,
                (false, false) => format!("{stats}{}{tail}", muted(" • ")),
            }
        };
        let rungs = [
            (self.model.clone(), true),
            (bare_model(&self.model).to_string(), true),
            (bare_model(&self.model).to_string(), false),
        ];
        let mut row = String::new();
        let mut fits = false;
        for (model, with_context) in rungs {
            row = render_row(&tail_for(&model, with_context));
            if visible_width(&row) <= width {
                fits = true;
                break;
            }
        }
        // Nothing fit on even the most degraded rung: cut, dimly.
        let line = if fits {
            row
        } else {
            truncate_to_width(&row, width, "…", false)
        };
        lines.push(line);

        // Gh #388 gap 2: the status row is **reserved**, not conditional.
        //
        // It used to be pushed only when `statuses` was non-empty, so the
        // first turn added a row and every later frame kept it: the dock grew
        // by one row the moment a turn started, which moved the whole
        // transcript above it. A row that is always present - empty until
        // there is something to say - makes the dock's height constant, which
        // is the entire point of the anti-jitter status.
        let joined = self.statuses.join(" • ");
        lines.push(if joined.is_empty() {
            String::new()
        } else {
            truncate_to_width(&(theme.footer)(&joined), width, "…", false)
        });
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

    // Verifies: pi's footer degradation ladder (chrome.md footer.ts,
    // "graceful degradation") with NFR-26's 80 columns - the stats line
    // drops the provider prefix before it truncates, so the model and the
    // context share still reach the eye on a narrow pane, and a wide pane
    // keeps the full label. gh #17: the measured cost rides the protected
    // stats half, so it survives narrowing that truncates the tail.
    #[test]
    fn the_stats_line_drops_the_provider_prefix_before_truncating() {
        let footer = Footer {
            model: "openai-compatible/mimo-v2.6-flash".into(),
            thinking: Some("high".into()),
            usage: Usage {
                input: 2_600,
                output: 1_300,
                cache_read: 4_100,
                cache_write: 0,
                ..Default::default()
            },
            context_window: 200_000,
            context_used: 12_400,
            tok_s: Some(142),
            ..Default::default()
        };
        let at80 = strip(&footer.render(80, &Theme::plain()));
        let stats80 = at80[1].clone();
        assert!(stats80.contains("mimo-v2.6-flash"), "{stats80}");
        assert!(stats80.contains("142 tok/s"), "{stats80}");
        assert!(
            !stats80.contains("openai-compatible/"),
            "the prefix is what goes first: {stats80}"
        );
        assert!(
            stats80.contains("$0.0000"),
            "the measured cost is stats, not tail, so narrowing keeps it: {stats80}"
        );

        let at140 = strip(&footer.render(140, &Theme::plain()));
        assert!(
            at140[1].contains("openai-compatible/mimo-v2.6-flash"),
            "a wide pane keeps the full label: {}",
            at140[1]
        );
        assert!(at140[1].contains("ctx 6%"), "{}", at140[1]);
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

    // Verifies: gh #389 gap 3 - **zero-noise.** A fresh session shows no
    // counters at all: `↑0 ↓0 R0 W0 0.0%` is noise that trains people to
    // stop reading the footer. Each cell appears as soon as it has something
    // to say, and goes again when it does not.
    #[test]
    fn a_fresh_session_shows_no_zero_counters() {
        let fresh = strip(&Footer::default().render(120, &Theme::plain()));
        for zero in ["↑0", "↓0", "R0", "W0", "0.0%"] {
            assert!(
                !fresh[1].contains(zero),
                "`{zero}` is noise before any usage: {}",
                fresh[1]
            );
        }

        // One measured counter is enough to show that one, and no other.
        let some = Footer {
            usage: Usage {
                input: 42,
                ..Default::default()
            },
            ..Default::default()
        };
        let out = strip(&some.render(120, &Theme::plain()));
        assert!(out[1].contains("↑42"), "{}", out[1]);
        assert!(!out[1].contains("↓0"), "still no zero output: {}", out[1]);
        assert!(
            !out[1].contains("R0"),
            "still no zero cache read: {}",
            out[1]
        );
        assert!(
            !out[1].contains("0.0%"),
            "a 0% cache ratio is noise when there is no cache: {}",
            out[1]
        );
    }

    // Verifies: gh #389 gap 3 - the cache percentage is a *ratio* of the same
    // counters, so it is shown exactly when the ratio is meaningful.
    #[test]
    fn the_cache_ratio_appears_only_with_cache_traffic() {
        let with_cache = Footer {
            usage: Usage {
                input: 100,
                cache_read: 900,
                ..Default::default()
            },
            ..Default::default()
        };
        let out = strip(&with_cache.render(120, &Theme::plain()));
        assert!(out[1].contains("90.0%"), "{}", out[1]);

        // Input alone, no cache traffic: no ratio to report.
        let no_cache = Footer {
            usage: Usage {
                input: 100,
                ..Default::default()
            },
            ..Default::default()
        };
        let out = strip(&no_cache.render(120, &Theme::plain()));
        assert!(!out[1].contains('%'), "{}", out[1]);
    }

    // Verifies: gh #389 gap 4 - `~` shortening happens at a **path
    // boundary**. The naive `strip_prefix` shortened `/home/u2/x` to `~2/x`
    // under a HOME of `/home/u`, printing a stranger's home as if it were
    // yours.
    #[test]
    fn home_shortening_respects_the_path_boundary() {
        // The real prefix.
        assert_eq!(shorten_home("/home/u/proj", Some("/home/u")), "~/proj");
        assert_eq!(shorten_home("/home/u", Some("/home/u")), "~");
        // A sibling whose name merely starts with the home path.
        assert_eq!(shorten_home("/home/u2/x", Some("/home/u")), "/home/u2/x");
        assert_eq!(
            shorten_home("/home/u-old/x", Some("/home/u")),
            "/home/u-old/x"
        );
        assert_eq!(shorten_home("/home/uuu", Some("/home/u")), "/home/uuu");
        // Another user's home entirely.
        assert_eq!(
            shorten_home("/home/other/x", Some("/home/u")),
            "/home/other/x"
        );
        // Windows separators, and the whole-path case.
        assert_eq!(
            shorten_home("C:\\Users\\u\\proj", Some("C:\\Users\\u")),
            "~\\proj"
        );
        // No home known: unchanged.
        assert_eq!(shorten_home("/x/y", None), "/x/y");
        assert_eq!(shorten_home("/x/y", Some("")), "/x/y");
    }

    // Verifies: gh #389 gap 4 - `USERPROFILE` is consulted when `HOME` is
    // unset, which is the case in native Windows shells (`pwsh`, `cmd`).
    #[test]
    fn the_home_directory_falls_back_to_userprofile() {
        // The environment is process-wide, so this row asserts the *shape*:
        // `home_dir` never returns an empty string, and the lookup order puts
        // HOME first. A live check of the fallback needs a child process, so
        // the guarantee tested here is that the list contains both keys.
        let home = home_dir();
        assert!(
            home.as_deref().is_none_or(|h| !h.is_empty()),
            "an empty home is never returned: {home:?}"
        );
        let source = include_str!("mod.rs");
        let start = source.find("pub fn home_dir()").expect("home_dir exists");
        let body = &source[start..];
        let order = body.find("\"HOME\"").expect("HOME is consulted");
        let fallback = body
            .find("\"USERPROFILE\"")
            .expect("USERPROFILE is the Windows fallback");
        assert!(order < fallback, "HOME wins, USERPROFILE is the fallback");
    }

    // Verifies: gh #389 gap 5 - **the degradation ladder, in order.** A long
    // status join must lose its least informative cells first and never exceed
    // the row: on the way down the provider prefix goes, then the context
    // cell, and only a pathologically narrow width is truncated.
    #[test]
    fn the_ladder_drops_the_least_informative_cell_first() {
        // Measured from the rendered rows: the full row is 84 columns, the
        // bare-model-plus-context row is 74, and the bare model alone is 68.
        const WIDTH_MIDDLE: u16 = 80;
        const WIDTH_NARROW: u16 = 60;
        let footer = Footer {
            model: "openai-compatible/mimo-v2.6-flash".into(),
            thinking: Some("high".into()),
            usage: Usage {
                input: 2_600,
                output: 1_300,
                cache_read: 4_100,
                ..Default::default()
            },
            context_window: 200_000,
            context_used: 12_400,
            ..Default::default()
        };
        // Wide: everything.
        let wide = strip(&footer.render(200, &Theme::plain()));
        assert!(wide[1].contains("openai-compatible/"), "{}", wide[1]);
        assert!(wide[1].contains("ctx 6%"), "{}", wide[1]);

        // Middling: the provider prefix is gone, the context still says 6%.
        let middle = strip(&footer.render(WIDTH_MIDDLE, &Theme::plain()));
        assert!(!middle[1].contains("openai-compatible/"), "{}", middle[1]);
        assert!(middle[1].contains("mimo-v2.6-flash"), "{}", middle[1]);
        assert!(middle[1].contains("ctx 6%"), "{}", middle[1]);

        // Narrow: the context cell goes too, the model stays.
        let narrow = strip(&footer.render(WIDTH_NARROW, &Theme::plain()));
        assert!(!narrow[1].contains("ctx"), "{}", narrow[1]);
        assert!(narrow[1].contains("mimo-v2.6-flash"), "{}", narrow[1]);

        // Every width is bounded, never overflowing the row.
        for width in [20u16, 30, 40, 55, 70, 80, 120, 200] {
            let line = strip(&footer.render(width, &Theme::plain()))[1].clone();
            assert!(
                visible_width(&line) <= usize::from(width),
                "at {width} cols the row is {} wide: {line:?}",
                visible_width(&line)
            );
        }
    }

    // Verifies: gh #389 gap 5 - an unbounded status join cannot steal the
    // row: the reserved status line is bounded too.
    #[test]
    fn a_long_status_join_stays_within_the_row() {
        let footer = Footer {
            statuses: vec!["a".repeat(200), "b".repeat(200)],
            ..Default::default()
        };
        let lines = strip(&footer.render(60, &Theme::plain()));
        let status = lines.last().expect("the reserved status row");
        assert!(
            visible_width(status) <= 60,
            "the status row is {} wide: {status:?}",
            visible_width(status)
        );
    }

    // Verifies: FR-PERM-26 (ADR-0042) - yolo is loud without costing
    // a row: the `[YOLO]` badge rides the location line every frame
    // while the mode is on, and is absent when it is off (gh #244).
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
            lines.iter().any(|line| line.contains("[YOLO]")),
            "{lines:?}"
        );
        assert!(
            !lines
                .iter()
                .any(|line| line.contains("every permission prompt")),
            "no dedicated banner line: {lines:?}"
        );
        let styled = footer.render(80, &Theme::colored());
        let error = Theme::colored().error.clone();
        assert!(
            styled.iter().any(|line| line.contains(&error("[YOLO]"))),
            "the badge wears the error role: {styled:?}"
        );
        footer.yolo = false;
        let lines = footer.render(80, &theme);
        assert!(!lines.iter().any(|line| line.contains("YOLO")), "{lines:?}");
    }
}
