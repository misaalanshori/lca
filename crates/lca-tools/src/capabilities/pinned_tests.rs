//! Split from `capabilities.rs` (cycle 7, P3).

use super::{GaiResolver, Name, PinnedResolver, Service};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};

// Verifies: FR-PERM-13's pinning - a checked host resolves to exactly the
// addresses that were checked, so hyper cannot re-resolve to a local
// address between the rebinding check and the connect.
#[tokio::test]
async fn a_pinned_host_resolves_to_the_checked_address() {
    let mut pins: HashMap<String, Vec<SocketAddr>> = HashMap::new();
    pins.insert(
        "example.com".to_string(),
        vec![SocketAddr::new(
            IpAddr::V4(Ipv4Addr::new(203, 0, 113, 7)),
            0,
        )],
    );
    let mut resolver = PinnedResolver {
        inner: GaiResolver::new(),
        pins: Arc::new(Mutex::new(pins)),
    };
    let addrs: Vec<SocketAddr> = resolver
        .call("Example.COM".parse::<Name>().expect("name"))
        .await
        .expect("resolves")
        .collect();
    assert_eq!(addrs.len(), 1);
    assert_eq!(addrs[0].ip().to_string(), "203.0.113.7");
}
