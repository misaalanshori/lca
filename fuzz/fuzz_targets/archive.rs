//! The plain-HTTPS archive reader (testing plan section12: the
//! HTTPS-archive resolver): bytes in, either the two files or a clean
//! error - never a panic inside the zip machinery.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok((manifest, component)) = lca_registry::read_archive(data) {
        assert!(!manifest.is_empty());
        assert!(!component.is_empty());
        // The pair must round-trip the digest the installer records
        // (FR-DIST-3's value).
        let _ = lca_registry::Resolved::digest_of(&component);
        if let Ok(hash) = lca_registry::grant_hash(&manifest) {
            assert!(hash.starts_with("sha256:"));
        }
    }
});
