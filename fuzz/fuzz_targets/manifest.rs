//! The manifest parser (testing plan section 13: fuzz the manifest
//! parser). Every input is either rejected cleanly or parsed into a
//! Manifest the loader would accept - no panics either way.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    if let Ok(manifest) = lca_ext_host::Manifest::parse(&text) {
        // A parseable manifest must survive every accessor the host
        // uses right after parsing.
        let _ = manifest.abi_in_window();
        assert!(!manifest.name.is_empty());
        assert!(!manifest.worlds.is_empty());
    }
});
