//! The `/settings` cycle (gh #30, gh #397), split from `chat_commands.rs` at
//! gate 11's 1,200-line ceiling. Behaviour unchanged except the value-preserving
//! cycle this batch adds.
//!
//! **Why this is its own file.** Gh #397's defect was a *one-way door on a config
//! file*: the cycle looked the current value up in the preset list and fell back to
//! index 0 on a miss, so the first keypress replaced a hand-set value with the
//! second preset and the user's value was gone. The fix is small but subtle - the
//! list is presets union the current value, plus a remembered slot that survives
//! the `default` stop's key removal - and a reader needs to see all of it together:
//! the options, the step, the remembering, and the writing. Split across a 1,300
//! line file those four pieces drift apart, and this repo has already paid for
//! drift three times (#403, #456, #407).

use super::chat::Chat;

impl Chat {
    /// The stops a row's cycle walks (gh #397): `default`, then the presets
    /// **union the value the file actually holds**, in order.
    ///
    /// Gh #397's defect: the old cycle did
    /// `values.iter().position(|v| *v == current).unwrap_or(0)`, so a value the
    /// presets did not contain - a hand-set `ui.fullscreen_wheel_lines = 4`
    /// against `1, 2, 3, 5, 10` - fell back to index 0 and the first press wrote
    /// the *second* preset, destroying what the user had. A miss was
    /// indistinguishable from "at the start", and the value left the cycle for
    /// good: a one-way door on a config file.
    ///
    /// pi's `settings-selector.ts` has the fix (`...new Set([1, 2, 3, 5, 10,
    /// current])`): put the current value *in* the list. Then a custom value is a
    /// stop like any other, cycling reaches it again, and no separate "remembered
    /// slot" is needed - the file already holds the answer, so the list can carry
    /// it. Numeric lists sort numerically, so `4` sits between `3` and `5`
    /// instead of at the end.
    ///
    /// `default` leads the list because unsetting is a real choice (the key
    /// removed, the built-in default winning) and it is what closes the round
    /// trip: `custom → default → a → b → custom`. It is written as `None` by
    /// [`Self::persisted_for`], the existing unset contract, not as a magic
    /// value.
    /// [`Self::cycle_options`] with the picker's remembered custom slot (gh #397).
    ///
    /// The list alone is not enough to close the round trip: the `default` stop
    /// **removes the key**, so the next read finds no value and the user's custom
    /// number is unreachable again - a live pane showed `4 → 5 → 10 → default →
    /// 1 → 2 → 3 → 5`, losing the `4` exactly as the old code did. The remembered
    /// value keeps it a stop across the removal, which is the brief's
    /// `custom → default → a → b → custom`.
    pub(crate) fn cycle_options_with(
        row: &crate::state::SettingRow,
        remembered: Option<&String>,
    ) -> Vec<String> {
        let mut options: Vec<String> = vec!["default".to_string()];
        options.extend(row.values.iter().cloned());
        // Gh #218: a substituted secret shows as stars; the mask is not a stop,
        // or cycling would write `********` into the file.
        //
        // Gh #397: and a row whose value came from the **default** is not a custom
        // value - it is already "no key set". Adding it as a stop made the cycle
        // land on `default` twice (once as the stop, once as the resolved default)
        // and stop moving, which a live pane showed as a stuck row.
        if !row.masked
            && row.source != "default"
            && !row.value.is_empty()
            && !options.contains(&row.value)
        {
            options.push(row.value.clone());
        }
        // the remembered slot survives the `default` stop's key removal.
        if let Some(remembered) = remembered
            && !row.masked
            && !options.contains(remembered)
        {
            options.push(remembered.clone());
        }
        // Presets keep their declared order (an enum's order is meaningful: the
        // tri-state reads unset/on/off). Numbers sort, because the preset order
        // for a numeric list is arbitrary while the number line is not.
        let numeric = options
            .iter()
            .filter(|value| *value != "default")
            .all(|value| value.parse::<i64>().is_ok());
        if numeric {
            options[1..].sort_by_key(|value| value.parse::<i64>().unwrap_or(i64::MAX));
        }
        options
    }

