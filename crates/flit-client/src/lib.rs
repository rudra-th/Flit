//! The Flit sending side: dial a peer, pin its fingerprint, send files.
//!
//! Pinning is not optional. A peer is identified by the fingerprint advertised
//! in discovery, and a certificate that does not match is a hard failure. There
//! is no "continue anyway" path in this crate.
//!
//! The pin is checked against the **certificate's public key**, not against
//! anything the peer says in JSON. A peer that answers `/info` with the right
//! fingerprint while presenting a different key still fails the handshake,
//! because the handshake happens first.
//!
//! M1 status: dial, pin, and `GET /api/v1/info`. Uploads land next.

#![deny(missing_docs)]

mod send;

pub use send::{SavedFile, SendOutcome};

use std::{
    net::SocketAddr,
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};

use flit_api::{API_PREFIX, Info, PIN_HEADER, PROTO_HEADER, PROTO_VERSION, PROTO_VERSION_STR};
use flit_crypto::{ed25519_public_key, fingerprint_of};
use http_body_util::{BodyExt, Full, combinators::BoxBody};
use hyper::{Method, Request, StatusCode, body::Bytes, client::conn::http1, header};
use hyper_util::rt::TokioIo;
use rustls::{
    DigitallySignedStruct, Error as RustlsError, SignatureScheme,
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    crypto::WebPkiSupportedAlgorithms,
    pki_types::{CertificateDer, ServerName, UnixTime},
};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;

/// How long a dial (TCP connect plus TLS handshake) may take before giving up.
pub const DIAL_TIMEOUT: Duration = Duration::from_secs(5);

/// How long a small control request may take: info, prepare, finish, cancel.
pub const CONTROL_TIMEOUT: Duration = Duration::from_secs(30);

/// How long one file body may take to travel.
///
/// This is a ceiling on a whole transfer, not an idle timeout. Four gigabytes
/// over slow Wi-Fi is slow, not broken, so the budget is deliberately large.
/// Noticing a peer that has stopped reading entirely needs byte-level progress
/// tracking, which is later hardening.
pub const TRANSFER_TIMEOUT: Duration = Duration::from_secs(60 * 60);

/// Everything that can stop a send before any bytes move.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// The peer's fingerprint was not 64 lowercase hex characters.
    #[error("not a fingerprint: {0:?}")]
    BadFingerprint(String),
    /// The TCP connection could not be opened.
    #[error("could not reach {addr}: {source}")]
    Connect {
        /// The address that was refused.
        addr: SocketAddr,
        /// The operating system's reason.
        #[source]
        source: std::io::Error,
    },
    /// The handshake completed but the certificate was not the pinned identity.
    #[error("the peer's key is {presented}, but {expected} was expected")]
    PinMismatch {
        /// Fingerprint learned from discovery or a QR code.
        expected: String,
        /// Fingerprint of the key the peer actually presented.
        presented: String,
    },
    /// The peer's certificate could not be read as an Ed25519 identity.
    #[error("the peer's certificate is unusable: {0}")]
    BadCertificate(String),
    /// The TLS handshake failed for a reason other than the pin.
    #[error("TLS handshake failed: {0}")]
    Handshake(String),
    /// The HTTP connection failed.
    #[error("HTTP failed: {0}")]
    Http(#[from] hyper::Error),
    /// The peer answered with a non-success status.
    #[error("the peer answered {status}: {body}")]
    Status {
        /// HTTP status code.
        status: u16,
        /// The peer's error body, unparsed.
        body: String,
    },
    /// The response body did not match the protocol.
    #[error("could not decode the peer's response: {0}")]
    Decode(String),
    /// The peer speaks a different major version.
    #[error("the peer speaks protocol {theirs}, this build speaks {ours}")]
    Version {
        /// The peer's advertised major version.
        theirs: u16,
        /// Ours.
        ours: u16,
    },
    /// A local file could not be read.
    #[error("could not read {path}: {source}")]
    Local {
        /// The file.
        path: std::path::PathBuf,
        /// The operating system's reason.
        #[source]
        source: std::io::Error,
    },
    /// The receiver refused the item during prepare.
    #[error("the receiver refused the item: {0}")]
    Rejected(flit_api::ErrorCode),
    /// The receiver's hash did not match; nothing was saved.
    #[error("the receiver could not verify the file, so it was not saved")]
    NotVerified,
}

