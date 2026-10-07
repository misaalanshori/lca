wit_bindgen::generate!({
    path: "../../wit",
    world: "hooks-trust",
    export_macro_name: "export_hooks_trust",
    with: {
        "lca:host/log@0.6.0": generate,
    },
});

use exports::lca::ext::hook_project_trust::{Guest, TrustDecision};

pub struct HooksTrustComponent;

impl Guest for HooksTrustComponent {
    fn on_project_trust(cwd: String) -> TrustDecision {
        if cwd.contains("trust-yes") {
            TrustDecision {
                trusted: "yes".to_string(),
                remember: true,
            }
        } else if cwd.contains("trust-no") {
            TrustDecision {
                trusted: "no".to_string(),
                remember: false,
            }
        } else {
            TrustDecision {
                trusted: "undecided".to_string(),
                remember: false,
            }
        }
    }
}

export_hooks_trust!(HooksTrustComponent);
