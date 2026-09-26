//! The ABI decode path (testing plan section 13): arbitrary bytes
//! through Wasmtime's component decoder and linker - the gate every
//! downloaded artifact must survive anyway (FR-DIST-3's other half).
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() < 8 {
        return;
    }
    let mut config = wasmtime::Config::new();
    config.wasm_component_model(true);
    let Ok(engine) = wasmtime::Engine::new(&config) else {
        return;
    };
    // The same call the loader makes on a downloaded artifact: parse
    // and validate without instantiating - this is where a malformed
    // download dies (FR-DIST-3's other half).
    let _ = wasmtime::component::Component::new(&engine, data);
});