/// Every request body, boxed so a file stream and a small buffer share a type.
pub(crate) type RequestBody = BoxBody<Bytes, std::io::Error>;

/// A client bound to one peer and one pinned fingerprint.
pub struct Client {
    addr: SocketAddr,
    server_name: ServerName<'static>,
    fingerprint: String,
    pin: Option<String>,
    config: Arc<rustls::ClientConfig>,
    dial_timeout: Duration,
    control_timeout: Duration,
    transfer_timeout: Duration,
    presented: Arc<Mutex<Option<String>>>,
}

impl Client {
    /// Build a client for `addr` that will only accept `fingerprint`.
    ///
    /// The fingerprint is normalized to lowercase, so a value copied from a QR
    /// code or a log line works either way.
    pub fn new(addr: SocketAddr, fingerprint: &str) -> Result<Self, ClientError> {
        let fingerprint = normalize_fingerprint(fingerprint)?;
        let presented = Arc::new(Mutex::new(None));
        let config = pinned_config(&fingerprint, Arc::clone(&presented));
        Ok(Self {
            addr,
            server_name: ServerName::IpAddress(addr.ip().into()),
            fingerprint,
            pin: None,
            config,
            dial_timeout: DIAL_TIMEOUT,
            control_timeout: CONTROL_TIMEOUT,
            transfer_timeout: TRANSFER_TIMEOUT,
            presented,
        })
    }

    /// Send `pin` as `X-Flit-Pin` on every request.
    ///
    /// The PIN travels inside TLS and is never stored.
    #[must_use]
    pub fn with_pin(mut self, pin: impl Into<String>) -> Self {
        self.pin = Some(pin.into());
        self
    }

    /// Override the dial timeout.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.dial_timeout = timeout;
        self
    }

    /// Override how long one file body may take.
    ///
    /// Only useful for tests that need the transfer to fail fast; production
    /// callers should leave the default alone.
    #[must_use]
    pub fn with_transfer_timeout(mut self, timeout: Duration) -> Self {
        self.transfer_timeout = timeout;
        self
    }

    /// The pinned fingerprint, normalized.
    #[must_use]
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    /// The peer address.
    #[must_use]
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Dial, verify the pin during the handshake, and return a connection.
    pub async fn connect(&self) -> Result<Connection, ClientError> {
        // Clear any fingerprint recorded by a previous attempt on this client.
        *self.presented.lock().expect("poisoned lock") = None;

        let tcp = tokio::time::timeout(self.dial_timeout, TcpStream::connect(self.addr))
            .await
            .map_err(|_| ClientError::Connect {
                addr: self.addr,
                source: std::io::Error::new(std::io::ErrorKind::TimedOut, "dial timed out"),
            })?
            .map_err(|source| ClientError::Connect {
                addr: self.addr,
                source,
            })?;
        tcp.set_nodelay(true)
            .map_err(|source| ClientError::Connect {
                addr: self.addr,
                source,
            })?;

        let connector = TlsConnector::from(Arc::clone(&self.config));
        let tls = match connector.connect(self.server_name.clone(), tcp).await {
            Ok(tls) => tls,
            Err(err) => {
                let presented = self.presented.lock().expect("poisoned lock").clone();
                return match presented {
                    Some(presented) if presented != self.fingerprint => {
                        Err(ClientError::PinMismatch {
                            expected: self.fingerprint.clone(),
                            presented,
                        })
                    }
                    _ => Err(ClientError::Handshake(err.to_string())),
                };
            }
        };

        let (sender, driver) = http1::handshake(TokioIo::new(tls)).await?;
        let driver = tokio::spawn(async move {
            if let Err(err) = driver.await {
                log::debug!("HTTP connection closed: {err}");
            }
        });

        Ok(Connection {
            sender,
            driver,
            addr: self.addr,
            fingerprint: self.fingerprint.clone(),
            pin: self.pin.clone(),
            control_timeout: self.control_timeout,
            transfer_timeout: self.transfer_timeout,
        })
    }
}

