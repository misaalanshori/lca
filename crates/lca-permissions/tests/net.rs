//! `net` and `net-local` pattern validation (capability catalog,
//! ADR-0011): the rules a manifest must satisfy at install time and the
//! canonical local ranges the runtime checks against.

use lca_permissions::{
    LocalPattern, NetPattern, PatternError, is_local_address, normalize_ip, parse_local_pattern,
    parse_net_pattern,
};

// Verifies: FR-PERM-15 (a bare wildcard covering every host under `net`
// is rejected; so is a wildcard with a port pin).
#[test]
fn bare_wildcards_and_wildcard_ports_are_rejected() {
    assert!(matches!(
        parse_net_pattern("*"),
        Err(PatternError::BareWildcard)
    ));
    assert!(matches!(
        parse_net_pattern("*.example.com:8443"),
        Err(PatternError::WildcardWithPort)
    ));
}

#[test]
fn net_patterns_parse_and_match_with_port_rules() {
    let exact: NetPattern = parse_net_pattern("api.example.com").expect("exact");
    assert!(
        exact.matches("api.example.com", 443),
        "portless grants443 only"
    );
    assert!(!exact.matches("api.example.com", 8443));
    assert!(
        exact.matches("API.EXAMPLE.COM", 443),
        "matching is case-insensitive"
    );
    assert!(!exact.matches("other.example.com", 443), "exact is exact");

    let pinned: NetPattern = parse_net_pattern("build.example.com:8443").expect("pinned");
    assert!(pinned.matches("build.example.com", 8443));
    assert!(
        !pinned.matches("build.example.com", 443),
        "a pin replaces the default"
    );

    let wildcard: NetPattern = parse_net_pattern("*.example-cdn.com").expect("wildcard");
    assert!(wildcard.matches("assets.example-cdn.com", 443));
    // One or more labels in that position: a.b matches, the apex does not.
    assert!(wildcard.matches("a.b.example-cdn.com", 443));
    assert!(!wildcard.matches("example-cdn.com", 443));
    assert!(!wildcard.matches("evil-example-cdn.com", 443));
}

// Verifies: FR-PERM-14 (a net-local address outside the canonical local
// ranges is rejected at manifest validation; net-local cannot smuggle
// general internet access).
#[test]
fn local_patterns_refuse_non_local_ranges() {
    assert!(parse_local_pattern("192.168.0.0/16").is_ok());
    assert!(
        parse_local_pattern("100.64.0.0/10").is_ok(),
        "the tailnet case"
    );
    assert!(parse_local_pattern("127.0.0.1").is_ok());
    assert!(parse_local_pattern("localhost").is_ok());
    assert!(parse_local_pattern("*.local").is_ok());
    assert!(parse_local_pattern("fc00::/7").is_ok());

    for outside in ["8.8.8.0/24", "8.8.8.8", "0.0.0.0/0", "2001:4860::/32"] {
        assert!(
            matches!(
                parse_local_pattern(outside),
                Err(PatternError::OutOfRange { .. })
            ),
            "{outside} is not a local range"
        );
    }
    assert!(matches!(
        parse_local_pattern("example.com"),
        Err(PatternError::Invalid { .. })
    ));
}

#[test]
fn local_patterns_match_by_address_or_name() {
    let range: LocalPattern = parse_local_pattern("192.168.0.0/16").expect("cidr");
    assert!(range.matches_ip("192.168.4.7".parse().expect("ip")));
    assert!(!range.matches_ip("192.169.0.1".parse().expect("ip")));

    let localhost: LocalPattern = parse_local_pattern("localhost").expect("localhost");
    assert!(localhost.matches_name("localhost"));
    assert!(localhost.matches_ip("127.0.0.1".parse().expect("ip")));
    assert!(!localhost.matches_name("example.com"));

    let mdns: LocalPattern = parse_local_pattern("*.local").expect("mdns");
    assert!(mdns.matches_name("printer.local"));
    assert!(!mdns.matches_name("printer.example.com"));
}

// Verifies: FR-PERM-13's range set and FR-PERM-17 (IPv4-mapped IPv6 is
// normalized before any range check).
#[test]
fn the_canonical_local_ranges_cover_exactly_the_local_space() {
    for local in [
        "127.0.0.1",
        "10.1.2.3",
        "172.16.9.9",
        "192.168.1.1",
        "169.254.0.1",
        "100.64.0.1",
        "::1",
        "fe80::1",
        "fd12:3456::1",
    ] {
        assert!(
            is_local_address(local.parse().expect("ip")),
            "{local} must be local"
        );
    }
    for public in [
        "8.8.8.8",
        "1.1.1.1",
        "172.15.0.1",
        "172.32.0.1",
        "100.63.0.1",
        "::2",
        "2606::1",
    ] {
        assert!(
            !is_local_address(public.parse().expect("ip")),
            "{public} must NOT be local"
        );
    }

    // FR-PERM-17: a mapped address is unwrapped before checking, so
    // ::ffff:10.0.0.1 reads as local and ::ffff:8.8.8.8 does not.
    let mapped_local: std::net::IpAddr = "::ffff:10.0.0.1".parse().expect("ip");
    assert!(is_local_address(normalize_ip(mapped_local)));
    let mapped_public: std::net::IpAddr = "::ffff:8.8.8.8".parse().expect("ip");
    assert!(!is_local_address(normalize_ip(mapped_public)));
    assert_eq!(
        normalize_ip(mapped_local),
        "10.0.0.1".parse::<std::net::IpAddr>().expect("ip")
    );
}
