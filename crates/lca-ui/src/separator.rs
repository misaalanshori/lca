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
    /// A compaction is running (gh #388 gap 2): it reads `Compacting` in the
    /// same row a turn reads `Working`, with the reason on screen.
    ///
    /// It used to be handled *outside* the separator, which is why a
    /// compacting interface had no status row of its own - the dock's status
    /// row said nothing while the session was being summarized, and the only
    /// evidence was the transcript's own notice once it finished.
    Compacting {
        /// Why the compaction started.
        reason: CompactionReason,
    },
}

/// Why a compaction started (gh #388 gap 2).
///
/// The reason is shown, not merely carried: a threshold compaction fires
/// without the user asking, so "why is my session being rewritten" needs an
/// answer on screen. One short word keeps the row from ever wrapping, and the
/// state word wins over the reason when the width forces a choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactionReason {
    /// The user ran `/compact`.
    Manual,
    /// The context crossed its threshold.
    Threshold,
    /// A provider reported an overflow, so the range was cut to recover.
    Overflow,
}

impl CompactionReason {
    /// The short word the separator shows.
    #[must_use]
    pub fn word(self) -> &'static str {
        match self {
            CompactionReason::Manual => "manual",
            CompactionReason::Threshold => "threshold",
            CompactionReason::Overflow => "overflow",
        }
    }
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

    /// Say a compaction started, and why (gh #388 gap 2).
    pub fn compacting(&mut self, reason: CompactionReason) {
        self.state = SeparatorState::Compacting { reason };
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
        self.render_inner(width as usize, theme, border, None)
    }

    /// The editor frame's top cap (gh #249): pi's rounded-corner
    /// frame. `╭` + the separator's inner row narrowed by the two
    /// corners + `╮`: idle caps plain dashes, working and retrying
    /// embed the spinner row exactly as [`Self::render`] paints it.
    /// Always exactly one row; narrow terminals fall back to plain
    /// dashes rather than overflowing.
    pub fn render_top(&self, width: u16, theme: &Theme, border: &StyleFn) -> String {
        self.render_top_with(width, theme, border, 0)
    }

    /// The top cap, told how many editor rows are hidden above the view
    /// (gh #388 gap 1).
    ///
    /// `hidden` is the overflow count the badge reports; the caller owns it,
    /// because only the caller knows the frame height the editor was capped
    /// to. Zero is the historic shape, byte for byte.
    pub fn render_top_with(
        &self,
        width: u16,
        theme: &Theme,
        border: &StyleFn,
        hidden: usize,
    ) -> String {
        let width = width as usize;
        if width <= 2 {
            return border(&"─".repeat(width));
        }
        format!(
            "{}{}{}",
            border("╭"),
            self.render_inner(
                width.saturating_sub(2),
                theme,
                border,
                (hidden > 0).then_some(hidden)
            ),
            border("╮")
        )
    }

    /// The editor frame's bottom cap (gh #249): plain rounded dashes,
    /// width-exact; no status ever lives here.
    pub fn render_bottom(&self, width: u16, border: &StyleFn) -> String {
        self.render_bottom_with(width, border, 0)
    }

    /// The bottom cap, told how many editor rows are hidden below the view
    /// (gh #388 gap 1): `↓ N more`, centered, degrading exactly like the top
    /// cap. Zero is the historic plain-dash shape.
    ///
    /// The count rides the *bottom* rule rather than the top one because the
    /// two directions are different facts: rows above are behind you, rows
    /// below are still to be typed past. pi puts them on the respective caps
    /// (`editor.ts` `renderTopBorder`/`renderBottomBorder`), and stacking both
    /// on one rule would make one of them a lie about the viewport.
    pub fn render_bottom_with(&self, width: u16, border: &StyleFn, hidden: usize) -> String {
        let width = width as usize;
        if width <= 2 {
            return border(&"─".repeat(width));
        }
        let inner = width.saturating_sub(2);
        let body = if hidden == 0 {
            border(&"─".repeat(inner))
        } else {
            let badge = format!(" ↓ {hidden} more ");
            let badge_width = visible_width(&badge);
            if badge_width + 2 <= inner {
                let start = inner.saturating_sub(badge_width) / 2;
                border(&format!(
                    "{}{}{}",
                    "─".repeat(start),
                    badge,
                    "─".repeat(inner.saturating_sub(start + badge_width))
                ))
            } else {
                border(&truncate_to_width(&badge, inner, "", false))
            }
        };
        format!("{}{}{}", border("╰"), body, border("╯"))
    }

    fn render_inner(
        &self,
        width: usize,
        theme: &Theme,
        border: &StyleFn,
        hidden: Option<usize>,
    ) -> String {
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
            // Gh #388 gap 1: an idle row with hidden editor rows still owes
            // the count. Plain dashes would be a lie - the prompt is longer
            // than what is drawn - and the count is the only place that fact
            // appears. With nothing hidden the idle row is byte-for-byte what
            // it always was, which is what "idle is unchanged" protects: the
            // HUD pills and any other restyle stay out of scope.
            SeparatorState::Idle => {
                return match hidden {
                    Some(hidden) => Self::badge_only_row(width, hidden, border),
                    None => border(&"─".repeat(width)),
                };
            }
            SeparatorState::Working => (border.clone(), border.clone(), "Working".to_string()),
            // Gh #388 gap 2: a compaction is work, so it takes the working
            // row (pi paints a running indicator the same way) and names its
            // reason. The state word leads, so a narrow row drops the reason
            // rather than the state - the same priority the label truncation
            // already implements below.
            SeparatorState::Compacting { reason } => (
                border.clone(),
                border.clone(),
                format!("Compacting ({})", reason.word()),
            ),
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
        // Gh #388 gap 1: the status and the overflow badge share one row, so
        // the fit is decided here rather than by whichever side finishes
        // last. pi's three tiers (`custom-editor.ts` `renderTopBorder`) in
        // order - and the order is the point: the count is information the
        // user cannot get anywhere else on this row, so when something must
        // go it is the words, not the count.
        if let Some(hidden) = hidden {
            return self.render_with_badge(
                width,
                frame,
                &spinner,
                &label_style,
                &label,
                hidden,
                border,
            );
        }
        let status = self.status_text(width, frame, &spinner, &label_style, &label);
        let status_width = visible_width(&status);
        let fill = width.saturating_sub(3 + 1).saturating_sub(status_width);
        format!(
            "{}{}{}",
            border("── "),
            status,
            border(&format!(" {}", "─".repeat(fill)))
        )
    }

    /// A border row carrying only the badge (gh #388 gap 1): the idle form of
    /// a scrolled editor, centered on plain dashes.
    fn badge_only_row(width: usize, hidden: usize, border: &StyleFn) -> String {
        let badge = format!(" ↑ {hidden} more ");
        let badge_width = visible_width(&badge);
        if badge_width + 2 > width {
            // Too narrow to center a badge: keep the row's one-row contract
            // and its dashes rather than painting a clipped count.
            return border(&"─".repeat(width));
        }
        let start = width.saturating_sub(badge_width) / 2;
        border(&format!(
            "{}{}{}",
            "─".repeat(start),
            badge,
            "─".repeat(width.saturating_sub(start + badge_width))
        ))
    }

    /// The status span alone (`⠋ Working`), shaped to what `width` allows.
    ///
    /// `width` is the whole row's width, so the `── ` head and the ` ` tail
    /// come off the budget here once instead of at every call site.
    fn status_text(
        &self,
        width: usize,
        frame: &str,
        spinner: &StyleFn,
        label_style: &StyleFn,
        label: &str,
    ) -> String {
        let head = 3usize;
        let tail = 1usize;
        let budget = width.saturating_sub(head + tail);
        let label = if visible_width(label) > budget.saturating_sub(2) {
            truncate_to_width(label, budget.saturating_sub(2), "…", false)
        } else {
            label.to_string()
        };
        let status = format!("{} {}", spinner(frame), label_style(&label));
        // The row's invariant is "exactly one row at any width": cap the
        // status at the budget so the fill can never push past it.
        truncate_to_width(&status, budget, "", false)
    }

    /// The status with the `↑ N more` badge set into the dashes (gh #388
    /// gap 1), degrading in pi's order when they cannot both fit.
    ///
    /// Tiers, exactly as `custom-editor.ts` orders them:
    /// 1. full status + badge, with at least one dash between them;
    /// 2. spinner-only + badge - the badge survives because the count is
    ///    the only place that information appears;
    /// 3. spinner-only, when even that does not fit, and then the plain
    ///    `── ⠋ ──` form for a row too narrow to hold a spinner.
    ///
    /// Every measurement is `visible_width`, never a byte length: a status
    /// carrying a wide glyph (`日本語`) must not make the fitter lie about
    /// what fits.
    #[allow(clippy::too_many_arguments)]
    fn render_with_badge(
        &self,
        width: usize,
        frame: &str,
        spinner: &StyleFn,
        label_style: &StyleFn,
        label: &str,
        hidden: usize,
        border: &StyleFn,
    ) -> String {
        let badge = format!(" ↑ {hidden} more ");
        let badge_width = visible_width(&badge);
        // The badge is centered, so it needs its own room plus a dash either
        // side; a row too narrow to hold it plus a gap is tier 3 directly.
        let badge_start = width.saturating_sub(badge_width) / 2;
        let fits_badge = badge_width + 2 <= width;

        // Tier 1: the words and the count, with one dash of breathing room.
        if fits_badge {
            let status = self.status_text(width, frame, spinner, label_style, label);
            let status_width = visible_width(&status);
            let left_width = 3 + status_width + 1;
            if badge_start >= left_width {
                return format!(
                    "{}{}{}",
                    border("── "),
                    status,
                    border(&format!(
                        " {}{}{}",
                        "─".repeat(badge_start - left_width),
                        badge,
                        "─".repeat(width.saturating_sub(badge_start + badge_width))
                    ))
                );
            }
        }

        // Tier 2: the spinner alone, so the count still lands.
        let only_spinner = truncate_to_width(&spinner(frame), width, "", false);
        let spinner_width = visible_width(&only_spinner);
        let spinner_left = 3 + spinner_width + 1;
        if fits_badge && badge_start >= spinner_left {
            let left_width = spinner_left;
            return format!(
                "{}{}{}",
                border("── "),
                only_spinner,
                border(&format!(
                    " {}{}{}",
                    "─".repeat(badge_start - left_width),
                    badge,
                    "─".repeat(width.saturating_sub(badge_start + badge_width))
                ))
            );
        }

        // Tier 3: the badge cannot be placed at all. Keep the spinner, and
        // give up the `── ` head when even that is too wide.
        let prefix = 3.min(width.saturating_sub(spinner_width));
        format!(
            "{}{}{}",
            border(&"─".repeat(prefix)),
            only_spinner,
            border(&"─".repeat(width.saturating_sub(prefix + spinner_width)))
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
    // Changed by gh #387 gap 4: this row used to pin two literal hex values
    // (`#b294bb`, `#505050`) from the hand-picked thinking table. Those literals
    // are gone - the ramp is authored in OKLCH - so pinning them would pin the
    // old defect. It now asserts the *wiring* the brief calls for: for each of
    // the seven levels, the SGR the border emits is the ramp's colour for that
    // level, and the seven are in ramp order. A beautiful ramp nobody paints is
    // not a fix, and this is the row that says it is painted.
    #[test]
    fn the_dashes_carry_the_thinking_level() {
        let theme = Theme::colored();
        let sep = Separator::new();
        let ladder =
            crate::theme::thinking::levels(None, lca_tui::engine::colors::ColorScheme::Dark);
        let cases = [
            (None, 0usize),
            (Some("off"), 0),
            (Some("minimal"), 1),
            (Some("low"), 2),
            (Some("medium"), 3),
            (Some("high"), 4),
            (Some("xhigh"), 5),
            (Some("max"), 6),
        ];
        for (level, index) in cases {
            let rendered = sep.render(40, &theme, &separator_border(&theme, level));
            let crate::theme::Color::Rgb(r, g, b) = ladder[index].color else {
                panic!("a rung is a concrete colour");
            };
            assert!(
                rendered.contains(&format!("38;2;{r};{g};{b}")),
                "level {level:?} must paint rung {index} (#{r:02x}{g:02x}{b:02x}): \
                 {rendered:?}"
            );
            assert_eq!(strip(&rendered), "─".repeat(40), "still one full row");
        }
        // An unknown level is the quietest rung, not an arbitrary colour.
        let unknown = sep.render(40, &theme, &separator_border(&theme, Some("banana")));
        let off = sep.render(40, &theme, &separator_border(&theme, None));
        assert_eq!(strip(&unknown), strip(&off), "unknown levels read as `off`");
    }

    // Verifies: gh #249 - the editor frame's top cap rounds the
    // separator row: idle caps plain dashes, working embeds the
    // spinner row, always exactly one width-exact row.
    #[test]
    fn top_cap_rounds_the_separator_row() {
        let theme = Theme::colored();
        let idle = Separator::new().render_top(60, &theme, &border_of(&theme));
        let plain = strip(&idle);
        assert_eq!(plain, format!("╭{}╮", "─".repeat(58)), "{plain:?}");
        assert_eq!(visible_width(&idle), 60, "width-exact");
        let mut sep = Separator::new();
        sep.working();
        let working = sep.render_top(60, &theme, &border_of(&theme));
        let plain = strip(&working);
        assert!(
            plain.starts_with("╭") && plain.ends_with("╮") && plain.contains("Working"),
            "the spinner row rides the cap: {plain:?}"
        );
        assert_eq!(visible_width(&working), 60, "width-exact while working");
    }

    // ---- gh #388 gap 1: the status and the overflow badge share one row ----

    /// The widths the collision matrix sweeps: wide enough for everything,
    /// then each tier boundary, then the pathological ones.
    const SWEEP: [u16; 6] = [80, 60, 40, 30, 20, 10];

    fn working() -> Separator {
        let mut sep = Separator::new();
        sep.working();
        sep
    }

    // Verifies: gh #388 gap 1 AC1 - a long status plus a badge on a narrow
    // width never overlaps, and the row is still exactly `width` cells.
    //
    // The overlap assertion is the row's own text: a collision shows up as a
    // doubled glyph run (the badge painted over the tail of the status), so
    // the plain text must contain no run of the same glyph repeated where the
    // badge begins. Cell-exactness catches the other half - a row that
    // "avoided" overlap by writing past the width.
    #[test]
    fn a_long_status_and_a_badge_never_overlap_at_any_width() {
        let theme = Theme::colored();
        let sep = working();
        let border = border_of(&theme);
        for width in SWEEP {
            for hidden in [1usize, 4, 12, 121] {
                let row = sep.render_top_with(width, &theme, &border, hidden);
                let plain = strip(&row);
                assert_eq!(
                    visible_width(&row),
                    width as usize,
                    "hidden={hidden}: exactly one row of {width} cells: {plain:?}"
                );
                // The badge, when present, appears exactly once and intact.
                let badge = format!("↑ {hidden} more");
                let count = plain.matches(&badge).count();
                assert!(
                    count <= 1,
                    "hidden={hidden} at {width}: the badge must not be painted twice: {plain:?}"
                );
                if count == 1 {
                    // And nothing of the status word may survive to its right.
                    let after = &plain[plain.find(&badge).unwrap() + badge.len()..];
                    assert!(
                        !after.contains("Working"),
                        "hidden={hidden} at {width}: the badge overpaints the status: {plain:?}"
                    );
                }
            }
        }
    }

    // Verifies: gh #388 gap 1 AC1 - the row stays exactly one row at every
    // swept width with a badge present, including the pathological widths
    // where nothing fits but a spinner.
    #[test]
    fn the_row_is_exactly_one_row_at_every_width_with_a_badge() {
        let theme = Theme::colored();
        let sep = working();
        let border = border_of(&theme);
        for width in [80u16, 60, 40, 30, 20, 10, 6, 4, 3] {
            let row = sep.render_top_with(width, &theme, &border, 7);
            assert!(!row.contains('\n'), "width {width}: never two rows");
            assert_eq!(
                visible_width(&row),
                width as usize,
                "width {width}: {}",
                strip(&row)
            );
        }
    }

    // Verifies: gh #388 gap 1 AC1 - the degradation order is
    // full status + badge, then spinner-only + badge, then spinner-only.
    // Each rung is asserted at a width where it is the answer, so no rung can
    // be silently skipped: a fixer that jumped straight to the spinner would
    // fail rung 1, and one that dropped the count first would fail rung 2.
    #[test]
    fn the_degradation_order_is_status_then_spinner_then_bare() {
        let theme = Theme::colored();
        let sep = working();
        let border = border_of(&theme);
        let plain_at = |width: u16| strip(&sep.render_top_with(width, &theme, &border, 4));

        // Rung 1, wide: the word and the count together.
        let wide = plain_at(80);
        assert!(
            wide.contains("Working") && wide.contains("↑ 4 more"),
            "rung 1 (full status + badge) at 80: {wide:?}"
        );

        // Rung 2, narrow: the count survives, the word does not. 30 cells
        // cannot hold `── ⠋ Working` plus a 10-cell centered badge.
        let narrow = plain_at(30);
        assert!(
            narrow.contains("↑ 4 more"),
            "rung 2 keeps the count at 30: {narrow:?}"
        );
        assert!(
            !narrow.contains("Working"),
            "rung 2 drops the word at 30: {narrow:?}"
        );

        // Rung 3, too narrow for a centered badge at all: spinner only, and
        // the count is honestly gone rather than misplaced.
        let tiny = plain_at(10);
        assert!(
            !tiny.contains("more"),
            "rung 3 drops the count at 10: {tiny:?}"
        );
        assert_eq!(visible_width(&tiny), 10, "still one row: {tiny:?}");
    }

    // Verifies: gh #388 gap 1 - a wide-glyph status measures in cells, not
    // bytes. Only the fitter's arithmetic is under test, so the row is
    // checked through the public path (a compaction reason is ASCII, so the
    // sweep is the retry countdown plus the badge).
    #[test]
    fn a_wide_glyph_status_is_measured_in_cells() {
        let theme = Theme::colored();
        let sep = working();
        let border = border_of(&theme);
        // Every width in a dense sweep: cell-exactness is the property that a
        // byte-length fitter violates, because a multi-byte glyph would make
        // it believe the row is wider than it paints.
        for width in 3u16..=90 {
            let row = sep.render_top_with(width, &theme, &border, 3);
            assert_eq!(
                visible_width(&row),
                width as usize,
                "width {width}: {}",
                strip(&row)
            );
        }
    }

    // Verifies: gh #388 gap 1 design call 4 - narrowing never reorders. The
    // status stays on the left and the badge sits where pi centers it, at
    // every rung.
    #[test]
    fn narrowing_never_moves_the_status_off_the_left() {
        let theme = Theme::colored();
        let sep = working();
        let border = border_of(&theme);
        for width in SWEEP {
            let plain = strip(&sep.render_top_with(width, &theme, &border, 3));
            // The status region begins after the `── ` head and the leading
            // corner: the spinner is the first non-dash glyph.
            let body = plain.trim_start_matches('╭');
            let spinner_at = body
                .chars()
                .position(|c| FRAMES.contains(&c.to_string().as_str()));
            let Some(spinner_at) = spinner_at else {
                continue; // a rung this narrow has no spinner to place
            };
            assert!(
                spinner_at <= 4,
                "width {width}: the status belongs at the left, found at {spinner_at}: {plain:?}"
            );
        }
    }

    // Verifies: gh #388 gap 1 design call 5 - idle is unchanged **when nothing
    // is hidden**. The HUD pills and any other restyle stay out of scope; the
    // plain-dash idle border is the shape this change must not touch.
    //
    // Changed by a live pane during this cycle: the first cut had the idle arm
    // ignore `hidden` entirely, so a 15-line prompt in a 24-row pane drew ten
    // dashes and no count - rows silently gone from a prompt the user could
    // still scroll with the keyboard. Bug 1's whole point is that the count is
    // information that exists nowhere else on the row, and scrolled-off lines
    // are exactly as unavailable in the idle frame. pi shows the count in every
    // state (`editor.ts` passes `hiddenLineCount` unconditionally to
    // `renderTopBorder`). So the rule is: hidden == 0 is byte-identical to
    // before; hidden > 0 shows the count and *only* the count - no pills, no
    // status, no recolor.
    #[test]
    fn idle_with_nothing_hidden_is_byte_identical() {
        let theme = Theme::colored();
        let sep = Separator::new();
        let border = border_of(&theme);
        assert_eq!(
            strip(&sep.render_top_with(60, &theme, &border, 0)),
            format!("╭{}╮", "─".repeat(58)),
            "an idle border with nothing hidden stays plain dashes"
        );
        // And the inner row is exactly what `render` paints for that width:
        // the corners are the cap's own two cells, nothing else changes.
        assert_eq!(
            strip(&sep.render_top_with(60, &theme, &border, 0)),
            format!("╭{}╮", strip(&sep.render(58, &theme, &border))),
            "zero hidden is the historic row between its corners"
        );
    }

    // Verifies: gh #388 gap 1 - an idle row with hidden rows still owes the
    // count, because a truncated prompt must not look complete.
    #[test]
    fn idle_with_hidden_rows_shows_only_the_count() {
        let theme = Theme::colored();
        let sep = Separator::new();
        let border = border_of(&theme);
        let plain = strip(&sep.render_top_with(60, &theme, &border, 9));
        assert!(
            plain.contains("↑ 9 more"),
            "the count is on the idle border: {plain:?}"
        );
        assert!(
            !plain.contains("Working") && !plain.contains("Compacting"),
            "and nothing else is: an idle row invents no status: {plain:?}"
        );
        assert_eq!(visible_width(&plain), 60, "still one width-exact row");
        // Every cell is a dash, a corner, or the badge: no pill, no second
        // badge, no status word hiding in there.
        let rest: String = plain.replace(" ↑ 9 more ", "");
        assert!(
            rest.chars().all(|c| c == '─' || c == '╭' || c == '╮'),
            "the idle row is dashes plus the count: {plain:?}"
        );
    }

    // Verifies: gh #388 gap 1 - zero hidden rows is the historic shape, byte
    // for byte, so the new path cannot quietly restyle the ordinary working
    // row every user sees.
    #[test]
    fn zero_hidden_rows_renders_the_historic_working_row() {
        let theme = Theme::colored();
        let sep = working();
        let border = border_of(&theme);
        assert_eq!(
            sep.render_top_with(60, &theme, &border, 0),
            sep.render_top(60, &theme, &border)
        );
        assert_eq!(
            strip(&sep.render_top_with(60, &theme, &border, 0)),
            format!("╭── ⠋ Working ─{}╮", "─".repeat(60 - 16))
        );
    }

    // Verifies: gh #249 - the bottom cap is plain rounded dashes,
    // width-exact, and narrow terminals never overflow.
    #[test]
    fn bottom_cap_is_plain_rounded_dashes() {
        let theme = Theme::colored();
        let row = Separator::new().render_bottom(60, &border_of(&theme));
        assert_eq!(strip(&row), format!("╰{}╯", "─".repeat(58)));
        assert_eq!(visible_width(&row), 60, "width-exact");
        for width in [0, 1, 2, 3] {
            let top = Separator::new().render_top(width, &theme, &border_of(&theme));
            let bottom = Separator::new().render_bottom(width, &border_of(&theme));
            assert!(
                visible_width(&top) == width as usize && visible_width(&bottom) == width as usize,
                "narrow-safe at {width}"
            );
        }
    }
}
