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

/// The gate every `ui-dialogs` function passes first (gh #124): on the loop
/// thread a dialog would wait for the loop to answer itself, so it is refused
/// with a message the guest can carry back to the model.
///
/// One function rather than the same `if` four times, because `questionnaire`
/// (gh #407) made it five: a gate restated per arm is a gate an arm eventually
/// forgets, and the class is exactly what the unit row below pins.
fn dialog_gate() -> Result<(), String> {
    if dialogs_forbidden() {
        Err(LOOP_THREAD_DENIAL.to_string())
    } else {
        Ok(())
    }
}

use lca_ext_abi::host::tool::lca::host::ui_dialogs::{Answer, Question};

/// The `ui-dialogs` import (gh #124/gh #172): through the session's
/// live slot, or the denied values with nothing installed (the
/// headless contract). Every world shares the one canonical import
/// name, so the tool world's registration serves all four.
impl lca_ext_abi::host::tool::lca::host::ui_dialogs::Host for HostState {
    fn confirm(&mut self, title: String, message: String) -> Result<bool, String> {
        dialog_gate()?;
        Ok(self.dialogs.confirm(&title, &message))
    }
    fn select(&mut self, title: String, options: Vec<String>) -> Result<Option<String>, String> {
        dialog_gate()?;
        Ok(self.dialogs.select(&title, &options))
    }
    fn input(
        &mut self,
        label: String,
        placeholder: Option<String>,
    ) -> Result<Option<String>, String> {
        dialog_gate()?;
        Ok(self.dialogs.input(&label, placeholder.as_deref()))
    }
    fn notify(&mut self, message: String, level: String) {
        if dialog_gate().is_err() {
            return;
        }
        self.dialogs.notify(&message, &level);
    }

    /// A tabbed questionnaire (gh #407): map the WIT records both ways.
    ///
    /// The loop-thread refusal is an **error** here, like `select`, and not
    /// a silent dismissal: a guest asking from `render` would otherwise read
    /// "the user declined" as an answer. `Ok(None)` is kept for the honest
    /// dismissal the surface reports (Esc) and for the headless slot, whose
    /// default answers nothing - a difference the guest can act on.
    fn questionnaire(
        &mut self,
        title: String,
        questions: Vec<Question>,
    ) -> Result<Option<Vec<Answer>>, String> {
        dialog_gate()?;
        let asked: Vec<lca_permissions::Question> = questions
            .into_iter()
            .map(|question| lca_permissions::Question {
                header: question.header,
                prompt: question.prompt,
                options: question.options,
                multiple: question.multiple,
            })
            .collect();
        Ok(self.dialogs.questionnaire(&title, &asked).map(|answers| {
            answers
                .into_iter()
                .map(|answer| Answer {
                    header: answer.header,
                    value: answer.value,
                    custom: answer.custom,
                })
                .collect()
        }))
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

#[cfg(test)]
mod tests {
    use super::*;

    // Verifies: gh #124, extended by gh #407 - every dialog is refused on the
    // loop thread with the message the guest can read, and allowed anywhere
    // else. The row pins the **gate** rather than one arm because the arms are
    // the same three lines five times over; what it cannot prove is that each
    // arm calls it, which is why the gate returns `Result` instead of a `bool`
    // (an arm that forgets it does not compile with `?` dropped for a value).
    #[test]
    fn the_dialog_gate_refuses_on_the_loop_thread_and_allows_off_it() {
        assert!(
            dialog_gate().is_ok(),
            "a worker thread (a tool, a hook) may ask"
        );
        let refused = without_dialogs(dialog_gate);
        assert_eq!(
            refused,
            Err(LOOP_THREAD_DENIAL.to_string()),
            "loop-thread dispatch refuses with the guest-readable reason"
        );
        // Nested and restored: the flag is saved and put back, so a tool
        // dispatched from inside an interaction still sees the refusal lifted
        // outside the nested call.
        without_dialogs(|| {
            assert!(dialog_gate().is_err(), "inside dispatch");
            without_dialogs(|| assert!(dialog_gate().is_err(), "nested"));
        });
        assert!(dialog_gate().is_ok(), "restored after the outer call");
    }
}
