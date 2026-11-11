//! Rows for the cooked-input notice (gh #592).

use lca_tui::engine::program_status::ProgramState;
use lca_tui::engine::terminal::{FakeTerminal, InputHandler, ResizeHandler, Terminal};

use super::cooked_input_notice;

/// A terminal that reports the reason a real one would (gh #323 defect 3).
struct Cooked(Option<String>);

impl Terminal for Cooked {
    fn start(&mut self, _on_input: InputHandler, _on_resize: ResizeHandler) {}
    fn stop(&mut self) {}
    fn drain_input(&mut self, _max_ms: u64, _idle_ms: u64) {}
    fn write(&mut self, _data: &str) {}
    fn columns(&self) -> u16 {
        80
    }
    fn rows(&self) -> u16 {
        24
    }
    fn kitty_protocol_active(&self) -> bool {
        false
    }
    fn raw_mode_error(&self) -> Option<String> {
        self.0.clone()
    }
    fn move_by(&mut self, _lines: i32) {}
    fn hide_cursor(&mut self) {}
    fn show_cursor(&mut self) {}
    fn clear_line(&mut self) {}
    fn clear_from_cursor(&mut self) {}
    fn clear_screen(&mut self) {}
    fn set_title(&mut self, _title: &str) {}
    fn set_progress(&mut self, _active: bool) {}
    fn set_program_status(&mut self, _state: ProgramState) {}
}

// Verifies: gh #592 - a refused raw mode is **said out loud**, naming what the
// user loses rather than what the console called it.
//
// Before this file the reason had no reader at all: the app ran on cooked input
// in silence. The row is on the notice because that is the product behaviour;
// the trait's `raw_mode_error` was already pinned by gh #323's own rows.
#[test]
fn a_refused_raw_mode_is_reported_with_what_it_costs() {
    let refused = Cooked(Some(
        "SetConsoleMode(stdin) refused raw mode (os error 5)".into(),
    ));
    let notice = cooked_input_notice(&refused).expect("a refusal is reported");
    assert!(
        notice.starts_with("warning: "),
        "every degraded-startup notice leads with warning:: {notice}"
    );
    assert!(
        notice.contains("os error 5"),
        "and it carries the reason verbatim, so a bug report has the code: {notice}"
    );
    assert!(
        notice.contains("Ctrl+C") && notice.contains("keybindings"),
        "and says what the user loses, not what the subsystem called it: {notice}"
    );

    // A terminal that entered raw mode says nothing - no notice is not a bug.
    assert!(
        cooked_input_notice(&Cooked(None)).is_none(),
        "a working terminal is not warned about"
    );
    // And the trait's default answer (a double with no opinion) is silent too.
    assert!(
        cooked_input_notice(&FakeTerminal::new(80, 24)).is_none(),
        "a terminal that cannot answer reports nothing"
    );
}
