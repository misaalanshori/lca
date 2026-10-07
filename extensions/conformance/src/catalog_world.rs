wit_bindgen::generate!({
    path: "../../wit",
    world: "tool-catalog",
    export_macro_name: "export_tool_catalog",
    with: {
        "lca:host/log@0.6.0": generate,
        "lca:host/fs@0.6.0": generate,
        "lca:host/process@0.6.0": generate,
        "lca:host/pty@0.6.0": generate,
        "lca:host/ui-dialogs@0.6.0": generate,
        "lca:host/resources@0.6.0": generate,
        "lca:host/state@0.6.0": generate,
        "lca:host/tools@0.6.0": generate,
    },
});

// Reuse the single-tool world's capability view: one guest, one host.
use crate::{mode_and_args, tool_world::GuestCap};
use exports::lca::ext::catalog::{Guest as CatalogGuest, ToolAnnotations, ToolNamespace, ToolSpec};
use exports::lca::ext::catalog_run::{Guest as RunGuest, ToolResult as WasmResult};
use lca::ext::types::ToolCall;

fn to_wit_spec(spec: &lca_protocol::ToolSpec) -> ToolSpec {
    ToolSpec {
        name: spec.name.clone(),
        description: spec.description.clone(),
        parameters: spec.parameters.to_string(),
        exposure: spec.exposure.as_str().to_string(),
        namespace: spec.namespace.as_ref().map(|namespace| ToolNamespace {
            name: namespace.name.clone(),
            description: namespace.description.clone(),
            instructions: namespace.instructions.clone(),
        }),
        annotations: spec
            .annotations
            .as_ref()
            .map(|annotations| ToolAnnotations {
                read_only_hint: annotations.read_only_hint,
                destructive_hint: annotations.destructive_hint,
                idempotent_hint: annotations.idempotent_hint,
                open_world_hint: annotations.open_world_hint,
            }),
        extras: spec
            .extras
            .iter()
            .map(|(key, value)| lca::ext::types::ExtraPair {
                key: key.clone(),
                value: value.clone(),
            })
            .collect(),
    }
}

pub struct CatalogComponent;

impl CatalogGuest for CatalogComponent {
    fn get_tools() -> Vec<ToolSpec> {
        crate::catalog_specs().iter().map(to_wit_spec).collect()
    }
}

impl RunGuest for CatalogComponent {
    fn run_tool(name: String, call: ToolCall) -> WasmResult {
        // Every suite tool dispatches through the same shared modes;
        // the name only matters for exposure, never behavior.
        let _ = name;
        let (mode, args) = mode_and_args(&call.arguments);
        if mode == "log" {
            lca::host::log::info(&"x".repeat(50_000));
        }
        let outcome = crate::run_tool_mode(&GuestCap, &call.call_id, &mode, &args);
        WasmResult {
            call_id: call.call_id,
            status: if outcome.ok { "ok" } else { "error" }.to_string(),
            content: Some(outcome.text),
            truncated: false,
            extras: Vec::new(),
        }
    }
}

export_tool_catalog!(CatalogComponent);
