//! QUIC and TLS setup (spec 01): self-signed server certificate, ALPN
//! `hytale/3`, and a client certificate that is required but not chained.

use std::{sync::Arc, time::Duration};

use anyhow::Result;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{
    CryptoProvider, WebPkiSupportedAlgorithms, verify_tls12_signature, verify_tls13_signature,
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{DigitallySignedStruct, DistinguishedName, SignatureScheme};

/// ALPNs offered by the server. `hytale/2` is accepted at the TLS level only
/// so the client can be told it is outdated.
pub const SERVER_ALPNS: [&[u8]; 2] = [b"hytale/3", b"hytale/2"];

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

fn self_signed() -> Result<(CertificateDer<'static>, PrivateKeyDer<'static>)> {
    let ck = rcgen::generate_simple_self_signed(vec!["localhost".into()])?;
    let key = PrivatePkcs8KeyDer::from(ck.key_pair.serialize_der());
    Ok((ck.cert.der().clone(), key.into()))
}

fn transport() -> Arc<quinn::TransportConfig> {
    let mut t = quinn::TransportConfig::default();
    t.max_concurrent_bidi_streams(8u32.into());
    // The server opens two unidirectional streams (Chunks, WorldMap) when a
    // player joins the world (spec 01, "Streams").
    t.max_concurrent_uni_streams(4u32.into());
    t.max_idle_timeout(Some(Duration::from_secs(60).try_into().unwrap()));
    t.keep_alive_interval(Some(Duration::from_secs(10)));
    Arc::new(t)
}

/// The server's QUIC config and the DER bytes of its certificate (the
/// authenticated login binds a token to it, spec 03).
pub fn server_config() -> Result<(quinn::ServerConfig, Vec<u8>)> {
    let p = provider();
    let (cert, key) = self_signed()?;
    let cert_der = cert.to_vec();
    let mut tls = rustls::ServerConfig::builder_with_provider(p.clone())
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_client_cert_verifier(Arc::new(AnyCert(p.signature_verification_algorithms)))
        .with_single_cert(vec![cert], key)?;
    tls.alpn_protocols = SERVER_ALPNS.iter().map(|a| a.to_vec()).collect();
    tls.max_early_data_size = 0;
    let mut cfg = quinn::ServerConfig::with_crypto(Arc::new(
        quinn::crypto::rustls::QuicServerConfig::try_from(tls)?,
    ));
    cfg.transport_config(transport());
    cfg.migration(false);
    Ok((cfg, cert_der))
}

/// DER bytes of the certificate the peer presented.
pub fn peer_cert_der(conn: &quinn::Connection) -> Option<Vec<u8>> {
    let certs = conn
        .peer_identity()?
        .downcast::<Vec<rustls::pki_types::CertificateDer<'static>>>()
        .ok()?;
    certs.first().map(|c| c.to_vec())
}

/// Client side, for the proxy: presents its own certificate and accepts any server.
pub fn client_config() -> Result<quinn::ClientConfig> {
    let p = provider();
    let (cert, key) = self_signed()?;
    let mut tls = rustls::ClientConfig::builder_with_provider(p.clone())
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AnyCert(p.signature_verification_algorithms)))
        .with_client_auth_cert(vec![cert], key)?;
    tls.alpn_protocols = vec![b"hytale/3".to_vec()];
    let mut cfg = quinn::ClientConfig::new(Arc::new(
        quinn::crypto::rustls::QuicClientConfig::try_from(tls)?,
    ));
    cfg.transport_config(transport());
    Ok(cfg)
}

/// The ALPN a connection settled on.
pub fn negotiated_alpn(conn: &quinn::Connection) -> Option<Vec<u8>> {
    conn.handshake_data()?
        .downcast::<quinn::crypto::rustls::HandshakeData>()
        .ok()?
        .protocol
}

/// Accepts any certificate but still checks the handshake signature, so the
/// peer has to hold the key of the certificate it shows.
#[derive(Debug)]
struct AnyCert(WebPkiSupportedAlgorithms);

impl ClientCertVerifier for AnyCert {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }
    fn verify_client_cert(
        &self,
        _: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: UnixTime,
    ) -> Result<ClientCertVerified, rustls::Error> {
        Ok(ClientCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        m: &[u8],
        c: &CertificateDer<'_>,
        d: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(m, c, d, &self.0)
    }
    fn verify_tls13_signature(
        &self,
        m: &[u8],
        c: &CertificateDer<'_>,
        d: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(m, c, d, &self.0)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.supported_schemes()
    }
}

impl ServerCertVerifier for AnyCert {
    fn verify_server_cert(
        &self,
        _: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        m: &[u8],
        c: &CertificateDer<'_>,
        d: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(m, c, d, &self.0)
    }
    fn verify_tls13_signature(
        &self,
        m: &[u8],
        c: &CertificateDer<'_>,
        d: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(m, c, d, &self.0)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.supported_schemes()
    }
}
