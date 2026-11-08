//! TLS identities that *are* the device identity.
//!
//! The certificate is self-signed and its subject public key is the device's
//! Ed25519 key. That is the whole point: pinning `SHA-256(public key)` from
//! discovery and then comparing it to the certificate the peer presents is the
//! same check. There is no second key, no separately-trusted CA, and nothing to
//! get out of sync when the identity rotates.
//!
//! TLS 1.3 only. Ed25519 signature schemes are not defined for TLS 1.2, and
//! supporting both would mean a second key for no benefit on a LAN.

use ed25519_dalek::pkcs8::EncodePrivateKey;
use pkcs8::SecretDocument;
use rcgen::{CertificateParams, DnType, IsCa, KeyPair, PKCS_ED25519};
use rustls_pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

use crate::Identity;

/// Why a TLS identity could not be produced.
#[derive(Debug, thiserror::Error)]
pub enum TlsError {
    /// The identity key could not be exported as PKCS#8.
    #[error("could not encode the identity key: {0}")]
    Encoding(String),
    /// The certificate generator rejected the key.
    #[error("could not load the identity key for signing: {0}")]
    Key(String),
    /// The certificate parameters were invalid.
    #[error("could not build certificate parameters: {0}")]
    Params(String),
    /// Self-signing failed.
    #[error("could not self-sign the certificate: {0}")]
    Sign(String),
}

/// A certificate chain and private key ready for a TLS server, plus the PEM
/// forms for anything that wants files instead of DER.
pub struct TlsIdentity {
    /// DER-encoded chain, leaf first. One certificate: this is self-signed.
    pub cert_chain: Vec<CertificateDer<'static>>,
    /// DER-encoded PKCS#8 private key.
    pub key: PrivateKeyDer<'static>,
    /// PEM form of the certificate, for config files and debugging.
    pub cert_pem: String,
    /// PEM form of the private key. Must never be written to disk in the clear.
    pub key_pem: String,
}

impl std::fmt::Debug for TlsIdentity {
    /// Print the certificate but never the private key, even by accident.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TlsIdentity")
            .field("cert_chain", &self.cert_chain)
            .field("key", &"<redacted>")
            .finish_non_exhaustive()
    }
}

impl Identity {
    /// Build the TLS identity for this device.
    ///
    /// `hostnames` become subject alternative names. Callers should pass
    /// `localhost` plus the device's current LAN addresses; a peer that reaches
    /// the device by IP needs that IP in the certificate.
    pub fn tls_identity(&self, hostnames: &[String]) -> Result<TlsIdentity, TlsError> {
        let pkcs8: SecretDocument = self
            .signing_key()
            .to_pkcs8_der()
            .map_err(|e| TlsError::Encoding(e.to_string()))?;

        let key_der = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(pkcs8.as_bytes().to_vec()));
        let key_pair = KeyPair::from_der_and_sign_algo(&key_der, &PKCS_ED25519)
            .map_err(|e| TlsError::Key(e.to_string()))?;

        let mut params = CertificateParams::new(hostnames.to_vec())
            .map_err(|e| TlsError::Params(e.to_string()))?;
        params.is_ca = IsCa::NoCa;
        params
            .distinguished_name
            .push(DnType::CommonName, "flit identity");

        let cert = params
            .self_signed(&key_pair)
            .map_err(|e| TlsError::Sign(e.to_string()))?;

        Ok(TlsIdentity {
            cert_chain: vec![cert.der().clone()],
            key: key_der,
            cert_pem: cert.pem(),
            key_pem: pkcs8
                .to_pem("PRIVATE KEY", pkcs8::LineEnding::LF)
                .map_err(|e| TlsError::Encoding(e.to_string()))?
                .to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hosts() -> Vec<String> {
        vec!["localhost".to_string(), "192.168.1.20".to_string()]
    }

    #[test]
    fn certificate_public_key_is_the_identity_key() {
        let identity = Identity::from_secret_bytes([5u8; 32]);
        let tls = identity.tls_identity(&hosts()).unwrap();

        let (rest, cert) = x509_parser::parse_x509_certificate(tls.cert_chain[0].as_ref())
            .expect("certificate must parse");
        assert!(rest.is_empty(), "no trailing bytes after the certificate");

        // `public_key().raw` is the whole SubjectPublicKeyInfo (algorithm id plus
        // the BIT STRING wrapper), so compare the bit-string contents instead.
        let spki = cert.public_key();
        assert_eq!(
            spki.algorithm.algorithm.to_id_string(),
            "1.3.101.112",
            "the certificate key must be Ed25519"
        );
        assert_eq!(
            spki.subject_public_key.data,
            identity.public_key().as_slice(),
            "the certificate key must be the identity key, or pinning means nothing"
        );
    }

    #[test]
    fn certificate_carries_the_requested_names() {
        let identity = Identity::from_secret_bytes([6u8; 32]);
        let tls = identity.tls_identity(&hosts()).unwrap();
        let (_, cert) = x509_parser::parse_x509_certificate(tls.cert_chain[0].as_ref()).unwrap();

        let san = cert
            .subject_alternative_name()
            .unwrap()
            .expect("SAN must be present");
        let names: Vec<String> = san
            .value
            .general_names
            .iter()
            .filter_map(|n| match n {
                x509_parser::extensions::GeneralName::DNSName(d) => Some((*d).to_string()),
                x509_parser::extensions::GeneralName::IPAddress(ip) => Some(format!("{ip:?}")),
                _ => None,
            })
            .collect();
        assert!(names.iter().any(|n| n == "localhost"), "{names:?}");
    }

    #[test]
    fn pem_forms_are_well_formed_and_stable() {
        let identity = Identity::from_secret_bytes([7u8; 32]);
        let first = identity.tls_identity(&hosts()).unwrap();
        let second = identity.tls_identity(&hosts()).unwrap();

        assert!(first.cert_pem.starts_with("-----BEGIN CERTIFICATE-----"));
        assert!(first.key_pem.starts_with("-----BEGIN PRIVATE KEY-----"));
        assert_eq!(first.cert_pem, second.cert_pem);
        assert_eq!(first.key_pem, second.key_pem);
    }

    #[test]
    fn debug_never_leaks_the_private_key() {
        let identity = Identity::from_secret_bytes([8u8; 32]);
        let tls = identity.tls_identity(&hosts()).unwrap();
        let text = format!("{tls:?}");
        assert!(text.contains("redacted"));
        assert!(!text.contains("PRIVATE KEY"));
        // The secret scalars themselves must not appear either.
        assert!(!text.contains(&hex::encode(identity.secret_bytes())));
    }

    #[test]
    fn different_identities_produce_different_certificates() {
        let a = Identity::from_secret_bytes([1u8; 32])
            .tls_identity(&hosts())
            .unwrap();
        let b = Identity::from_secret_bytes([2u8; 32])
            .tls_identity(&hosts())
            .unwrap();
        assert_ne!(a.cert_chain[0].as_ref(), b.cert_chain[0].as_ref());
    }
}
