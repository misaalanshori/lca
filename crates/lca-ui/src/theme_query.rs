//! The terminal colour query's lifecycle (gh #379 gap 1).
//!
//! `run` writes `colors::TERMINAL_COLOR_QUERY` at startup and the loop applies
//! whatever replies come back. A reply must be applied while the query is
//! expected - but **only** then, because the same byte sequences can be printed
//! by any program the user runs. Without a lifecycle, an `OSC 11` from `ls` with
//! colour or from `git` repaints the whole palette mid-session.
//!
//! The window is closed by the `DA1` sentinel the query ends with. Terminals
//! answer `DA1` whether or not they understand `OSC 11`, and they answer **in
//! order**, so its arrival proves the colour replies are answered or absent -
//! which is what a timeout could only guess at.
//!
//! # Why the grace is time, not a count
//!
//! Over a slow SSH link the answer can still be in flight when `DA1` overtakes
//! it, and it must land. A count of "one more reply" cannot tell that apart from a
//! stray printed seconds later, so the test is elapsed time: a real late reply
//! arrives within milliseconds of the sentinel, a stray from another program
//! arrives whenever it arrives. Both directions are pinned by rows (the mutation
//! check that made this concrete: "accept every reply" fails the stray row,
//! "accept none after the sentinel" fails the late row).

/// How long after the `DA1` sentinel a colour reply is still accepted.
///
/// A real late answer - the SSH case - is already in flight when `DA1` overtakes
/// it, so it lands well inside this. A stray `OSC 11` printed by a child process
/// seconds later is outside it and cannot repaint the palette.
pub const LATE_REPLY_GRACE: std::time::Duration = std::time::Duration::from_millis(250);

/// Whether the startup colour query still accepts replies, and for how long after
/// the sentinel.
///
/// Constructed open: `run` writes the query before the loop reads a byte, so the
/// very first (correct) reply must not be treated as stray.
#[derive(Debug, Clone)]
pub struct ThemeQuery {
    open: bool,
    closed_at: Option<std::time::Instant>,
}

impl Default for ThemeQuery {
    fn default() -> Self {
        Self::new()
    }
}

impl ThemeQuery {
    /// A query that is open and accepting replies.
    pub fn new() -> Self {
        Self {
            open: true,
            closed_at: None,
        }
    }

    /// Whether the window is still open.
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Close the window on the `DA1` sentinel.
    ///
    /// Idempotent in effect: a second sentinel leaves the first close time alone,
    /// so a terminal that repeats its answer cannot extend the grace.
    pub fn close(&mut self) {
        self.open = false;
        if self.closed_at.is_none() {
            self.closed_at = Some(std::time::Instant::now());
        }
    }

    /// Whether a reply arriving now should be applied.
    pub fn accepts_reply(&self) -> bool {
        self.open
            || self
                .closed_at
                .is_some_and(|at| at.elapsed() <= LATE_REPLY_GRACE)
    }
}

/// Handle one inbound byte sequence that may be a reply to the startup query.
///
/// Returns `true` when it was a reply and must **not** reach the composer. This is
/// the single place the three reply kinds are dispatched, so the apply path and
/// the guard cannot disagree about what counts as one.
///
/// The lifecycle: `DA1` closes the window and applies nothing (a sentinel carries
/// no colour); everything else is applied only while [`ThemeQuery::accepts_reply`]
/// says so, which is what keeps a stray `OSC 11` from a child process out of the
/// palette while still letting a slow link's late answer in.
pub fn handle_reply(chat: &mut crate::chat::Chat, data: &str) -> bool {
    use lca_tui::engine::colors::{
        is_device_attributes_report, is_startup_query_reply, parse_osc11_background_color,
        parse_terminal_color_scheme_report,
    };
    if is_device_attributes_report(data) {
        chat.close_theme_query();
        return true;
    }
    if !is_startup_query_reply(data) {
        return false;
    }
    if let Some(rgb) = parse_osc11_background_color(data) {
        // gh #10: the reply carries the background itself, not just its
        // scheme - a `system` theme rebuilds from it.
        chat.apply_terminal_background(rgb);
    } else if let Some(scheme) = parse_terminal_color_scheme_report(data) {
        chat.apply_detected_scheme(scheme);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    // Verifies: gh #379 gap 1 - the query starts open, so the first (correct)
    // reply on a normal terminal is applied rather than mistaken for a stray.
    #[test]
    fn a_new_query_accepts_replies() {
        let query = ThemeQuery::new();
        assert!(query.is_open());
        assert!(query.accepts_reply());
    }

    // Verifies: gh #379 gap 1 - the sentinel closes the window, and the close time
    // is recorded so the grace has something to measure from.
    #[test]
    fn the_sentinel_closes_the_window_and_starts_the_grace() {
        let mut query = ThemeQuery::new();
        query.close();
        assert!(!query.is_open(), "the window is closed");
        assert!(
            query.accepts_reply(),
            "but a reply already in flight still lands"
        );
    }

    // Verifies: gh #379 gap 1 - a stray reply outside the grace is refused. This
    // is the defect: a child process printing OSC 11 must not repaint the theme.
    #[test]
    fn a_reply_outside_the_grace_is_refused() {
        let mut query = ThemeQuery::new();
        query.close();
        // Pretend the sentinel was long ago, which is what a later stray sees.
        query.closed_at = Some(
            std::time::Instant::now()
                .checked_sub(LATE_REPLY_GRACE + std::time::Duration::from_millis(50))
                .expect("the grace fits in an Instant"),
        );
        assert!(!query.is_open());
        assert!(
            !query.accepts_reply(),
            "a reply printed after the grace is a stray, not an answer"
        );
    }

    // Verifies: gh #379 gap 1 - a repeated sentinel does not extend the grace, so
    // a chatty terminal cannot keep the window ajar for a later stray.
    #[test]
    fn a_second_sentinel_does_not_extend_the_grace() {
        let mut query = ThemeQuery::new();
        query.close();
        let first = query.closed_at;
        std::thread::sleep(std::time::Duration::from_millis(5));
        query.close();
        assert_eq!(
            query.closed_at, first,
            "the close time is the first close, not the latest"
        );
    }
}
