//! The HTTP client for model downloads and online translators.

/// A blocking client builder that trusts the system's root certificates and also the
/// Mozilla root certificates inside the binary. With the second set, downloads also
/// work on systems without a certificate store, for example a minimal container.
pub fn client_builder() -> reqwest::blocking::ClientBuilder {
    let roots = webpki_root_certs::TLS_SERVER_ROOT_CERTS
        .iter()
        .filter_map(|der| reqwest::Certificate::from_der(der).ok());
    reqwest::blocking::Client::builder().tls_certs_merge(roots)
}
