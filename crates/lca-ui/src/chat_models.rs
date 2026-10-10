//! The model catalog snapshot and the async `/model` picker path,
//! split from `chat_commands.rs` (the 1,200-line ceiling): gh #233's
//! lazy snapshot plus gh #232's background discovery and loader.

use super::chat::Chat;
use crate::chat_pickers::ModelPicker;

impl Chat {
    /// The models `/model` should offer: the host's live list when one is
    /// wired, else the startup snapshot (a login's discovery reaches the
    /// picker without a restart). Rows, not bare ids: the label decorates,
    /// the id stays raw (G2).
    pub fn model_rows(&self) -> Vec<crate::state::ModelRow> {
        self.world
            .options
            .hooks
            .models
            .as_ref()
            .map(|list| list())
            .unwrap_or_else(|| self.world.options.models.clone())
    }

    /// Read the snapshot without enumerating (gh #232): the instant
    /// `/model` path never calls the hook on the UI thread. `None`
    /// means unlisted; `Some` empty means listed-but-empty (an
    /// endpoint still waiting on consent).
    pub(crate) fn model_rows_snapshot(&self) -> Option<Vec<crate::state::ModelRow>> {
        self.model_rows_cache.clone()
    }

    /// Open the picker instantly (gh #232): snapshot rows show on the
    /// same frame with no refresh; a missing (or empty) snapshot opens
    /// a loading picker and enumerates on a background thread, which
    /// `poll_model_refresh` collects. Never blocks on providers. A
    /// host without a list hook gets the startup snapshot (a cheap
    /// clone); empty still falls through to consent below.
    pub(crate) fn open_model_picker(&mut self) {
        // A usable snapshot answers on the same frame; missing or
        // listed-but-empty falls to the loader below (consent may
        // still answer).
        match self.model_rows_snapshot() {
            Some(rows) if !rows.is_empty() => {
                let mut picker = ModelPicker::new(rows);
                picker.loading = false;
                self.model_picker = Some(picker);
                return;
            }
            _ => {}
        }
        let Some(list) = self.world.options.hooks.models.clone() else {
            let rows = self.world.options.models.clone();
            if !rows.is_empty() {
                self.model_picker = Some(ModelPicker::new(rows));
            }
            return;
        };
        let mut picker = ModelPicker::new(Vec::new());
        picker.loading = true;
        self.model_picker = Some(picker);
        if self.model_refresh.is_none() {
            let (tx, rx) = std::sync::mpsc::channel();
            self.model_refresh = Some(rx);
            std::thread::spawn(move || {
                let _ = tx.send(list());
            });
        }
    }

    /// Collect a landed background discovery (gh #232): stash the
    /// snapshot, fill an open loading picker in place (the query and
    /// cursor survive via `refilter`), and on an empty discovery close
    /// the loader and run the extension `model` command so endpoint
    /// consent (gh #31) still fires. Returns `true` when rows landed.
    pub(crate) fn poll_model_refresh(&mut self) -> bool {
        let rows = match self.model_refresh.as_ref().map(|rx| rx.try_recv()) {
            Some(Ok(rows)) => rows,
            _ => return false,
        };
        self.model_refresh = None;
        self.apply_model_refresh(rows);
        true
    }

    /// The blocking drain tests use to make the background arrival
    /// deterministic (the hook answers instantly there).
    #[cfg(test)]
    pub(crate) fn drain_model_refresh(&mut self) {
        let rows = self.model_refresh.take().and_then(|rx| rx.recv().ok());
        if let Some(rows) = rows {
            self.apply_model_refresh(rows);
        }
    }

    /// Stash arriving rows and settle the open picker (gh #232).
    fn apply_model_refresh(&mut self, rows: Vec<crate::state::ModelRow>) {
        self.model_rows_cache = Some(rows.clone());
        let Some(picker) = self.model_picker.as_mut() else {
            return;
        };
        if !picker.loading {
            return;
        }
        if rows.is_empty() {
            self.model_picker = None;
            self.dispatch_extension_command("model", "");
            return;
        }
        picker.models = rows;
        picker.loading = false;
        picker.refilter();
        picker.selected = picker.selected.min(picker.matches.len().saturating_sub(1));
    }

    /// Clear the catalog snapshot (gh #233): login, switch, grant,
    /// reload, and scope events all change what providers offer, so
    /// the next `/model` re-enumerates. A pending background arrival
    /// dies with it (gh #232): its rows predate the event.
    pub(crate) fn invalidate_models(&mut self) {
        self.model_rows_cache = None;
        self.model_refresh = None;
    }
}
