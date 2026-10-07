wit_bindgen::generate!({
    path: "../../wit",
    world: "compaction",
    export_macro_name: "export_compaction",
    with: {
        "lca:host/log@0.6.0": generate,
        "lca:host/completion@0.6.0": generate,
        "lca:host/types@0.6.0": generate,
        "lca:host/resources@0.6.0": generate,
        "lca:host/state@0.6.0": generate,
    },
});

use exports::lca::ext::compact::{Guest as CompactGuest, SessionRecord};
use lca::host::completion;

fn to_capability(err: completion::Error) -> String {
    use lca_protocol::CapabilityError as E;
    match err {
        completion::Error::Permission(d) => E::Permission(d).to_string(),
        completion::Error::NotGranted(d) => E::NotGranted(d).to_string(),
        completion::Error::Io(d) => E::Io(d).to_string(),
        completion::Error::Invalid(d) => E::Invalid(d).to_string(),
    }
}

pub struct CompactionWasm;

impl CompactGuest for CompactionWasm {
    fn compact(records: Vec<SessionRecord>) -> Result<String, String> {
        let excerpts: Vec<(String, String)> = records
            .into_iter()
            .map(|record| (record.kind, record.body))
            .collect();
        let ask = || -> Result<String, String> {
            let request = completion::Message {
                role: "user".to_string(),
                content: "conformance completion request".to_string(),
                tool_calls: Vec::new(),
                tool_call_id: None,
                extras: Vec::new(),
            };
            let response = completion::request(&[request]).map_err(to_capability)?;
            Ok(response.text)
        };
        crate::compact_script(&excerpts, Some(&ask as &dyn Fn() -> Result<String, String>))
    }
}

export_compaction!(CompactionWasm);
