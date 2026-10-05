//! Headless-contract parity: the exit-code map and the usage envelope fields.
//! Pi's JSONL framing rule matches; the event vocabulary itself is an LCA
//! shape (see `docs/pi-parity.md`).

use lca_cli::{exit, exit_code};
use lca_protocol::{StopReason, TurnOutcome, TurnStatus, Usage};

fn outcome(status: TurnStatus, stop: StopReason, class: Option<&str>) -> TurnOutcome {
    TurnOutcome {
        status,
        stop_reason: stop,
        usage: Usage::default(),
        error: class.map(|c| format!("{c} failure")),
    }
}

// Verifies: pi:packages/coding-agent/docs/json.md#framing-and-process-io
// (stdout is machine-readable records; the process exit still signals the
// run's fate — LCA's `docs/headless.md` exit-code table).
#[test]
fn pi_parity_headless_exit_code_map() {
    assert_eq!(
        exit_code(
            &outcome(TurnStatus::Ok, StopReason::Stop, None),
            false,
            None
        ),
        exit::OK
    );
    assert_eq!(
        exit_code(
            &outcome(TurnStatus::Error, StopReason::Stop, None),
            true,
            None
        ),
        exit::PERMISSION,
        "a needed approval outranks every other signal"
    );
    assert_eq!(
        exit_code(
            &outcome(TurnStatus::Error, StopReason::IterationLimit, None),
            false,
            None
        ),
        exit::ABORTED
    );
    assert_eq!(
        exit_code(
            &outcome(TurnStatus::Error, StopReason::Cancelled, None),
            false,
            None
        ),
        exit::ABORTED
    );
    for class in ["transport", "auth", "invalid"] {
        assert_eq!(
            exit_code(
                &outcome(TurnStatus::Error, StopReason::Error, None),
                false,
                Some(class)
            ),
            exit::PROVIDER,
            "class {class} is a provider error"
        );
    }
    assert_eq!(
        exit_code(
            &outcome(TurnStatus::Error, StopReason::Error, None),
            false,
            Some("internal")
        ),
        exit::INTERNAL
    );
    assert_eq!(
        exit_code(
            &outcome(TurnStatus::Error, StopReason::Error, None),
            false,
            Some("other")
        ),
        exit::ABORTED
    );
    assert_eq!(
        exit_code(
            &outcome(TurnStatus::Error, StopReason::Error, None),
            false,
            None
        ),
        exit::INTERNAL
    );
}

// Verifies: pi:packages/coding-agent/docs/cli-integration.md (a script
// can rely on the process exit; LCA's release policy freezes these codes).
#[test]
fn pi_parity_exit_code_values_are_stable() {
    assert_eq!(exit::OK, 0);
    assert_eq!(exit::INTERNAL, 1);
    assert_eq!(exit::USAGE, 2);
    assert_eq!(exit::PROVIDER, 3);
    assert_eq!(exit::PERMISSION, 4);
    assert_eq!(exit::ABORTED, 5);
    assert_eq!(exit::SESSION, 6);
}

// Verifies: pi:packages/coding-agent/docs/json.md#agent-and-turn-events
// (a usage report carries cache counts beside input/output — LCA's
// `usage` envelope carries the same five buckets).
#[test]
fn pi_parity_usage_envelope_carries_cache_fields() {
    let usage = Usage {
        input: 40,
        output: 12,
        cache_read: 1200,
        cache_write: 300,
        cache_write_1h: 0,
        cost: 0.001,
        ..Default::default()
    };
    let value = serde_json::to_value(&usage).expect("usage serializes");
    assert_eq!(value["input"], 40);
    assert_eq!(value["output"], 12);
    assert_eq!(
        value["cache_read"], 1200,
        "cache buckets ride every usage report"
    );
    assert_eq!(value["cache_write"], 300);
    assert!(value["cost"].as_f64().expect("cost") > 0.0);
    let back: Usage = serde_json::from_value(value).expect("usage round-trips");
    assert_eq!(back, usage);
}
