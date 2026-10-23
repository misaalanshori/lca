//! The `/settings` project-override row's cycle (gh #397).
//!
//! Split from `chat_commands.rs` under the 1,200-line ceiling. Kept as its own
//! file because the tri-state is a distinct promise from the session cycle it
//! sits beside: `Enter` writes the **user** file and changes this session,
//! while `Tab` writes `<cwd>/.lca/config.toml` and states something about *this
//! project only* - a different file, different consequences, and the one place
//! where "inherit" has to mean removal rather than a value.

use super::Chat;

impl Chat {
    /// One step on a `/settings` row's **project override** (gh #397): the
    /// tri-state cycle `inherit → on → off → inherit`, written to
    /// `<cwd>/.lca/config.toml`.
    ///
    /// `inherit` removes the key; it never writes `= false`, because the
    /// absence of an opinion is what inheriting *is*. That is the whole point:
    /// a two-way toggle conflated the two and left a hand-written override
    /// unremovable from the interface.
    pub(super) fn cycle_project_override(
        &mut self,
        picker: &mut crate::chat_pickers::SettingsPicker,
        forward: bool,
    ) -> String {
        // Gh #397: resolve through the *visible* set, not `rows[selected]` -
        // the cursor indexes the filtered list, so indexing the raw rows acted
        // on a different setting entirely whenever a search was active.
        let Some(row) = picker.selected_row().cloned() else {
            return String::new();
        };
        let Some(state) = row.project else {
            return format!("{} is not a project override", row.key);
        };
        let key = row.key.clone();
        // Only a two-valued row has an on/off to write; `values` holds them in
        // the order the row cycles, so `on` is `true` and `off` is `false` for
        // the boolean rows this applies to.
        let (Some(on), Some(off)) = (
            row.values.iter().find(|value| *value == "true").cloned(),
            row.values.iter().find(|value| *value == "false").cloned(),
        ) else {
            return format!("{key} has no on/off to override");
        };
        let next = state.step(forward);
        let written = match next {
            crate::state::ProjectOverride::Inherit => None,
            crate::state::ProjectOverride::On => Some(on),
            crate::state::ProjectOverride::Off => Some(off),
        };
        let Some(persist) = self.world.options.hooks.persist_project_setting.clone() else {
            return format!("{key}: this host cannot write a project file");
        };
        if let Err(err) = persist(&key, written.clone()) {
            // Read-only honest: the row keeps showing what the file says.
            return format!("{key}: could not write the project file: {err}");
        }
        // Advance the row in place as well as re-reading: the re-read is
        // authoritative, but a host whose row source cannot see the project
        // layer (or a test fixture without one) would otherwise leave the
        // cycle stuck on the state it started in.
        if let Some(row) = picker.rows.get_mut(picker.selected)
            && row.key == key
        {
            row.project = Some(next);
            if let Some(value) = &written {
                row.value = value.clone();
            }
        }
        // The session follows only when the value is actually live (the
        // project is trusted); `apply_setting` is safe either way because the
        // merged value it is given is what the loader will read next.
        if let Some(value) = &written
            && self.project_layer_is_live()
        {
            self.apply_setting(&key, value);
        }
        self.refresh_settings_rows();
        let where_ = if self.project_layer_is_live() {
            ""
        } else {
            " (project not trusted, so this does not take effect yet)"
        };
        format!(
            "{key}: project {} (was {}){where_}",
            next.word(),
            state.word()
        )
    }

    /// Whether a project-layer write can take effect in this session: the
    /// project must be trusted, or the loader ignores `.lca/config.toml`
    /// (FR-PERM-9). Asked through the source of a key that only a project
    /// file could set, which is the same trust the picker's row asked about.
    fn project_layer_is_live(&self) -> bool {
        self.world
            .options
            .hooks
            .project_layer_trusted
            .as_ref()
            .is_none_or(|trusted| trusted())
    }
}
