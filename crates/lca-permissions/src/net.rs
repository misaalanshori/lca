//! `net` and `net-local` pattern validation and the canonical local
//! ranges (capability catalog, ADR-0011). The catalog is normative for
//! the list; this module is its single implementation.

use std::net::IpAddr;
use std::str::FromStr;

use ipnet::IpNet;

/// Why a manifest pattern was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PatternError {
    /// A bare `*` covers every host; a consent screen cannot evaluate it
    /// (FR-PERM-15).
    #[error("a bare `*` is rejected: the consent screen must name something evaluable")]
    BareWildcard,
    /// A wildcard already grants broadly; a port pin on top of it is
    /// rejected at manifest validation (capability catalog).
    #[error("a wildcard pattern may not pin a port")]
    WildcardWithPort,
    /// A `net-local` range outside the canonical local blocks (FR-PERM-14).
    #[error("`{value}` is outside the local ranges: {value} cannot be a net-local grant")]
    OutOfRange {
        /// The offending pattern text.
        value: String,
    },
    /// Unparseable pattern text.
    #[error("invalid pattern `{value}`: {reason}")]
    Invalid {
        /// The offending pattern text.
        value: String,
        /// Why it failed.
        reason: String,
    },
}

/// One `net` host pattern: exact hostname or `*.` wildcard, optionally
/// pinning a non-default port (portless grants HTTPS/443 only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetPattern {
    host: String,
    port: Option<u16>,
    wildcard: bool,
}

impl NetPattern {
    /// Does this pattern cover the hostname, before port rules? A match
    /// here puts the request under `net`'s rules even when the port then
    /// fails them (FR-PERM-5 needs the distinction).
    pub fn matches_host(&self, host: &str) -> bool {
        let host = host.to_ascii_lowercase().trim_end_matches('.').to_string();
        if self.wildcard {
            let suffix = &self.host; // stored with the leading dot: ".example.com"
            host.ends_with(suffix) && host.len() > suffix.len()
        } else {
            host == self.host
        }
    }

    /// Does this pattern grant `host:port`? Case-insensitive, trailing
    /// dot tolerated.
    pub fn matches(&self, host: &str, port: u16) -> bool {
        if !self.matches_host(host) {
            return false;
        }
        match self.port {
            Some(pinned) => port == pinned,
            None => port == 443,
        }
    }

    /// The granted port, when pinned.
    pub fn pinned_port(&self) -> Option<u16> {
        self.port
    }
}

/// Parse a `net` manifest entry.
pub fn parse_net_pattern(value: &str) -> Result<NetPattern, PatternError> {
    let invalid = |reason: &str| PatternError::Invalid {
        value: value.to_string(),
        reason: reason.to_string(),
    };
    if value == "*" {
        return Err(PatternError::BareWildcard);
    }
    let (host_part, port) = match value.rsplit_once(':') {
        Some((host, port)) => {
            if host.contains('*') {
                return Err(PatternError::WildcardWithPort);
            }
            let port: u16 = port.parse().map_err(|_| invalid("port is not a number"))?;
            (host, Some(port))
        }
        None => (value, None),
    };
    let wildcard = host_part.starts_with("*.");
    let host = host_part.trim_start_matches('*').to_ascii_lowercase();
    if host.is_empty() || host == "*" {
        return Err(invalid("not a hostname"));
    }
    if wildcard {
        let rest = host.trim_start_matches('.');
        if rest.starts_with('.') || rest.is_empty() {
            return Err(invalid("wildcard must take the form *.example.com"));
        }
    }
    Ok(NetPattern {
        host: host.to_string(),
        port,
        wildcard,
    })
}

/// One `net-local` entry: a CIDR/literal inside the canonical ranges,
/// `localhost`, or an mDNS `.local` name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalPattern {
    /// A CIDR range (or single address) within the local blocks.
    Cidr(IpNet),
    /// The `localhost` name (or loopback addresses).
    Localhost,
    /// An mDNS-style `.local` hostname.
    Mdns,
}