    /// The stop after (or before) `current` in `options`, wrapping.
    ///
    /// A value that is somehow not in the list starts from `default`, which is
    /// the same "beginning" the old fallback meant - but because the list now
    /// contains every value the file can hold, that branch is unreachable in
    /// practice rather than being the bug.
    pub(crate) fn next_cycle_value(options: &[String], current: &str, forward: bool) -> String {
        if options.is_empty() {
            return current.to_string();
        }
        let position = options
            .iter()
            .position(|value| value == current)
            .unwrap_or(0);
        let next = if forward {
            (position + 1) % options.len()
        } else {
            (position + options.len() - 1) % options.len()
        };
        options[next].clone()
    }

    /// What a cycle stop persists (gh #397): `None` for the `default` stop, so it
    /// removes the key exactly the way the tri-state's `inherit` does.
    pub(crate) fn persisted_for(value: &str, _key: &str) -> Option<String> {
        (value != "default").then(|| value.to_string())
    }

    /// One step on a `/settings` row (gh #30): the next stop on the row's cycle
    /// (gh #397: value-preserving), applied and persisted, then the rows are
    /// re-read so value and source show what actually won.
    pub(crate) fn cycle_setting(
        &mut self,
        picker: &mut crate::chat_pickers::SettingsPicker,
        forward: bool,
    ) -> String {
        // Gh #397: the cursor indexes the *filtered* set, so the row must be
        // resolved through `selected_row()`. Reading `rows[selected]` while a
        // search is active acted on whichever row happened to sit at that index
        // in the unfiltered list - with the query `wheel` it cycled `ui.theme`,
        // and the row the reader was looking at never changed. Found in a live
        // pane; the filter rows above all use `selected_row`, which is why the
        // mismatch was invisible until an edit key was pressed under a filter.
        let Some(row) = picker.selected_row().cloned() else {
            return String::new();
        };
        if row.values.is_empty() {
            return format!("{} is edited through its picker", row.key);
        }
        let (key, current) = (row.key.clone(), row.value.clone());
        let remembered = picker.remembered.get(&key).cloned();
        let options = Self::cycle_options_with(&row, remembered.as_ref());
        // Gh #397: a row whose value came from the **default** is *at* the
        // `default` stop even when its resolved text is something else (a
        // `permissions.mode` row reads `ask` while the file says nothing). Treat
        // it as such, so the first press moves to a real value instead of
        // landing on the `default` stop it is already on.
        // Only when the resolved text is not itself a stop: a `ui.color` row
        // reads `auto`, which *is* its first preset, so it cycles from there; a
        // `permissions.mode` row reads `ask` while the file is silent, and the
        // list has no `ask`, so it is really at `default`.
        let anchor = if options.contains(&current) {
            current.clone()
        } else {
            "default".to_string()
        };
        let next = Self::next_cycle_value(&options, &anchor, forward);
        let notice = match Self::persisted_for(&next, &key) {
            Some(value) => self.apply_setting(&key, &value),
            // The `default` stop: the key goes away, so the built-in default
            // applies again. `persist_setting`'s `None` is the existing unset
            // contract - the same one the tri-state's `inherit` uses.
            None => match &self.world.options.hooks.persist_setting {
                Some(persist) => {
                    persist(&key, None);
                    format!("{key} = default (the key is removed)")
                }
                None => format!("{key} = default"),
            },
        };
        // Gh #397: keep the custom value reachable across the `default` stop,
        // which removes the key. "Custom" means the row holds a value that is
        // neither the `default` stop nor one of the declared presets
        // (`row.values`, not `cycle_options` - the latter already contains the
        // current value, so comparing against it would never find anything).
        //
        // Only a value the **file** supplied is remembered. A row whose source is
        // `default` is showing the built-in resolution of an absent key - after
        // the `default` stop it reads `auto`, and remembering that would evict
        // the user's own `4` from the slot (found in a live pane: press four
        // recorded `auto`, and the `4` never came back).
        if row.source != "default" && current != "default" && !row.values.contains(&current) {
            picker.remembered.insert(key.clone(), current);
        }
        picker.rows = self.settings_rows();
        picker.selected = picker.selected.min(picker.rows.len().saturating_sub(1));
        notice
    }
}
