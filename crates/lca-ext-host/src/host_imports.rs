//! The host-import surface: every `impl …Host for HostState` the linker wires,
//! one file per **capability domain** (gh #552).
//!
//! ## Why it is partitioned this way
//!
//! This module is the capability linker surface - the code that decides what a
//! sandboxed component may reach, and `SECURITY.md` asks a reviewer to read it.
//! At 997 lines it was a single file, so a review either read all of it or
//! trusted a summary. The domains are the units a reviewer actually reasons
//! about, and they were already separated by the file's own `impl` blocks:
//!
//! * `bags` - `log`, `resources`, `state`: what an extension stores and reads.
//! * `sys` - `fs`, `process`, `pty`: the process/filesystem surface.
//! * `net_oauth` - `net`, `oauth`, `credentials`: the network and identity
//!   surface, including the consent rule that distinguishes a tool call from a
//!   provider probe.
//! * `wit` - the ABI crossing: WIT records <-> protocol types.
//! * `ui_tools` - `ui-dialogs` and the shared `tools` import.
//!
//! A domain file imports what it uses by name. The previous file began with
//! `use super::*;`, which is exactly what makes a security surface hard to
//! review: it hides which host items a given import can touch. There is no
//! `use super::*` anywhere below.
//!
//! Nothing about the wiring moved: the `impl` blocks and the `net_host_for!`
//! macro are the same items, now defined in sibling files. `lib.rs` declares
//! this module and reaches `without_dialogs` through it exactly as before.
//!
//! Split from `lib.rs` (cycle 7, P3). Behaviour unchanged.

mod bags;
mod net_oauth;
mod sys;
mod ui_tools;
mod wit;

pub use ui_tools::without_dialogs;
// What the rest of the crate names: the two dispatch entries ask for
// `without_dialogs`, and the provider/compaction/transform work paths cross the
// WIT boundary through these four.
pub(crate) use wit::{from_wit_event, from_wit_identity, from_wit_usage, to_wit_request};
