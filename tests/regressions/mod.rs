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
// Gh #380 gap 1: `@file` completed only at column 0.
#[path = "380-at-sign-completes-anywhere-a-word-starts.rs"]
mod at_sign_completes_anywhere_a_word_starts;
// Gh #366 gap 3: a split terminal reply is no longer typed into the prompt.
#[path = "366-split-reply-never-typed.rs"]
mod split_reply_never_typed;
// Gh #386 gap 1: the popup's ragged description column and missing provenance.
#[path = "386-autocomplete-column-and-provenance.rs"]
mod autocomplete_column_and_provenance;
