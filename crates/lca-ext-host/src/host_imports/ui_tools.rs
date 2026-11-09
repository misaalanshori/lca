//! The **interface and tool-domain** imports: `ui-dialogs` (gh #124/gh #172) and
//! the shared `tools` import both tool worlds call (gh #77).

use lca_permissions::DialogPrompt;

use crate::HostState;

thread_local! {
    /// Whether this thread is inside loop-thread dispatch (gh #124):
    /// `render`, `interaction`, and command `invoke` all run on the
    /// TUI loop's own thread, so a dialog there would wait for the
    /// loop to answer itself - a deadlock. Worker threads (tools,
    /// hooks) never set it, so their questions reach the user.
    static DIALOG_FORBIDDEN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Run `call` with dialogs forbidden on this thread (gh #124): the
/// loop-thread dispatch entries (`render`, `interaction`, command
/// `invoke`) wrap their guest call in this.
pub fn without_dialogs<R>(call: impl FnOnce() -> R) -> R {
    DIALOG_FORBIDDEN.with(|forbidden| {
        let was = forbidden.get();
        forbidden.set(true);
        let out = call();
        forbidden.set(was);
        out
    })
}

/// The denial text for a dialog asked on the loop thread.
pub(crate) const LOOP_THREAD_DENIAL: &str =
    "cannot ask a question while handling interface input; ask from a tool or hook instead.";

/// Whether this thread is currently inside loop-thread dispatch.
fn dialogs_forbidden() -> bool {
    DIALOG_FORBIDDEN.with(|forbidden| forbidden.get())
}

/// The `ui-dialogs` import (gh #124/gh #172): through the session's
/// live slot, or the denied values with nothing installed (the
/// headless contract). Every world shares the one canonical import
/// name, so the tool world's registration serves all four.
impl lca_ext_abi::host::tool::lca::host::ui_dialogs::Host for HostState {
    fn confirm(&mut self, title: String, message: String) -> Result<bool, String> {
        if dialogs_forbidden() {
            return Err(LOOP_THREAD_DENIAL.to_string());
        }
        Ok(self.dialogs.confirm(&title, &message))
    }
    fn select(&mut self, title: String, options: Vec<String>) -> Result<Option<String>, String> {
        if dialogs_forbidden() {
            return Err(LOOP_THREAD_DENIAL.to_string());
        }
        Ok(self.dialogs.select(&title, &options))
    }
    fn input(
        &mut self,
        label: String,
        placeholder: Option<String>,
    ) -> Result<Option<String>, String> {
        if dialogs_forbidden() {
            return Err(LOOP_THREAD_DENIAL.to_string());
        }
        Ok(self.dialogs.input(&label, placeholder.as_deref()))
    }
    fn notify(&mut self, message: String, level: String) {
        if dialogs_forbidden() {
            return;
        }
        self.dialogs.notify(&message, &level);
    }
}

/// Shared `tools` import logic (gh #77): one tool calling another
/// through the host. Both the `tool` and `tool-catalog` worlds import
/// the identical interface, so the two impls below share these
/// helpers and map one answer onto each world's generated types.
use lca_ext_abi::host::tool::lca::host::tools::{ActiveToolsResult, NestedResult, ToolName};
use lca_ext_abi::host::tool_catalog::lca::host::tools::{
    ActiveToolsResult as CatalogActiveToolsResult, NestedResult as CatalogNestedResult,
    ToolName as CatalogToolName,
};

impl HostState {
    /// The grant gate every `tools` function passes first: without
    /// the manifest grant the import refuses and records (FR-PERM-3),
    /// and without a loaded registry there is nothing to call through.
    fn tools_gate(&self) -> Result<std::sync::Arc<dyn lca_ext_abi::ToolsRegistryView>, String> {
        if let Err(err) = self.cap.check_tools() {
            return Err(err.to_string());
        }
        self.tools_view
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone()
            .ok_or_else(|| "no turn is running to serve tools".to_string())
    }

    fn tools_execute_answer(&mut self, name: &str, arguments: &str) -> (String, bool, String) {
        let fail = |content: String| (String::new(), true, content);
        let view = match self.tools_gate() {
            Ok(view) => view,
            Err(content) => return fail(content),
        };
        let Some(parent) = self.executing_call.clone() else {
            return fail("no tool call is executing on this store".to_string());
        };
        let Some(slot) = view.nested_slot() else {
            return fail("no turn is running to serve the nested call".to_string());
        };
        let (reply_tx, reply_rx) = std::sync::mpsc::channel();
        let request = lca_ext_abi::NestedCall {
            parent_call_id: parent,
            name: name.to_string(),
            arguments: arguments.to_string(),
            reply: reply_tx,
        };
        // The turn serves on its own task while this thread waits; a
        // dead turn drops the reply, which reads as a failure, never
        // a hang of the host itself (the guest still burns fuel).
        if slot.send(request).is_err() {
            return fail("the turn is gone; the nested call was dropped".to_string());
        }
        match reply_rx.recv() {
            Ok(result) => (
                result.call_id,
                result.status != lca_protocol::ToolResultStatus::Ok,
                result.content,
            ),
            Err(_) => fail("the turn dropped the nested call".to_string()),
        }
    }
}

impl lca_ext_abi::host::tool::lca::host::tools::Host for HostState {
    fn execute_tool(&mut self, name: String, arguments: String) -> NestedResult {
        let (call_id, is_error, content) = self.tools_execute_answer(&name, &arguments);
        NestedResult {
            call_id,
            is_error,
            content,
        }
    }
    fn list_tools(&mut self) -> Vec<ToolName> {
        self.tools_gate()
            .map(|view| view.callable_names())
            .unwrap_or_default()
            .into_iter()
            .map(|(name, description)| ToolName { name, description })
            .collect()
    }
    fn set_active_tools(&mut self, names: Vec<String>) -> ActiveToolsResult {
        let (applied, ignored) = self
            .tools_gate()
            .map(|view| view.set_active_tools(&names))
            .unwrap_or_default();
        ActiveToolsResult { applied, ignored }
    }
    fn get_active_tools(&mut self) -> Vec<String> {
        self.tools_gate()
            .map(|view| view.active_tools())
            .unwrap_or_default()
    }
}

impl lca_ext_abi::host::tool_catalog::lca::host::tools::Host for HostState {
    fn execute_tool(&mut self, name: String, arguments: String) -> CatalogNestedResult {
        let (call_id, is_error, content) = self.tools_execute_answer(&name, &arguments);
        CatalogNestedResult {
            call_id,
            is_error,
            content,
        }
    }
    fn list_tools(&mut self) -> Vec<CatalogToolName> {
        self.tools_gate()
            .map(|view| view.callable_names())
            .unwrap_or_default()
            .into_iter()
            .map(|(name, description)| CatalogToolName { name, description })
            .collect()
    }
    fn set_active_tools(&mut self, names: Vec<String>) -> CatalogActiveToolsResult {
        let (applied, ignored) = self
            .tools_gate()
            .map(|view| view.set_active_tools(&names))
            .unwrap_or_default();
        CatalogActiveToolsResult { applied, ignored }
    }
    fn get_active_tools(&mut self) -> Vec<String> {
        self.tools_gate()
            .map(|view| view.active_tools())
            .unwrap_or_default()
    }
}