impl LocalPattern {
    /// Match a resolved IP address.
    pub fn matches_ip(&self, ip: IpAddr) -> bool {
        match self {
            LocalPattern::Cidr(net) => net.contains(&normalize_ip(ip)),
            LocalPattern::Localhost => is_loopback(ip),
            LocalPattern::Mdns => false,
        }
    }

    /// Match a hostname (no DNS here: names resolve at request time and
    /// the result is checked as an address too).
    pub fn matches_name(&self, name: &str) -> bool {
        let name = name.to_ascii_lowercase();
        match self {
            LocalPattern::Localhost => name == "localhost",
            LocalPattern::Mdns => name.ends_with(".local"),
            LocalPattern::Cidr(_) => false,
        }
    }
}

/// Parse a `net-local` manifest entry, refusing anything outside the
/// canonical local ranges (FR-PERM-14).
pub fn parse_local_pattern(value: &str) -> Result<LocalPattern, PatternError> {
    let invalid = |reason: &str| PatternError::Invalid {
        value: value.to_string(),
        reason: reason.to_string(),
    };
    if value == "localhost" {
        return Ok(LocalPattern::Localhost);
    }
    if value == "*.local" {
        return Ok(LocalPattern::Mdns);
    }
    let net = match IpAddr::from_str(value) {
        Ok(IpAddr::V4(v4)) => IpNet::from(ipnet::Ipv4Net::new(v4, 32).expect("prefix 32 is valid")),
        Ok(IpAddr::V6(v6)) => {
            IpNet::from(ipnet::Ipv6Net::new(v6, 128).expect("prefix 128 is valid"))
        }
        Err(_) => IpNet::from_str(value).map_err(|err| invalid(&err.to_string()))?,
    };

    let network = net.network();
    let last = last_address(&net);
    if is_local_address(network) && last.is_some_and(is_local_address) {
        return Ok(LocalPattern::Cidr(net));
    }
    Err(PatternError::OutOfRange {
        value: value.to_string(),
    })
}

fn last_address(net: &IpNet) -> Option<IpAddr> {
    match net {
        IpNet::V4(v4) => Some(IpAddr::V4(v4.broadcast())),
        IpNet::V6(v6) => {
            let network = u128::from(v6.network());
            // Host bits are the low (128 - prefix) bits: the last address
            // inside the block.
            let max = network | (u128::MAX >> (128 - u32::from(v6.prefix_len())));
            let bytes = max.to_be_bytes();
            Some(IpAddr::V6(std::net::Ipv6Addr::from(bytes)))
        }
    }
}

/// Unwrap an IPv4-mapped IPv6 address before any range check
/// (FR-PERM-17).
pub fn normalize_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => ip,
        },
        _ => ip,
    }
}

fn is_loopback(ip: IpAddr) -> bool {
    match normalize_ip(ip) {
        IpAddr::V4(v4) => v4.octets()[0] == 127,
        IpAddr::V6(v6) => v6.is_loopback(),
    }
}

/// The canonical local ranges, the one normative list for `net-local`
/// grants and `net`'s rebinding check (capability catalog):
/// IPv4 loopback `127.0.0.0/8`, private `10/8` `172.16/12` `192.168/16`,
/// link-local `169.254/16`, carrier-grade NAT `100.64/10`; IPv6 loopback
/// `::1`, link-local `fe80::/10`, unique-local `fc00::/7`.
/// IPv4-mapped IPv6 addresses are normalized first (FR-PERM-17).
pub fn is_local_address(ip: IpAddr) -> bool {
    match normalize_ip(ip) {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            o[0] == 127
                || o[0] == 10
                || (o[0] == 172 && (16..=31).contains(&o[1]))
                || (o[0] == 192 && o[1] == 168)
                || (o[0] == 169 && o[1] == 254)
                || (o[0] == 100 && (64..=127).contains(&o[1]))
        }
        IpAddr::V6(v6) => {
            let b = v6.octets();
            v6.is_loopback()
                || (b[0] & 0xfe) == 0xfc // fc00::/7 unique local
                || (b[0] == 0xfe && (b[1] & 0xc0) == 0x80) // fe80::/10 link-local
        }
    }
}
