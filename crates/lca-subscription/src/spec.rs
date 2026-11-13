//! The data half of the kit (gh #567): the spec tables, the account strategy,
//! and the two credential-key readers every flow shares.

use lca_protocol::ProviderCap;

pub(crate) fn stored(cap: &dyn ProviderCap, key: &str) -> String {
    cap.credentials_get(key).unwrap_or_default()
}

/// Read an endpoint from the extension's own namespace first, so tests
/// (or a gateway) redirect every call; otherwise the compiled default.
pub(crate) fn endpoint(cap: &dyn ProviderCap, key: &str, default: &str) -> String {
    let value = stored(cap, key);
    if value.is_empty() {
        default.to_string()
    } else {
        value
    }
}

/// Where the subscription account id comes from after the exchange.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountStrategy {
    /// Decode it from the access token's JWT payload
    /// (`claim` object, then `field`).
    JwtClaim {
        /// The namespaced claim object.
        claim: &'static str,
        /// The account field inside it.
        field: &'static str,
    },
    /// GET the userinfo endpoint with the token and read `field`
    /// (usually `sub`).
    Userinfo {
        /// The JSON field carrying the account id.
        field: &'static str,
    },
}

/// Everything about one subscription gateway that is data: endpoints,
/// client, scope, account lookup. The flows below are identical for
/// every provider carrying one of these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OAuthSpec {
    /// The extension name, for messages.
    pub name: &'static str,
    /// The authorize endpoint default (`auth_endpoint` overrides).
    pub auth_endpoint: &'static str,
    /// The token endpoint default (`token_endpoint` overrides).
    pub token_endpoint: &'static str,
    /// The revoke endpoint default, when the server has one.
    pub revoke_endpoint: Option<&'static str>,
    /// The userinfo endpoint default (needed for [`AccountStrategy::Userinfo`]).
    pub userinfo_endpoint: Option<&'static str>,
    /// The gateway's inference base default (`api_base` overrides).
    pub api_base: &'static str,
    /// The public OAuth client id (embedded like pi embeds its own).
    pub client_id: &'static str,
    /// The authorize scope.
    pub scope: &'static str,
    /// Extra authorize params (`originator`, `referrer`, ...).
    pub extra_auth_params: &'static [(&'static str, &'static str)],
    /// How the account id is learned after the exchange.
    pub account: AccountStrategy,
}
