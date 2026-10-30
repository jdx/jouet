//! TLS for jouet's HTTP requests.

use std::sync::Arc;
use ureq::tls::{Certificate, RootCerts, TlsConfig};

/// TLS that trusts the system's root certificates (`SSL_CERT_FILE` and
/// `SSL_CERT_DIR` are honoured), as gh and curl do, so a proxy's CA
/// installed on the machine works too.
pub fn config() -> TlsConfig {
    let roots = rustls_native_certs::load_native_certs()
        .certs
        .iter()
        .map(|der| Certificate::from_der(der).to_owned())
        .collect();
    TlsConfig::builder()
        .root_certs(RootCerts::Specific(Arc::new(roots)))
        .unversioned_rustls_crypto_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .build()
}
