//! This device's identity: a key pair and a self-signed certificate made once and kept in the state folder. The device id is derived
//! from the certificate, so a peer's id can always be checked against the certificate it shows in the TLS handshake: an id can't be
//! claimed, only proven.

use std::path::{Path, PathBuf};

use anyhow::Context as _;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

const CERT_FILE: &str = "cert.der";
const KEY_FILE: &str = "key.der";

pub struct Identity {
    pub cert: CertificateDer<'static>,
    key: Vec<u8>,
    /// SHA-256 of the certificate, lower-case hex (64 characters).
    pub fingerprint: String,
    /// The short form other devices know this one by: the first 20 hex characters of the fingerprint.
    pub id: String,
}

impl Identity {
    /// The identity kept in `dir`, or a new one written there (the folder is made when missing).
    pub fn load_or_create(dir: &Path) -> anyhow::Result<Identity> {
        let (cert_path, key_path) = (dir.join(CERT_FILE), dir.join(KEY_FILE));
        if let (Ok(cert), Ok(key)) = (std::fs::read(&cert_path), std::fs::read(&key_path)) {
            return Ok(Identity::from_parts(CertificateDer::from(cert), key));
        }
        std::fs::create_dir_all(dir).with_context(|| format!("cannot make {}", dir.display()))?;
        let key = rcgen::KeyPair::generate().context("cannot make a key")?;
        let params = rcgen::CertificateParams::new(vec!["rustyfiles".to_string()]).context("certificate parameters")?;
        let cert = params.self_signed(&key).context("cannot sign the certificate")?;
        let (cert, key) = (cert.der().clone(), key.serialize_der());
        write_private(&key_path, &key)?;
        std::fs::write(&cert_path, cert.as_ref()).with_context(|| format!("cannot write {}", cert_path.display()))?;
        Ok(Identity::from_parts(cert, key))
    }

    fn from_parts(cert: CertificateDer<'static>, key: Vec<u8>) -> Identity {
        let fingerprint = fingerprint(&cert);
        let id = id_of(&fingerprint);
        Identity { cert, key, fingerprint, id }
    }

    pub fn key(&self) -> PrivateKeyDer<'static> {
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(self.key.clone()))
    }
}

/// SHA-256 of a certificate, lower-case hex.
pub fn fingerprint(cert: &CertificateDer<'_>) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, cert.as_ref());
    digest.as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

/// The device id for a fingerprint.
pub fn id_of(fingerprint: &str) -> String {
    fingerprint.chars().take(20).collect()
}

/// The private key: written so that only this user can read it where the platform lets us say so.
fn write_private(path: &PathBuf, bytes: &[u8]) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write as _;
        use std::os::unix::fs::OpenOptionsExt as _;
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path)?;
        f.write_all(bytes)?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        // (on Windows the state folder is in the user's own profile, which other users can't read)
        std::fs::write(path, bytes).with_context(|| format!("cannot write {}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn an_identity_is_made_once_and_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let a = super::Identity::load_or_create(dir.path()).unwrap();
        let b = super::Identity::load_or_create(dir.path()).unwrap();
        assert_eq!(a.fingerprint, b.fingerprint);
        assert_eq!(a.id.len(), 20);
        assert_eq!(super::fingerprint(&a.cert), a.fingerprint);
    }
}
