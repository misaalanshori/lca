wit_bindgen::generate!({
    path: "../../wit",
    world: "hooks-cache",
    export_macro_name: "export_hooks_cache",
    with: {
        "lca:host/log@0.6.0": generate,
    },
});

use exports::lca::ext::hook_cache_warming::{CacheDecision, Guest};

pub struct HooksCacheComponent;

impl Guest for HooksCacheComponent {
    fn on_cache_warming_decision(_provider: String, model: String) -> CacheDecision {
        CacheDecision {
            warm: !model.contains("no-warm"),
        }
    }
}

export_hooks_cache!(HooksCacheComponent);
