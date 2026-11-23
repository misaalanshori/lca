//! The release channel a build belongs to, and the one tag it therefore
//! resolves a *default* reference to (gh #615, ADR-0043).
//!
//! Two channels exist, and they publish different tags for the same extension:
//! the stable line ships `abi-<line>` (`.github/workflows/publish-extensions.yml`),
//! the unstable line ships `:unstable` and `:b<sha7>`
//! (`.github/workflows/unstable.yml`). Before #615 a bare name always expanded to
//! the line tag, so an unstable binary asked for a tag that channel never
//! published - and the failure named a token endpoint instead of the mistake.
//!
//! **The fact that decides**: the product version carries `.b<sha7>` iff the
//! unstable workflow built this binary - it bakes
//! `LCA_BUILD_VERSION=<workspace version>.b<sha7>` (`.github/workflows/unstable.yml:163`),
//! `crates/lca-cli/build.rs:12` turns that into `PRODUCT_VERSION`, and this crate
//! reads the same variable through its own `build.rs` (identical fallback, so a
//! stable build is unchanged). Nothing else - no setting, no file, no network
//! probe - may flip the channel: a binary's channel is a property of how it was
//! built, which is why the version is the signal.

use super::{OFFICIAL_HOST, OFFICIAL_ORG, expand_shorthand, split_oci_reference};

/// Which release line this binary came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    /// A tagged release: extensions come from their `abi-<line>` tags.
    Release,
    /// The rolling unstable line: extensions come from `:unstable`.
    Unstable,
}

impl Channel {
    /// The tag this channel publishes a *default* reference under.
    ///
    /// Both channels call it by name here, so a third channel adds one arm
    /// rather than a second rule.
    #[must_use]
    pub fn tag(self) -> String {
        match self {
            Channel::Release => format!("abi-{}", lca_ext_abi::ABI_VERSION),
            Channel::Unstable => "unstable".to_string(),
        }
    }

    /// Classify a product version: `X.Y.Z.b<sha7>` is an unstable build, and
    /// anything else is a release (`X.Y.Z`).
    ///
    /// The `.b<sha>` suffix is ADR-0043's scheme, and it is the *only* marker:
    /// a prerelease-looking version without it (`0.7.0-rc1`) is treated as a
    /// release line, because no workflow produces one - if one ever does, it
    /// earns a marker here rather than a guess at the shape.
    #[must_use]
    pub fn from_product_version(product: &str) -> Channel {
        match product.split_once(".b") {
            Some((_base, sha)) if !sha.is_empty() => Channel::Unstable,
            _ => Channel::Release,
        }
    }

    /// This binary's channel, from the version its build baked in.
    #[must_use]
    pub fn current() -> Channel {
        Channel::from_product_version(env!("LCA_PRODUCT_VERSION"))
    }
}

/// Expand a **user-supplied** source for `channel` (gh #252's shorthand).
///
/// A bare name gains the channel's tag; a source that names a tag - any tag,
/// including one this channel does not publish - is returned as written. That
/// is the "explicit is honoured" half of the rule: if someone asks for
/// `name:abi-0.6` they get a fetch of `abi-0.6` and, if it is absent, the copy
/// that says so.
#[must_use]
pub fn expand_shorthand_with(source: &str, channel: Channel) -> String {
    // The release form is the authored one; the bare-name tag is the only
    // thing the channel decides.
    let release_form = expand_shorthand(source);
    if channel == Channel::Release {
        return release_form;
    }
    // A tag the user wrote is theirs; only a default tag moves.
    if source.contains('/') || source.contains('\\') || source.contains('@') {
        return release_form;
    }
    let tag = source
        .split_once(':')
        .map(|(_name, tag)| tag)
        .unwrap_or_default();
    if !tag.is_empty() {
        return release_form;
    }
    // `expand_shorthand` appended the line tag to a bare name; swap exactly
    // that suffix, and only for our own namespace.
    swap_default_tag(&release_form, channel)
}

/// The reference a **catalog** entry's authored `oci_ref` resolves to on
/// `channel`.
///
/// Catalogs are authored in their release form (`:abi-<line>`, see
/// `extensions/catalog.toml`), so that tag is the catalog's *default*, not a
/// user's choice: on the unstable channel it names `:unstable`. Every other tag
/// (a version, a `:b<sha7>` pin, another line) is returned as written, and so is
/// any reference outside the first-party namespace: we know our own channel's
/// tags and nobody else's.
#[must_use]
pub fn catalog_ref_for_channel(oci_ref: &str, channel: Channel) -> String {
    if channel == Channel::Release {
        return oci_ref.to_string();
    }
    swap_default_tag(oci_ref, channel)
}

/// Replace a first-party `:abi-<current line>` tag with `channel`'s tag.
/// Anything that is not exactly that shape comes back untouched.
fn swap_default_tag(reference: &str, channel: Channel) -> String {
    let line_tag = Channel::Release.tag();
    let Ok((host, name, tag)) = split_oci_reference(reference) else {
        return reference.to_string();
    };
    let own_namespace = host == OFFICIAL_HOST
        && (name == OFFICIAL_ORG || name.starts_with(&format!("{OFFICIAL_ORG}/")));
    if !own_namespace || tag != line_tag {
        return reference.to_string();
    }
    format!("{host}/{name}:{}", channel.tag())
}

#[cfg(test)]
mod channel_tests {
    use super::*;

    /// Verifies: ADR-0043's marker is the whole signal - the unstable
    /// workflow's `X.Y.Z.b<sha7>` is unstable, a release's `X.Y.Z` is not.
    #[test]
    fn the_product_version_carries_the_channel() {
        assert_eq!(
            Channel::from_product_version("0.6.0.b9653a3"),
            Channel::Unstable
        );
        assert_eq!(Channel::from_product_version("0.7.0"), Channel::Release);
        // A suffix with no sha is not the workflow's shape, and a bare
        // "b" is not a marker.
        assert_eq!(Channel::from_product_version("0.7.0.b"), Channel::Release);
        assert_eq!(Channel::from_product_version("0.7.0-rc1"), Channel::Release);
        assert_eq!(Channel::from_product_version(""), Channel::Release);
    }

    /// Verifies: the two channels name their own default tag, and the release
    /// one tracks the line (so a line move needs no edit here).
    #[test]
    fn each_channel_names_its_own_default_tag() {
        assert_eq!(
            Channel::Release.tag(),
            format!("abi-{}", lca_ext_abi::ABI_VERSION)
        );
        assert_eq!(Channel::Unstable.tag(), "unstable");
    }
}
