//! Session debug traces (gh #258 §3B): append shapes and lineage
//! walks. Split from `store.rs` under the workspace's 1,200-line file
//! ceiling; every value crossing in is content-free by construction.

/// Trace one append: type, id, and bytes — never content.
///
/// The type name comes from `Record::type_tag`, the one place the record
/// vocabulary is spelled out (gh #339 finding 3). This module used to carry its own
/// 20-arm copy, arm for arm identical to the protocol's; a second copy of a
/// vocabulary that gains an arm per record type is the drift this repo keeps
/// closing, and the copy here would have been the *silent* one (a debug trace
/// nobody diffs against the protocol).
pub(crate) fn trace_append(record: &lca_protocol::Record, bytes: usize) {
    lca_debug::session::event(
        "record.append",
        &format!(
            "type={} id={} bytes={bytes}",
            record.type_tag(),
            record.id().unwrap_or("?")
        ),
    );
}
