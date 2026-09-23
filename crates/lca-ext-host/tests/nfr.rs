//! Performance gates owned by the extension host: instantiation (NFR-4),
//! hook-call overhead (NFR-5), and epoch-interruption latency (NFR-29).
//!
//! Measured here in debug builds against a real component; the pipeline's
//! size-and-startup gate re-measures the release binary (NFR-7 discipline,
//! docs/phase0-report.md).

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lca_ext_abi::ExtensionDispatch;
use lca_ext_host::{ExtHost, ExtensionLimits, HostEnvironment};
use lca_permissions::{Decision, GrantStore, PermissionPrompt, ProposalDiff, ScopeRoots};
use lca_protocol::ToolCall;

const FIXTURE: &[u8] = include_bytes!("../../../extensions/conformance/fixtures/tool-world.wasm");
const MANIFEST: &str = include_str!("../../../extensions/conformance/extension.toml");

struct Always;
impl PermissionPrompt for Always {
    fn ask(&mut self, _action: &lca_permissions::Action) -> Decision {
        Decision::Always
    }
    fn review_proposals(&mut self, _diff: &ProposalDiff) -> bool {
        false
    }
}

fn load() -> (lca_ext_host::WasmExtension, ExtHost) {
    let root = std::env::temp_dir().join(format!("lca-nfr-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for dir in ["project", "private", "config", "data", "tmp"] {
        std::fs::create_dir_all(root.join(dir)).expect("mkdir");
    }
    let env = Arc::new(HostEnvironment {
        roots: ScopeRoots {
            workspace: root.join("project"),
            private: root.join("private"),
            home_config: root.join("config"),
            temp: root.join("tmp"),
            state_dir: root.join("data"),
        },
        prompt: Arc::new(Mutex::new(Always)),
        grant_store: Arc::new(Mutex::new(
            GrantStore::open(&root.join("grants.json")).expect("store"),
        )),
        project: root.join("project"),
        proposals: None,
    });
    let mut host = ExtHost::new(
        ExtensionLimits {
            memory_bytes: 64 * 1024 * 1024,
            fuel_per_call: u64::MAX, // NFR-29 isolates epoch latency from fuel
            log_limit_bytes: 4096,
        },
        env,
    );
    let extension = host.load(FIXTURE, MANIFEST).expect("load");
    (extension, host)
}

fn median(values: &[Duration]) -> Duration {
    let mut sorted = values.to_vec();
    sorted.sort();
    sorted[sorted.len() / 2]
}

// Verifies: NFR-4 (instantiation of one precompiled WASM extension does
// not exceed20 ms). Each `schema` call builds a fresh store, links, and
// instantiates before it answers, so its median is the instantiation
// budget plus the extension's own trivial work.
#[test]
fn instantiation_stays_within_twenty_milliseconds() {
    let (extension, _host) = load();
    // Warm one call so the measure is steady-state, as ADR-0001's
    // precompile-per-digest model assumes.
    extension.schema().expect("warmup");
    let mut samples = Vec::new();
    for _ in 0..20 {
        let started = Instant::now();
        extension.schema().expect("schema");
        samples.push(started.elapsed());
    }
    let typical = median(&samples);
    assert!(
        typical <= Duration::from_millis(20),
        "instantiation median {typical:?} exceeds NFR-4's20 ms bound"
    );
}

// Verifies: NFR-5 (a hook call into a WASM extension does not exceed1 ms
// of overhead above the extension's own work; the conformance hook is a
// name check, so the measured round trip is essentially all overhead).
#[test]
fn hook_calls_stay_within_one_millisecond_of_overhead() {
    let (extension, _host) = load();
    let call = ToolCall {
        call_id: "c1".to_string(),
        name: "read".to_string(),
        arguments: "{}".to_string(),
    };
    futures_executor_block_on(extension.on_pre_tool_use(&call)).expect("warmup");
    let rounds = 30;
    let started = Instant::now();
    for _ in 0..rounds {
        futures_executor_block_on(extension.on_pre_tool_use(&call)).expect("hook");
    }
    let average = started.elapsed() / rounds;
    // The threshold is the SRDD's1 ms. A debug wasmtime build carries
    // enough instrumentation that instantiation alone dominates it, so
    // debug runs hold a looser ceiling and the release-mode run (CI's
    // `nfr release gates` step) asserts the real bound.
    let bound = if cfg!(debug_assertions) {
        // Generous: debug wasmtime plus a parallel test run's contention.
        Duration::from_millis(50)
    } else {
        Duration::from_millis(1)
    };
    assert!(
        average <= bound,
        "hook round trip averages {average:?}, above NFR-5's {bound:?} bound"
    );
}

thread_local! {
    /// One runtime for the whole test thread: building it per call would
    /// measure the runtime, not the extension (outside any async context,
    /// so synchronous WASI inside the blocking pool works as designed,
    /// ADR-0014).
    static RUNTIME: tokio::runtime::Runtime =
        tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime");
}

/// Drive one boxed dispatch future to completion on this (non-async) test
/// thread.
fn futures_executor_block_on<T>(future: lca_ext_abi::DispatchFuture<'_, T>) -> T {
    RUNTIME.with(|runtime| runtime.block_on(future))
}

// Verifies: NFR-29 (cancelling a running WASM extension call takes effect
// within50 ms of the epoch increment, measured from increment to the
// instance trapping).
#[test]
fn epoch_interruption_traps_within_fifty_milliseconds() {
    let (extension, host) = load();
    let engine = host.engine().clone();

    let spinner = std::thread::spawn(move || {
        // A spinning call on its own blocking thread, fuel unlimited.
        let _ = extension.execute(&call_loop());
    });
    // Let it get well into the spin.
    std::thread::sleep(Duration::from_millis(80));

    let started = Instant::now();
    engine.increment_epoch();
    spinner.join().expect("spinner returns");
    let latency = started.elapsed();

    assert!(
        latency <= Duration::from_millis(50),
        "epoch increment to trap took {latency:?}, above NFR-29's50 ms bound"
    );
}

fn call_loop() -> ToolCall {
    ToolCall {
        call_id: "spin".to_string(),
        name: "conformance".to_string(),
        arguments: r#"{"mode":"loop"}"#.to_string(),
    }
}
