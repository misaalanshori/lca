//! The shared capability traits (`lca-protocol`) for [`Capabilities`]: the
//! native host's side of the provider world's imports. Split out of
//! `mod.rs` for the line ceiling.

use super::{Capabilities, CapabilityError};

// ---------------------------------------------------------------------------
// The shared capability traits (lca-protocol): native mode's side of
// the provider world's imports
// ---------------------------------------------------------------------------

impl lca_protocol::ProviderCap for Capabilities {
    fn net_request(
        &self,
        method: &str,
        url: &str,
        headers: &[(&str, &str)],
        body: Option<&[u8]>,
    ) -> Result<u32, CapabilityError> {
        Capabilities::net_request(self, method, url, headers, body)
    }

    fn net_response_status(&self, handle: u32) -> Result<u16, CapabilityError> {
        Capabilities::net_response_status(self, handle)
    }

    fn net_read_body(&self, handle: u32, max: usize) -> Result<Option<Vec<u8>>, CapabilityError> {
        Capabilities::net_read_body(self, handle, max)
    }

    fn net_close_response(&self, handle: u32) -> Result<(), CapabilityError> {
        Capabilities::net_close_response(self, handle)
    }

    fn credentials_get(&self, key: &str) -> Option<String> {
        // Denial reads as absence (capability catalog): checking for an
        // existing login needs no denial/absence distinction.
        Capabilities::credentials_get(self, key).unwrap_or(None)
    }

    fn credentials_set(&self, key: &str, value: &str) -> Result<(), CapabilityError> {
        Capabilities::credentials_set(self, key, value)
    }

    fn credentials_delete(&self, key: &str) -> Result<(), CapabilityError> {
        Capabilities::credentials_delete(self, key)
    }

    fn resource_read(&self, path: &str) -> Result<Vec<u8>, CapabilityError> {
        Capabilities::resource_read(self, path)
    }
}

impl lca_protocol::OauthCap for Capabilities {
    fn oauth_begin(&self, redirect_path: &str) -> Result<(String, u32), CapabilityError> {
        Capabilities::oauth_begin(self, redirect_path)
    }

    fn oauth_open(&self, url: &str) -> Result<(), CapabilityError> {
        Capabilities::oauth_open(self, url)
    }

    fn oauth_await(&self, handle: u32) -> Result<Vec<(String, String)>, CapabilityError> {
        Capabilities::oauth_await(self, handle)
    }

    fn oauth_end(&self, handle: u32) -> Result<(), CapabilityError> {
        Capabilities::oauth_end(self, handle)
    }
}

#[cfg(all(windows, test))]
mod windows_acl_tests {
    // Verifies: NFR-14 on Windows - the credential path's ACL helper leaves
    // the file with a protected, non-inheriting DACL.
    #[test]
    fn set_owner_only_protects_the_file() {
        let dir = lca_testkit::scratch_path("lca-acl");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        let file = dir.join("cred.json");
        std::fs::write(&file, b"{}").expect("write");
        crate::capabilities::windows_acl::set_owner_only(&file).expect("set owner-only");
        assert!(
            crate::capabilities::windows_acl::dacl_is_protected(&file).expect("query DACL"),
            "the DACL must be protected (inheritance off)"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