/// One live connection to a pinned peer.
pub struct Connection {
    pub(crate) sender: http1::SendRequest<RequestBody>,
    driver: tokio::task::JoinHandle<()>,
    pub(crate) addr: SocketAddr,
    pub(crate) fingerprint: String,
    pub(crate) pin: Option<String>,
    pub(crate) control_timeout: Duration,
    pub(crate) transfer_timeout: Duration,
}

impl Connection {
    /// The peer address.
    #[must_use]
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// The pinned fingerprint.
    #[must_use]
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    /// Build an origin-form request with every header the protocol requires.
    pub(crate) fn request(
        &self,
        method: Method,
        path: String,
        body: RequestBody,
    ) -> Result<Request<RequestBody>, ClientError> {
        let mut builder = Request::builder()
            .method(method)
            .uri(path)
            .header(header::HOST, host_header(self.addr))
            .header(PROTO_HEADER, PROTO_VERSION_STR);
        if let Some(pin) = &self.pin {
            builder = builder.header(PIN_HEADER, pin);
        }
        builder
            .body(body)
            .map_err(|e| ClientError::Decode(e.to_string()))
    }

    /// Send a control request and collect a bounded response.
    pub(crate) async fn execute(
        &mut self,
        request: Request<RequestBody>,
    ) -> Result<(StatusCode, hyper::HeaderMap, Bytes), ClientError> {
        let timeout = self.control_timeout;
        self.execute_within(request, timeout).await
    }

    /// Send a request, waiting at most `timeout` for the response head.
    ///
    /// Upload replies are tiny; the only large body on this connection is the
    /// request side, which is why a body transfer gets its own budget.
    pub(crate) async fn execute_within(
        &mut self,
        request: Request<RequestBody>,
        timeout: Duration,
    ) -> Result<(StatusCode, hyper::HeaderMap, Bytes), ClientError> {
        let response = tokio::time::timeout(timeout, self.sender.send_request(request))
            .await
            .map_err(|_| {
                ClientError::Decode(format!(
                    "the peer did not answer within {:.0}s",
                    timeout.as_secs_f64()
                ))
            })??;

        let status = response.status();
        let headers = response.headers().clone();
        let body = response.into_body().collect().await?.to_bytes();
        Ok((status, headers, body))
    }

    /// Fetch `GET /api/v1/info` and check that it agrees with the pin.
    pub async fn info(&mut self) -> Result<Info, ClientError> {
        let request = self.request(
            Method::GET,
            format!("{API_PREFIX}/info"),
            bytes_body(Bytes::new()),
        )?;
        let (status, _, body) = self.execute(request).await?;

        if status != StatusCode::OK {
            return Err(ClientError::Status {
                status: status.as_u16(),
                body: String::from_utf8_lossy(&body).into_owned(),
            });
        }

        let info: Info =
            serde_json::from_slice(&body).map_err(|e| ClientError::Decode(e.to_string()))?;
        if info.proto != PROTO_VERSION {
            return Err(ClientError::Version {
                theirs: info.proto,
                ours: PROTO_VERSION,
            });
        }
        // Pinning already proved the key. This catches a peer that lies in JSON
        // about which key it presented, which is a bug worth reporting loudly.
        if info.fingerprint != self.fingerprint {
            return Err(ClientError::PinMismatch {
                expected: self.fingerprint.clone(),
                presented: info.fingerprint,
            });
        }
        Ok(info)
    }

    /// Close the connection and stop the driver task.
    pub fn close(self) {
        // Dropping `sender` ends the driver; abort covers the case where the
        // driver is blocked on a half-open socket.
        self.driver.abort();
    }
}

impl std::fmt::Debug for Connection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connection")
            .field("addr", &self.addr)
            .field("fingerprint", &self.fingerprint)
            .finish_non_exhaustive()
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.driver.abort();
    }
}

/// Read `fingerprint` leniently, refusing anything that is not a SHA-256.
fn normalize_fingerprint(input: &str) -> Result<String, ClientError> {
    let lower = input.trim().to_ascii_lowercase();
    let valid = lower.len() == 64 && lower.bytes().all(|b| b.is_ascii_hexdigit());
    if valid {
        Ok(lower)
    } else {
        Err(ClientError::BadFingerprint(input.to_string()))
    }
}

