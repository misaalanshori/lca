//! The `lca:ext` contract crate: the normative WIT package under `wit/`
//! plus generated bindings.
//!
//! Extension authors depend on this crate for the guest bindings without
//! pulling in the agent (ADR-0002). The binary enables the `host` feature
//! to get Wasmtime-side bindings for every world.

#![forbid(unsafe_code)]

/// The ABI version this build implements (`major.minor`, per the manifest's
/// `abi` field and the support window in docs/abi-versioning.md).
pub const ABI_VERSION: &str = "0.1";

/// The WIT package name that crosses every manifest and registry tag.
pub const PACKAGE: &str = "lca:ext";

#[cfg(feature = "host")]
#[allow(missing_docs)] // generated bindings: the WIT files carry the docs
pub mod host {
    //! Host-side bindings for the worlds written so far. Phase 3, 4, and6
    //! add their worlds to this module as they land.

    /// The `tool` world.
    pub mod tool {
        wasmtime::component::bindgen!({ path: "../../wit", world: "tool" });
    }

    /// The `command` world.
    pub mod command {
        wasmtime::component::bindgen!({ path: "../../wit", world: "command" });
    }

    /// The `hooks` world.
    pub mod hooks {
        wasmtime::component::bindgen!({ path: "../../wit", world: "hooks" });
    }
}

#[cfg(test)]
mod tests {
    /// The constants every manifest check compares against.
    #[test]
    fn abi_version_matches_the_manifest_line() {
        assert_eq!(super::ABI_VERSION, "0.1");
        assert_eq!(super::PACKAGE, "lca:ext");
    }
}
