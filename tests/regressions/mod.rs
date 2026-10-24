#[path = "part0.rs"]
mod part0;

#[path = "part1.rs"]
mod part1;

#[path = "part2.rs"]
mod part2;

#[path = "part3.rs"]
mod part3;
// A shared test double, kept at the crate root so every part can `crate::mid_turn_slash_dispatches_as_command`.
#[path = "mid-turn-slash-dispatches-as-command.rs"]
mod mid_turn_slash_dispatches_as_command;
// A shared test double, kept at the crate root so every part can `crate::doubles_275`.
#[path = "doubles_275.rs"]
mod doubles_275;
