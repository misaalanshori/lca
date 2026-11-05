//! `DENY_PROGRAMS`'s uniqueness row (gh #339 finding 1).
//!
//! `"socat"` was listed twice. Harmless - a linear scan answers the same either way -
//! which is exactly why nothing caught it: the table has no reader that would notice a
//! repeat, and a refusal test cannot distinguish "denied once" from "denied twice".
//!
//! The row is on the **table** rather than on a refusal, so a second copy of any entry
//! fails here instead of waiting for someone to read 120 lines.

use super::DENY_PROGRAMS;

#[test]
fn every_denied_program_is_listed_once() {
    let repeated: Vec<&str> = DENY_PROGRAMS
        .iter()
        .copied()
        .filter(|name| DENY_PROGRAMS.iter().filter(|other| *other == name).count() > 1)
        .collect();
    assert!(repeated.is_empty(), "DENY_PROGRAMS repeats {repeated:?}");
}
