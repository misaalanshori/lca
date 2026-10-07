wit_bindgen::generate!({
    path: "../../wit",
    world: "hooks-stream",
    export_macro_name: "export_hooks_stream",
    with: {
        "lca:host/log@0.6.0": generate,
    },
});

use exports::lca::ext::hook_stream_event::Guest;

pub struct HooksStreamComponent;

impl Guest for HooksStreamComponent {
    fn on_stream_event(_provider: String, _model: String, _kind: String, _data: String) {}
}

export_hooks_stream!(HooksStreamComponent);
