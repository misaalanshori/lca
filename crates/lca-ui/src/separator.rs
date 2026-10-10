//! The separator row: pi's spinner-in-the-border
//! (`pi-tui-re/src_re/agent-components/chrome.md` §`status-indicator`,
//! `components/status-indicator.ts` + `components/custom-editor.ts`).
//!
//! One terminal row divides the transcript from the composer. Idle it is
//! plain border dashes; while work runs it carries the status inline -
//! `── ⠴ Working ─────…` - with the spinner frame cycling on the
//! interface's tick. Every state is exactly one row and fills the width,
//! and every state says its condition in words, so color is never the
//! only signal (NFR-28).

use std::time::{Duration, Instant};

use lca_tui::engine::text::{truncate_to_width, visible_width};

use crate::theme::{Role, StyleFn, Theme};

/// pi's braille spinner frames and cadence (`pi-tui` `loader.ts`).
/// Shared with the model picker's loading line (gh #232).
pub(crate) const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
pub(crate) const FRAME_MS: Duration = Duration::from_millis(80);

/// What the separator is saying right now (pi's `StatusIndicatorKind` plus
/// `IdleStatus`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SeparatorState {
    /// The rest state: plain border dashes.
    Idle,
    /// A turn is running: the whole row in `separator_border` - spinner,
    /// label and dashes in one color, pi's *rendered* working row.
    Working,
    /// A provider call is waiting out a backoff: warning spinner +
    /// `Retrying (n/m) in Ns...`, counting down live (pi's
    /// `RetryStatusIndicator` + `CountdownTimer`).
    Retrying {
        /// The attempt about to run (1-based).
        attempt: u32,
        /// The configured limit.
        max: u32,
        /// When the backoff ends.
        deadline: Instant,
    },
}

/// The separator row and its animation clock.
#[derive(Debug)]
pub struct Separator {
    /// The current state.
    state: SeparatorState,
    /// The spinner frame index.
    frame: usize,
    /// When the frame last advanced.
    advanced: Instant,
}

impl Default for Separator {
    fn default() -> Self {
        Separator {
            state: SeparatorState::Idle,
            frame: 0,
            advanced: Instant::now(),
        }
    }
}

impl Separator {
    /// A separator at rest.
    pub fn new() -> Self {
        Self::default()
    }

    /// The state on display.
    pub fn state(&self) -> &SeparatorState {
        &self.state
    }

    /// Say the turn started.
    pub fn working(&mut self) {
        self.state = SeparatorState::Working;
    }

    /// Say a retry was scheduled for `delay_ms` from now.
    pub fn retrying(&mut self, attempt: u32, max: u32, delay_ms: u64) {
        self.state = SeparatorState::Retrying {
            attempt,
            max,
            deadline: Instant::now() + Duration::from_millis(delay_ms),
        };
    }

    /// Say the turn ended (or nothing is running): the rest state. pi
    /// clears its indicator the same way - the transcript's own error line
    /// and the footer's status carry a stop that was not clean.
    pub fn idle(&mut self) {
        self.state = SeparatorState::Idle;
        self.frame = 0;
    }

    /// Advance the animation. Returns `true` when the frame moved, so the
    /// loop knows to repaint (one frame per `FRAME_MS`, only while an
    /// indicator is up - idle never wakes the renderer).
    pub fn tick(&mut self) -> bool {
        if self.state == SeparatorState::Idle {
            return false;
        }
        if self.advanced.elapsed() < FRAME_MS {
            return false;
        }
        self.frame = (self.frame + 1) % FRAMES.len();
        self.advanced = Instant::now();
        true
    }

