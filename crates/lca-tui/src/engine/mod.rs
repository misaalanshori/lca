//! The engine: terminal I/O, input reassembly and key dialects, the
//! semantic keybinding registry, text measurement, and the renderers.
//!
//! Everything here is agent-agnostic (the §1 boundary rule): no import of
//! `lca-core`, `lca-protocol`, `lca-session`, or session types. Widgets that
//! need agent data are wrapped by `lca-ui`.

pub mod alt_screen;
pub mod colors;
pub mod core;
pub mod keybindings;
pub mod keys;
pub mod layout;
pub mod main_screen;
pub mod selection;
pub mod stdin_buffer;
pub(crate) mod sys;
pub mod terminal;
pub mod text;
