//! The HTTPS trust-anchor hook (gh #94): trusting one extra CA
//! beside the webpki roots, and the shared connector stack both the
//! engine's own client and anchor rebuilds build on. Split from `mod.rs`
//! for the line ceiling; behavior lives here, the engine stays the
//! single owner.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::client::legacy::connect::dns::GaiResolver;

use super::{Capabilities, PinnedResolver, proxy};
use lca_protocol::CapabilityError;

/// The pinned-DNS HTTP connector under the proxy layer (gh #145): one
/// builder for the engine's own client and for trust-anchor rebuilds,
/// so the two can never disagree about pins, `enforce_http`, or proxy
/// discipline.
pub(super) fn proxied_stack(
    pins: &Arc<Mutex<HashMap<String, Vec<SocketAddr>>>>,
) -> proxy::ProxyTcp {
    let mut http = HttpConnector::new_with_resolver(PinnedResolver {
        inner: GaiResolver::new(),
        pins: pins.clone(),
    });
    // The wrapped connector must accept `https`: hyper-rustls's
    // `build()` clears this on the connector it creates, but
    // `wrap_connector` leaves a caller-supplied one as-is, and the
    // default `enforce_http` rejects the scheme before TLS is even
    // considered.
    http.enforce_http(false);
    // gh #145: the proxy layer sits under rustls, over the pinned
    // connector, so direct paths keep their exact behavior and
    // proxied ones inherit the pin/DNS discipline above.
    proxy::ProxyTcp::new(http, proxy::proxies_from_env())
}

impl Capabilities {
    /// Trust one more CA for this engine's HTTPS client (gh #94): the
    /// fixture anchor the offline TLS rows pin, beside the webpki roots
    /// every other request keeps. Test and advanced use only — it
    /// rebuilds the client, so in-flight requests keep the old roots —
    /// and unreachable from the WASM guest (no WIT import names it),
    /// so a curriculum extension cannot widen its own trust.
    pub fn add_trust_anchor(&mut self, ca_pem: &[u8]) -> Result<(), CapabilityError> {
        use rustls::pki_types::pem::PemObject;
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let mut anchored = 0usize;
        for cert in rustls::pki_types::CertificateDer::pem_slice_iter(ca_pem) {
            let cert =
                cert.map_err(|err| CapabilityError::Invalid(format!("bad CA pem: {err}")))?;
            roots
                .add(cert)
                .map_err(|err| CapabilityError::Invalid(format!("bad CA cert: {err}")))?;
            anchored += 1;
        }
        if anchored == 0 {
            return Err(CapabilityError::Invalid(
                "the CA pem held no certificates".to_string(),
            ));
        }
        let config = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        let https = hyper_rustls::HttpsConnectorBuilder::new()
            .with_tls_config(config)
            .https_or_http()
            .enable_http1()
            .wrap_connector(proxied_stack(&self.pins));
        self.client = Client::builder(hyper_util::rt::TokioExecutor::new()).build(https);
        Ok(())
    }
}
