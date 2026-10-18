//! Ctrl+C's two-stage behavior on the main screen (split from
//! `chat_tests.rs` under the 1,200-line file ceiling).

use super::chat;
use super::*;

// Verifies: the documented Ctrl+C ladder - a non-empty editor clears first,
// and only an empty one exits, so a stray tap never ends a session with text
// still typed.
#[test]
fn ctrl_c_clears_text_first_and_exits_when_empty() {
    let mut chat = chat();
    for c in "scratch-text".chars() {
        chat.handle_key(&c.to_string());
    }
    assert_eq!(chat.editor.text(), "scratch-text");
    assert_eq!(chat.handle_key("\x03"), Action::Continue);
    assert_eq!(chat.editor.text(), "", "the first tap clears");
    assert_eq!(chat.handle_key("\x03"), Action::Continue);
    assert_eq!(chat.handle_key("\x03"), Action::Exit);
}
