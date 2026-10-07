wit_bindgen::generate!({
    path: "../../wit",
    world: "hooks-compaction",
    export_macro_name: "export_hooks_compaction",
    with: {
        "lca:host/log@0.6.0": generate,
    },
});

use exports::lca::ext::hook_session_before_compact::{CompactVerdict, Guest as BeforeGuest};
use exports::lca::ext::hook_session_compact_failed::Guest as FailedGuest;

pub struct HooksCompactionComponent;

impl BeforeGuest for HooksCompactionComponent {
    fn on_session_before_compact(_reason: String) -> CompactVerdict {
        CompactVerdict::Allow
    }
}

impl FailedGuest for HooksCompactionComponent {
    fn on_session_compact_failed(_reason: String, _error: Option<String>) {}
}

export_hooks_compaction!(HooksCompactionComponent);
