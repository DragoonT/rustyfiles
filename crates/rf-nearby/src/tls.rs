//! TLS for QUIC: TLS 1.3 only, both sides show their self-signed certificate (mutual TLS), and the handshake still proves that each
//! side holds the key of the certificate it shows (the signature checks below are real). What a certificate is worth is decided above
//! this layer: by its fingerprint, against the trusted devices or the user's yes to an offer or a pairing code.

use std::sync::Arc;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, WebPkiSupportedAlgorithms};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{DigitallySignedStruct, DistinguishedName, SignatureScheme};

use crate::identity::Identity;

/// The ALPN of the protocol: a peer that does not speak it is turned away in the handshake.
pub const ALPN: &[u8] = b"rustyfiles-nearby/1";

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

#[derive(Debug)]
struct AnyCert(WebPkiSupportedAlgorithms);

impl AnyCert {
    fn new() -> AnyCert {
        AnyCert(provider().signature_verification_algorithms)
    }
}

impl ServerCertVerifier for AnyCert {
    fn verify_server_cert(&self, _: &CertificateDer<'_>, _: &[CertificateDer<'_>], _: &ServerName<'_>, _: &[u8], _: UnixTime) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(&self, _: &[u8], _: &CertificateDer<'_>, _: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        Err(rustls::Error::General("TLS 1.2 is not used".into()))
    }
    fn verify_tls13_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.0)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.supported_schemes()
    }
}

impl ClientCertVerifier for AnyCert {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }
    fn verify_client_cert(&self, _: &CertificateDer<'_>, _: &[CertificateDer<'_>], _: UnixTime) -> Result<ClientCertVerified, rustls::Error> {
        Ok(ClientCertVerified::assertion())
    }
    fn verify_tls12_signature(&self, _: &[u8], _: &CertificateDer<'_>, _: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        Err(rustls::Error::General("TLS 1.2 is not used".into()))
    }
    fn verify_tls13_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.0)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.supported_schemes()
    }
    fn client_auth_mandatory(&self) -> bool {
        true
    }
}

/// The QUIC transport: windows large enough that one connection fills a gigabit link (the defaults are made for the internet).
fn transport() -> Arc<quinn::TransportConfig> {
    let mut t = quinn::TransportConfig::default();
    t.stream_receive_window(quinn::VarInt::from_u32(16 << 20));
    t.receive_window(quinn::VarInt::from_u32(64 << 20));
    t.send_window(64 << 20);
    t.max_concurrent_uni_streams(quinn::VarInt::from_u32(64));
    t.keep_alive_interval(Some(std::time::Duration::from_secs(5)));
    t.max_idle_timeout(Some(quinn::VarInt::from_u32(30_000).into()));
    Arc::new(t)
}

pub fn server_config(me: &Identity) -> anyhow::Result<quinn::ServerConfig> {
    let mut tls = rustls::ServerConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_client_cert_verifier(Arc::new(AnyCert::new()))
        .with_single_cert(vec![me.cert.clone()], me.key())?;
    tls.alpn_protocols = vec![ALPN.to_vec()];
    let mut cfg = quinn::ServerConfig::with_crypto(Arc::new(quinn::crypto::rustls::QuicServerConfig::try_from(tls)?));
    cfg.transport_config(transport());
    Ok(cfg)
}

pub fn client_config(me: &Identity) -> anyhow::Result<quinn::ClientConfig> {
    let mut tls = rustls::ClientConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AnyCert::new()))
        .with_client_auth_cert(vec![me.cert.clone()], me.key())?;
    tls.alpn_protocols = vec![ALPN.to_vec()];
    let mut cfg = quinn::ClientConfig::new(Arc::new(quinn::crypto::rustls::QuicClientConfig::try_from(tls)?));
    cfg.transport_config(transport());
    Ok(cfg)
}

/// The fingerprint of the certificate the other side of `conn` proved in the handshake.
pub fn peer_fingerprint(conn: &quinn::Connection) -> Option<String> {
    let any = conn.peer_identity()?;
    let certs = any.downcast::<Vec<CertificateDer<'static>>>().ok()?;
    certs.first().map(crate::identity::fingerprint)
}

/// The six-digit code both sides of a pairing show: drawn from the TLS session itself (RFC 5705 exporter), so a device in the middle,
/// which has two different sessions, can't make the two screens agree.
pub fn pairing_code(conn: &quinn::Connection) -> Option<String> {
    let mut out = [0u8; 8];
    conn.export_keying_material(&mut out, b"rustyfiles nearby pairing", b"").ok()?;
    Some(format!("{:06}", u64::from_be_bytes(out) % 1_000_000))
}