    /// Render the row: border dashes across the full width with the
    /// status set into them, left-aligned after `── ` (pi's
    /// `renderInBorder` + `CustomEditor::renderTopBorder`).
    ///
    /// `border` is the dash style: pi colors its editor border with the
    /// thinking level (`interactive-mode.ts` `editor.borderColor =
    /// getThinkingBorderColor(level)`), falling back to `thinkingOff` -
    /// which is `borderMuted`'s own darkGray - when no level is set.
    /// [`Chat`](crate::Chat) resolves it and passes it in.
    pub fn render(&self, width: u16, theme: &Theme, border: &StyleFn) -> String {
        let width = width as usize;
        if width == 0 {
            return String::new();
        }
        // The working row is one color end to end - spinner, label and
        // dashes alike - which is how pi actually paints it when the
        // indicator is embedded (`interactive-mode.ts:2251` passes the
        // editor's border color as the indicator's own color function, so
        // a live pi row measures `38;5;109` from the first dash to the
        // last). The register's "accent spinner + muted text" described
        // `WorkingStatusIndicator`'s *unembedded* defaults; the owner has
        // ruled for pi's rendered look. Retry keeps its two roles: pi
        // gives that state a warning spinner and a muted countdown.
        let (spinner, label_style, label) = match &self.state {
            SeparatorState::Idle => return border(&"─".repeat(width)),
            SeparatorState::Working => (border.clone(), border.clone(), "Working".to_string()),
            SeparatorState::Retrying {
                attempt,
                max,
                deadline,
            } => {
                let left = deadline.saturating_duration_since(Instant::now());
                // pi's countdown renders `Math.ceil(delayMs / 1000)` and
                // ticks down once a second.
                let secs = left.as_millis().div_ceil(1000);
                (
                    theme.role(Role::Warning),
                    theme.role(Role::Muted),
                    format!("Retrying ({attempt}/{max}) in {secs}s..."),
                )
            }
        };
        let frame = FRAMES[self.frame % FRAMES.len()];
        // `── ` + `⠴ Working` + ` ` + dashes = width, exactly one row.
        let head = 3usize;
        let tail = 1usize;
        let budget = width.saturating_sub(head + tail);
        let label = if visible_width(&label) > budget.saturating_sub(2) {
            truncate_to_width(&label, budget.saturating_sub(2), "…", false)
        } else {
            label
        };
        let status = format!("{} {}", spinner(frame), label_style(&label));
        // The row's invariant is "exactly one row at any width": cap the
        // status at the budget so the fill can never push past it.
        let status = truncate_to_width(&status, budget, "", false);
        let status_width = visible_width(&status);
        let fill = budget.saturating_sub(status_width);
        format!(
            "{}{}{}",
            border("── "),
            status,
            border(&format!(" {}", "─".repeat(fill)))
        )
    }
}

