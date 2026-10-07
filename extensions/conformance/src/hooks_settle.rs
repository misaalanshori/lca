wit_bindgen::generate!({
    path: "../../wit",
    world: "hooks-settle",
    export_macro_name: "export_hooks_settle",
    with: {
        "lca:host/log@0.6.0": generate,
    },
});

use exports::lca::ext::hook_agent_before_settle::Guest as SettleGuest;
use exports::lca::ext::hook_turn_end::{Guest as TurnEndGuest, SettleDecision, SettleSummary};

pub struct HooksSettleComponent;

impl TurnEndGuest for HooksSettleComponent {
    fn on_turn_end(_summary: SettleSummary) -> SettleDecision {
        SettleDecision {
            append: None,
            continue_once: false,
        }
    }
}

impl SettleGuest for HooksSettleComponent {
    fn on_agent_before_settle(_summary: SettleSummary) -> SettleDecision {
        SettleDecision {
            append: None,
            continue_once: false,
        }
    }
}

export_hooks_settle!(HooksSettleComponent);
