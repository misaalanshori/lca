//! The startup header's entry-level methods (gh #400 gaps 2 and 3), split from
//! `transcript/mod.rs` at the 1,200-line ceiling.
//!
//! The header is an [`Entry`] rather than pre-rendered rows so three things hold:
//! the expand key reaches it, the collision notices ride it and survive
//! `quietStartup`, and it outlives `/new` - a header that resets when a fresh
//! session opens is worse than one that never expanded. The state lives on the
//! entry, which is what makes that survivable: the session-switch path re-pushes
//! the entry rather than rebuilding it from launch values.

use super::{Entry, Transcript};

impl Transcript {
    /// Append the startup header (gh #400 gaps 2 and 3): the compact line, or the
    /// table behind `ctrl+o`.
    ///
    /// An entry rather than `Raw` lines so the expand state and the collision
    /// notices live somewhere `clear()` cannot lose - see [`Entry::StartupResources`].
    pub fn push_startup_resources(&mut self, compact: Vec<String>, expanded: Vec<String>) {
        self.entries.push(Entry::StartupResources {
            compact,
            expanded,
            expanded_state: false,
        });
        self.invalidate_cache();
    }

    /// Flip the startup header between compact and expanded. Returns `true` when a
    /// header was there to flip, so the caller knows whether the key was consumed.
    ///
    /// The state lives on the entry, which is what makes it survive `/new`: the
    /// entry is re-pushed by the session-switch path with `expanded_state`
    /// restored, rather than being rebuilt from scratch at launch values.
    pub fn toggle_startup_resources(&mut self) -> bool {
        for entry in self.entries.iter_mut() {
            if let Entry::StartupResources { expanded_state, .. } = entry {
                *expanded_state = !*expanded_state;
                self.invalidate_cache();
                return true;
            }
        }
        false
    }

    /// Whether the startup header is currently expanded (for the session-switch path
    /// to carry across `/new`).
    pub fn startup_resources_expanded(&self) -> bool {
        self.entries.iter().any(|entry| {
            matches!(
                entry,
                Entry::StartupResources {
                    expanded_state: true,
                    ..
                }
            )
        })
    }
}