/// The separator's dash style: the thinking level's own role, which is
/// `thinkingOff` (the same darkGray as `borderMuted`) when no level is
/// set - pi's `editor.borderColor` rule.
pub fn separator_border(theme: &Theme, level: Option<&str>) -> StyleFn {
    let role = match level {
        None | Some("") | Some("off") => Role::ThinkingOff,
        Some("minimal") => Role::ThinkingMinimal,
        Some("low") => Role::ThinkingLow,
        Some("medium") => Role::ThinkingMedium,
        Some("high") => Role::ThinkingHigh,
        Some("xhigh") => Role::ThinkingXhigh,
        Some("max") => Role::ThinkingMax,
        Some(_) => Role::ThinkingOff,
    };
    theme.role(role)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The dash style a test renders with: no configured thinking level.
    fn border_of(theme: &Theme) -> StyleFn {
        separator_border(theme, None)
    }

    fn strip(line: &str) -> String {
        lca_tui::engine::text::strip_terminal_sequences(line)
    }

    #[test]
    fn idle_is_plain_border_dashes_across_the_width() {
        let theme = Theme::colored();
        let row = Separator::new().render(60, &theme, &border_of(&theme));
        assert_eq!(strip(&row), "─".repeat(60));
        assert_eq!(visible_width(&row), 60, "exactly one full row");
    }

    // Verifies: R2 (owner's ruling, cycle 9-fix F3) - the working row is
    // pi's *rendered* one: spinner, label and dashes all in
    // `separator_border`, one color end to end, one row, full width.
    #[test]
    fn working_row_paints_one_color_end_to_end() {
        let theme = Theme::colored();
        let mut sep = Separator::new();
        sep.working();
        let row = sep.render(60, &theme, &border_of(&theme));
        let plain = strip(&row);
        assert_eq!(plain, format!("── ⠋ Working ─{}", "─".repeat(60 - 14)));
        assert_eq!(visible_width(&row), 60, "one row, no overflow: {plain:?}");
        let border = border_of(&theme);
        assert!(
            row.contains(&border("⠋")),
            "spinner in the border color: {row:?}"
        );
        assert!(
            row.contains(&border("Working")),
            "label in the border color too: {row:?}"
        );
        assert!(row.contains(&border("── ")), "and the dashes: {row:?}");
        // Exactly one foreground color on the row: nothing accent, muted,
        // or warning left over from the register's earlier wording.
        let colors: Vec<&str> = row
            .split('\x1b')
            .filter(|s| s.starts_with("[38;2;"))
            .collect();
        assert_eq!(
            colors.len(),
            4,
            "three styled spans plus the closing prefix, all one color: {row:?}"
        );
        assert!(
            !row.contains("38;2;138;190;183") && !row.contains("38;2;128;128;128"),
            "no accent or muted on the working row: {row:?}"
        );
        assert!(!row.contains("\x1b[0m"), "channel resets only: {row:?}");
    }

    // Verifies: R2 - a retry shows pi's warning spinner and countdown text.
    #[test]
    fn retrying_counts_down_with_a_warning_spinner() {
        let theme = Theme::colored();
        let mut sep = Separator::new();
        sep.retrying(2, 5, 3_000);
        let plain = strip(&sep.render(70, &theme, &border_of(&theme)));
        assert!(plain.starts_with("── "), "{plain:?}");
        assert!(
            plain.contains("Retrying (2/5) in 3s..."),
            "the countdown names attempt, limit, and seconds: {plain:?}"
        );
        assert!(
            sep.render(70, &theme, &border_of(&theme))
                .contains(&theme.role(Role::Warning)("⠋")),
            "warning spinner while retrying"
        );
        assert_eq!(
            visible_width(&sep.render(70, &theme, &border_of(&theme))),
            70
        );
    }

    // Verifies: R2 - the row still fills the width when the label barely
    // fits (narrow terminal): no overflow, no half row.
    #[test]
    fn a_narrow_row_still_fills_exactly_one_row() {
        let theme = Theme::colored();
        let mut sep = Separator::new();
        sep.retrying(10, 10, 30_000);
        for width in [6, 10, 20, 33, 40] {
            let row = sep.render(width, &theme, &border_of(&theme));
            assert_eq!(
                visible_width(&row),
                width as usize,
                "width {width}: {}",
                strip(&row)
            );
        }
    }

    // Verifies: R2 - the animation advances only while an indicator is up,
    // so an idle interface never repaints.
    #[test]
    fn the_spinner_advances_only_while_work_runs() {
        let mut sep = Separator::new();
        assert!(!sep.tick(), "idle never ticks");
        sep.working();
        // Force the cadence: backdate the last advance.
        sep.advanced = Instant::now() - FRAME_MS;
        assert!(sep.tick(), "a frame advanced");
        assert!(!sep.tick(), "not faster than the cadence");
        sep.idle();
        sep.advanced = Instant::now() - FRAME_MS;
        assert!(!sep.tick(), "back to rest");
    }

    // Verifies: R2 - the dashes carry the thinking level, the way pi
    // colors its editor border (`interactive-mode.ts`:
    // `editor.borderColor = getThinkingBorderColor(level)`), and fall back
    // to `thinkingOff` - the same darkGray `borderMuted` uses - when no
    // level is configured.
    #[test]
    fn the_dashes_carry_the_thinking_level() {
        let theme = Theme::colored();
        let sep = Separator::new();
        let high = sep.render(40, &theme, &separator_border(&theme, Some("high")));
        // The style wraps the whole run, so assert on the SGR, not on a
        // one-character wrap.
        assert!(
            high.contains("38;2;178;148;187"),
            "thinkingHigh #b294bb: {high:?}"
        );
        let off = sep.render(40, &theme, &separator_border(&theme, None));
        assert!(
            off.contains("38;2;80;80;80"),
            "thinkingOff #505050: {off:?}"
        );
        assert_eq!(strip(&off), "─".repeat(40), "still one full row");
    }
}
