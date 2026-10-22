//! The scenario-argument helpers for `conformance_diff` (split out when the
//! file passed gate 11's 1,200-line ceiling; behaviour unchanged).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use lca_protocol::ToolCall;

pub(crate) fn call(mode_args: &str) -> ToolCall {
    ToolCall {
        call_id: "c1".to_string(),
        name: "conformance".to_string(),
        arguments: mode_args.to_string(),
        parent_call_id: None,
    }
}

pub(crate) fn spawn_args(program: &str, marker: &str) -> String {
    if cfg!(unix) {
        format!(r#"{{"mode":"spawn","program":"{program}","args":["{marker}"],"cwd":"workspace"}}"#)
    } else {
        format!(
            r#"{{"mode":"spawn","program":"{program}","args":["/C","echo","{marker}"],"cwd":"workspace"}}"#
        )
    }
}

/// An `env-get` scenario (gh #170): `present:<value>` or `absent`.
pub(crate) fn env_get_args(name: &str) -> String {
    format!(r#"{{"mode":"env-get","name":"{name}"}}"#)
}

/// A whole GET (gh #302), so request, status, body and close all compare.
pub(crate) fn net_fetch_args(url: &str) -> String {
    format!(r#"{{"mode":"net-fetch","url":"{url}"}}"#)
}

pub(crate) fn pty_args(program: &str, marker: &str) -> String {
    if cfg!(unix) {
        format!(
            r#"{{"mode":"pty","program":"{program}","args":["{marker}"],"cwd":"workspace","rows":24,"cols":80}}"#
        )
    } else {
        format!(
            r#"{{"mode":"pty","program":"{program}","args":["/C","echo","{marker}"],"cwd":"workspace","rows":24,"cols":80}}"#
        )
    }
}
