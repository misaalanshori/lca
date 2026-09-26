//! The plain-HTTPS archive reader (testing plan section 13: the
//! HTTPS-archive resolver): bytes in, either the package payload or a clean
//! error - never a panic inside the zip machinery.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(archive) = lca_registry::read_archive(data) {
        assert!(!archive.manifest.is_empty());
        // A package carries a component, or declares resources and is
        // data-only. Either way the manifest parses.
        assert!(
            !archive.component.is_empty() || !archive.resources.is_empty(),
            "an archive that parsed carries something"
        );
        if let Ok(hash) = lca_registry::grant_hash(&archive.manifest) {
            assert!(hash.starts_with("sha256:"));
        }
        // The component's digest is what the installer records
        // (FR-DIST-3's value). A data-only package has no component, so
        // the digest is only meaningful when there is one.
        if !archive.component.is_empty() {
            let digest = lca_registry::Resolved::digest_of(&archive.component);
            assert!(digest.starts_with("sha256:"));
        }
        // Resources survive the read with their paths and bytes intact,
        // and never exceed the per-package cap the reader enforces.
        let mut total = 0u64;
        for (path, bytes) in &archive.resources {
            assert!(!path.is_empty(), "a resource path is non-empty");
            total += bytes.len() as u64;
        }
        assert!(total <= lca_registry::RESOURCE_PACKAGE_MAX_BYTES);
    }
});