/// `ip:port`, wrapping IPv6 in brackets.
fn host_header(addr: SocketAddr) -> String {
    match addr.ip() {
        std::net::IpAddr::V6(ip) => format!("[{ip}]:{}", addr.port()),
        std::net::IpAddr::V4(ip) => format!("{ip}:{}", addr.port()),
    }
}

/// Wrap bytes in the boxed request-body type.
pub(crate) fn bytes_body(bytes: Bytes) -> RequestBody {
    Full::new(bytes).map_err(|never| match never {}).boxed()
}

/// Signature schemes accepted during the handshake, from the ring provider.
fn verification_algorithms() -> &'static WebPkiSupportedAlgorithms {
    static ALGORITHMS: OnceLock<WebPkiSupportedAlgorithms> = OnceLock::new();
    ALGORITHMS
        .get_or_init(|| rustls::crypto::ring::default_provider().signature_verification_algorithms)
}

/// A certificate verifier whose entire policy is "this exact key".
///
/// Chain building, expiry and subject names are deliberately not consulted:
/// Flit certificates are self-signed and the identity is the key. Checking a
/// name would add a failure mode without adding a guarantee.
struct PinnedIdentity {
    fingerprint: String,
    presented: Arc<Mutex<Option<String>>>,
}

impl std::fmt::Debug for PinnedIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PinnedIdentity")
            .field("fingerprint", &self.fingerprint)
            .finish_non_exhaustive()
    }
}

impl ServerCertVerifier for PinnedIdentity {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, RustlsError> {
        let key = ed25519_public_key(end_entity.as_ref()).map_err(|e| {
            RustlsError::InvalidCertificate(rustls::CertificateError::Other(rustls::OtherError(
                Arc::new(UnreadableCertificate(e.to_string())),
            )))
        })?;
        let presented = fingerprint_of(&key);
        *self.presented.lock().expect("poisoned lock") = Some(presented.clone());

        if presented == self.fingerprint {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(RustlsError::InvalidCertificate(
                rustls::CertificateError::ApplicationVerificationFailure,
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, RustlsError> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, verification_algorithms())
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, RustlsError> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, verification_algorithms())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        verification_algorithms().supported_schemes()
    }
}

/// Wrapper so a certificate parse failure survives inside rustls' error type.
#[derive(Debug)]
struct UnreadableCertificate(String);

impl std::fmt::Display for UnreadableCertificate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "unreadable certificate: {}", self.0)
    }
}

impl std::error::Error for UnreadableCertificate {}

/// Build a TLS 1.3 client config that trusts exactly one key.
fn pinned_config(
    fingerprint: &str,
    presented: Arc<Mutex<Option<String>>>,
) -> Arc<rustls::ClientConfig> {
    let mut config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_protocol_versions(&[&rustls::version::TLS13])
    .expect("the ring provider supports TLS 1.3")
    .dangerous()
    .with_custom_certificate_verifier(Arc::new(PinnedIdentity {
        fingerprint: fingerprint.to_string(),
        presented,
    }))
    .with_no_client_auth();
    config.alpn_protocols = vec![b"flit/1".to_vec()];
    Arc::new(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_fingerprints_leniently() {
        let hex = "A".repeat(64);
        assert_eq!(normalize_fingerprint(&hex).unwrap(), "a".repeat(64));
        let padded = format!("  {}  ", "b".repeat(64));
        assert_eq!(normalize_fingerprint(&padded).unwrap(), "b".repeat(64));
    }

    #[test]
    fn rejects_things_that_are_not_fingerprints() {
        for bad in ["", "abc", &"g".repeat(64), &"a".repeat(63), &"a".repeat(65)] {
            assert!(
                matches!(
                    normalize_fingerprint(bad),
                    Err(ClientError::BadFingerprint(_))
                ),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn host_headers_bracket_ipv6() {
        let v4: SocketAddr = "192.168.1.5:47821".parse().unwrap();
        assert_eq!(host_header(v4), "192.168.1.5:47821");
        let v6: SocketAddr = "[fe80::1]:47821".parse().unwrap();
        assert_eq!(host_header(v6), "[fe80::1]:47821");
    }
}
