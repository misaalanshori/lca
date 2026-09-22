//! Phase 0 host: instantiate a two-function component, measure instantiation
//! and per-event streaming overhead. Prints JSON lines to stdout.

use std::time::Instant;

use wasmtime::component::{Component, HasSelf, Linker, ResourceAny, ResourceTable};
use wasmtime::{Engine, Store, StoreLimitsBuilder};
use wasmtime_wasi::{WasiCtx, WasiCtxView, WasiView};

mod bindings {
    wasmtime::component::bindgen!({ path: "../wit", world: "two-func" });
}

use bindings::TwoFunc;

struct HostState {
    wasi: WasiCtx,
    table: ResourceTable,
    #[allow(dead_code)]
    limits: wasmtime::StoreLimits,
}

impl bindings::lca::spike::api::Host for HostState {
    fn host_add(&mut self, a: u64, b: u64) -> u64 {
        a.wrapping_add(b)
    }
}

impl WasiView for HostState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView { ctx: &mut self.wasi, table: &mut self.table }
    }
}

fn make_engine() -> Engine {
    let mut config = wasmtime::Config::new();
    config.wasm_component_model(true);
    // The interpreter-only build loads precompiled Pulley bytecode artifacts.
    #[cfg(not(feature = "cranelift"))]
    config.target("pulley64").expect("pulley target");
    Engine::new(&config).expect("engine")
}

fn new_store(engine: &Engine) -> Store<HostState> {
    Store::new(
        engine,
        HostState {
            wasi: WasiCtx::builder().build(),
            table: ResourceTable::new(),
            limits: StoreLimitsBuilder::new().build(),
        },
    )
}

fn drain(
    store: &mut Store<HostState>,
    instance: &TwoFunc,
    handle: ResourceAny,
) -> u64 {
    let mut count = 0u64;
    loop {
        let next = instance
            .lca_spike_streams()
            .events()
            .call_next(&mut *store, handle)
            .expect("next");
        match next {
            Some(_) => count += 1,
            None => break,
        }
    }
    count
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mode = args.get(1).map(String::as_str).unwrap_or("bench");

    let t0 = Instant::now();
    let engine = make_engine();
    let engine_ready_ms = t0.elapsed().as_secs_f64() * 1000.0;

    let wasm = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../target/wasm32-wasip2/release/guest.wasm"
    ))
    .expect("read guest component");
    #[cfg(feature = "cranelift")]
    let (precompiled, precompile_ms) = match std::env::var("HOST_ARTIFACT") {
        Ok(path) => (
            std::fs::read(path).expect("read precompiled artifact"),
            0.0,
        ),
        Err(_) => {
            let t = Instant::now();
            let p = engine.precompile_component(&wasm).expect("precompile");
            (p, t.elapsed().as_secs_f64() * 1000.0)
        }
    };
    #[cfg(not(feature = "cranelift"))]
    let (precompiled, precompile_ms) = {
        let path = std::env::var("HOST_ARTIFACT").unwrap_or_else(|_| "/tmp/guest.pulley.wasm".into());
        (std::fs::read(path).expect("read precompiled artifact"), 0.0)
    };

    let t = Instant::now();
    let component = unsafe { Component::deserialize(&engine, &precompiled) }.expect("deserialize");
    let deserialize_us = t.elapsed().as_secs_f64() * 1_000_000.0;

    let mut linker: Linker<HostState> = Linker::new(&engine);
    wasmtime_wasi::p2::add_to_linker_sync(&mut linker).expect("wasi");
    TwoFunc::add_to_linker::<_, HasSelf<_>>(&mut linker, |s| s).expect("link api");

    #[cfg(feature = "cranelift")]
    if mode == "xpile" {
        // Precompile to Pulley bytecode for the no-compiler (interpreter-only) host.
        let out = args.get(2).expect("output path");
        let mut config = wasmtime::Config::new();
        config.wasm_component_model(true);
        if args.get(3).map(String::as_str) != Some("native") {
            config.target("pulley64").expect("pulley target");
        }
        let engine = Engine::new(&config).expect("engine");
        let bytes = engine.precompile_component(&wasm).expect("precompile");
        std::fs::write(out, bytes).expect("write");
        println!("{{\"pulley_artifact_bytes\":{}}}", std::fs::metadata(out).unwrap().len());
        return;
    }
    #[cfg(not(feature = "cranelift"))]
    if mode == "xpile" {
        eprintln!("xpile needs the cranelift host build");
        std::process::exit(2);
    }

    match mode {
        "quick" => {
            let t = Instant::now();
            let mut store = new_store(&engine);
            let instance = TwoFunc::instantiate(&mut store, &component, &linker).expect("instantiate");
            let inst_us = t.elapsed().as_secs_f64() * 1_000_000.0;
            let n = instance.call_compute(&mut store, 41).expect("compute");
            assert_eq!(n, 42);
            let handle = instance.lca_spike_streams().call_start(&mut store).expect("start");
            let t = Instant::now();
            let count = drain(&mut store, &instance, handle);
            let stream_us = t.elapsed().as_secs_f64() * 1_000_000.0;
            println!(
                "{{\"instantiation_us\":{inst_us:.1},\"events\":{count},\"stream_us\":{stream_us:.1}}}"
            );
        }
        "bench" => {
            let runs = args
                .get(2)
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(50);
            let mut inst_times = Vec::with_capacity(runs);
            let mut counts = Vec::new();
            let mut stream_times = Vec::new();
            let mut call_times = Vec::new();
            for _ in 0..runs {
                let t = Instant::now();
                let mut store = new_store(&engine);
                let instance =
                    TwoFunc::instantiate(&mut store, &component, &linker).expect("instantiate");
                inst_times.push(t.elapsed().as_secs_f64() * 1_000_000.0);
                let handle = instance.lca_spike_streams().call_start(&mut store).expect("start");
                let t = Instant::now();
                for i in 0..1_000u64 {
                    let n = instance.call_compute(&mut store, i).expect("compute");
                    assert_eq!(n, i + 1);
                }
                call_times.push(t.elapsed().as_secs_f64() * 1_000_000.0 / 1_000.0);
                let t = Instant::now();
                let count = drain(&mut store, &instance, handle);
                stream_times.push(t.elapsed().as_secs_f64() * 1_000_000.0);
                counts.push(count);
                drop(instance);
                drop(store);
            }
            inst_times.sort_by(|a, b| a.partial_cmp(b).unwrap());
            stream_times.sort_by(|a, b| a.partial_cmp(b).unwrap());
            call_times.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let med = |v: &[f64]| v[v.len() / 2];
            let events = counts[0];
            println!(
                "{{\"runs\":{runs},\"deserialize_us\":{deserialize_us:.1},\"precompile_ms\":{precompile_ms:.1},\"engine_ready_ms\":{engine_ready_ms:.1},\"instantiate_us_median\":{:.1},\"instantiate_us_max\":{:.1},\"events\":{events},\"stream_us_median\":{:.1},\"per_event_us\":{:.3},\"call_us_median\":{:.3}}}",
                med(&inst_times),
                inst_times[inst_times.len() - 1],
                med(&stream_times),
                med(&stream_times) / events as f64,
                med(&call_times)
            );
        }
        other => {
            eprintln!("unknown mode {other}");
            std::process::exit(2);
        }
    }
}
