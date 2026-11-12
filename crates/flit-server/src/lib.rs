//! The Flit receiving side: a TLS server that accepts uploads.
//!
//! The server owns four things: identity, sessions, the destination root, and
//! verification. It never trusts a declared size, never opens a path before
//! sanitizing it, and never renames a `.part` file into place until the hash
//! matches.
//!
//! TLS 1.3 only. The certificate is the device identity (see
//! `flit_crypto::TlsIdentity`), so a peer that pins the discovery fingerprint is
//! checking the same key it is talking to.

#![deny(missing_docs)]

mod http;
mod pin;
mod session;

pub use http::{ResponseBody, State};
pub use pin::{MAX_ATTEMPTS, PinError, PinGuard};
pub use session::{Commit, Item, Session, Sessions, is_sha256};

use std::{
    future::Future,
    net::{Ipv4Addr, SocketAddr, UdpSocket},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use flit_api::{DEFAULT_ALIAS, DeviceType, Info, PROTO_VERSION, TLS_PORT, caps};
use flit_crypto::{Identity, TlsError};
use hyper::{Request, body::Incoming, service::service_fn};
use hyper_util::rt::TokioIo;
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsAcceptor;

/// Capabilities this build can actually serve.
///
/// Deliberately not "everything in the spec": a peer that is told `pack` exists
/// will send a pack, so advertising it before it works is a lie the protocol has
/// no way to recover from.
#[must_use]
pub fn implemented_caps() -> Vec<String> {
    vec![
        caps::RESUME.to_string(),
        caps::PACK.to_string(),
        caps::VERIFY.to_string(),
        caps::TEXT.to_string(),
    ]
}

/// How the server presents itself and where files land.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// Name shown to other devices. Cosmetic and not trusted.
    pub alias: String,
    /// Device class shown in the peer list.
    pub device_type: DeviceType,
    /// Address to bind. Port 0 asks the OS for a free port, which the announce
    /// then carries.
    pub bind: SocketAddr,
    /// Directory that received files are written into.
    pub root: PathBuf,
    /// Capability tokens this build can actually serve.
    pub caps: Vec<String>,
    /// PIN required for first contact, if one is configured.
    ///
    /// `None` is an open receiver. `pin_required` in `/info` is derived from
    /// this, so the two can never disagree.
    pub pin: Option<String>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            alias: DEFAULT_ALIAS.to_string(),
            device_type: DeviceType::Desktop,
            bind: SocketAddr::from(([0, 0, 0, 0], TLS_PORT)),
            root: default_receive_dir(),
            caps: implemented_caps(),
            pin: None,
        }
    }
}

impl ServerConfig {
    /// Set the display name.
    #[must_use]
    pub fn with_alias(mut self, alias: impl Into<String>) -> Self {
        self.alias = alias.into();
        self
    }

    /// Set the device class.
    #[must_use]
    pub fn with_device_type(mut self, device_type: DeviceType) -> Self {
        self.device_type = device_type;
        self
    }

    /// Bind somewhere else, e.g. loopback and an ephemeral port for tests.
    #[must_use]
    pub fn with_bind(mut self, bind: SocketAddr) -> Self {
        self.bind = bind;
        self
    }

    /// Choose the destination directory.
    #[must_use]
    pub fn with_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.root = root.into();
        self
    }

    /// Require a PIN for first contact. See [`PinGuard`] for the format rules.
    #[must_use]
    pub fn with_pin(mut self, pin: impl Into<String>) -> Self {
        self.pin = Some(pin.into());
        self
    }
}

/// Things that stop a server from starting.
#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    /// The listening socket could not be created.
    #[error("could not bind {addr}: {source}")]
    Bind {
        /// The address that was refused.
        addr: SocketAddr,
        /// The operating system's reason.
        #[source]
        source: std::io::Error,
    },
    /// The destination directory could not be prepared.
    #[error("could not prepare {path}: {source}")]
    Root {
        /// The destination directory.
        path: PathBuf,
        /// The operating system's reason.
        #[source]
        source: std::io::Error,
    },
    /// The identity could not be turned into a certificate.
    #[error(transparent)]
    Tls(#[from] TlsError),
    /// rustls refused the certificate and key.
    #[error("TLS configuration was rejected: {0}")]
    TlsConfig(String),
    /// The configured PIN is not usable.
    #[error(transparent)]
    Pin(#[from] PinError),
    /// A socket operation failed after startup.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// A bound Flit receiver.
pub struct Server {
    listener: TcpListener,
    acceptor: TlsAcceptor,
    local_addr: SocketAddr,
    state: Arc<State>,
}

impl Server {
    /// Bind, build the TLS listener, and prepare the destination root.
    ///
    /// This does not start accepting connections; call [`Server::run`] or
    /// [`Server::run_until`].
    pub async fn bind(identity: &Identity, config: ServerConfig) -> Result<Self, ServerError> {
        // A PIN is validated before anything is bound or created, so a typo in
        // the configuration fails at startup instead of silently leaving the
        // receiver open.
        let pins = PinGuard::new(config.pin.clone())?;

        let listener =
            TcpListener::bind(config.bind)
                .await
                .map_err(|source| ServerError::Bind {
                    addr: config.bind,
                    source,
                })?;
        let local_addr = listener.local_addr()?;

        let sessions = Sessions::new(&config.root).map_err(|source| ServerError::Root {
            path: config.root.clone(),
            source,
        })?;

        let tls = identity.tls_identity(&subject_names(local_addr))?;
        let mut tls_config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| ServerError::TlsConfig(e.to_string()))?
        .with_no_client_auth()
        .with_single_cert(tls.cert_chain.clone(), tls.key)
        .map_err(|e| ServerError::TlsConfig(e.to_string()))?;
        tls_config.alpn_protocols = vec![b"flit/1".to_vec()];

        let info = Info {
            proto: PROTO_VERSION,
            alias: config.alias,
            device_id: identity.device_id(),
            fingerprint: identity.fingerprint(),
            device_type: config.device_type,
            port: local_addr.port(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            caps: config.caps,
            pin_required: pins.required(),
        };

        Ok(Self {
            listener,
            acceptor: TlsAcceptor::from(Arc::new(tls_config)),
            local_addr,
            state: Arc::new(State {
                info,
                sessions,
                pins,
            }),
        })
    }

    /// The address actually bound, which is the authoritative one.
    #[must_use]
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// What this server advertises to peers.
    #[must_use]
    pub fn info(&self) -> &Info {
        &self.state.info
    }

    /// The destination directory.
    #[must_use]
    pub fn root(&self) -> &std::path::Path {
        self.state.sessions.root()
    }

    /// Accept connections until the task is dropped.
    pub async fn run(self) -> std::io::Result<()> {
        self.run_until(std::future::pending()).await
    }

    /// Accept connections until `shutdown` resolves.
    pub async fn run_until<F>(self, shutdown: F) -> std::io::Result<()>
    where
        F: Future<Output = ()>,
    {
        tokio::pin!(shutdown);
        loop {
            tokio::select! {
                () = &mut shutdown => return Ok(()),
                accepted = self.listener.accept() => {
                    match accepted {
                        Ok((stream, peer)) => {
                            let acceptor = self.acceptor.clone();
                            let state = Arc::clone(&self.state);
                            tokio::spawn(async move {
                                handle_connection(stream, peer, acceptor, state).await;
                            });
                        }
                        // A single failed accept must not take the listener down.
                        Err(err) => {
                            log::warn!("accept failed: {err}");
                            tokio::time::sleep(Duration::from_millis(50)).await;
                        }
                    }
                }
            }
        }
    }
}

/// Complete one TLS handshake and serve HTTP/1.1 on it.
///
/// Handshake failures are expected: subnet scans, port probes and peers with a
/// different key all land here, and none of them are errors worth surfacing.
async fn handle_connection(
    stream: TcpStream,
    peer: SocketAddr,
    acceptor: TlsAcceptor,
    state: Arc<State>,
) {
    if let Err(err) = stream.set_nodelay(true) {
        log::debug!("could not disable Nagle for {peer}: {err}");
    }

    let tls = match acceptor.accept(stream).await {
        Ok(tls) => tls,
        Err(err) => {
            log::debug!("TLS handshake with {peer} failed: {err}");
            return;
        }
    };

    let service = service_fn(move |request: Request<Incoming>| {
        let state = Arc::clone(&state);
        async move { Ok::<_, std::convert::Infallible>(http::route(request, state, peer.ip()).await) }
    });

    if let Err(err) = hyper::server::conn::http1::Builder::new()
        .serve_connection(TokioIo::new(tls), service)
        .await
    {
        log::debug!("connection from {peer} ended: {err}");
    }
}

/// The default receive directory: a `Flit` folder inside the user's downloads.
#[must_use]
pub fn default_receive_dir() -> PathBuf {
    let home = if cfg!(windows) {
        std::env::var_os("USERPROFILE").map(PathBuf::from)
    } else {
        std::env::var_os("HOME").map(PathBuf::from)
    };
    home.unwrap_or_else(|| PathBuf::from("."))
        .join("Downloads")
        .join("Flit")
}

/// Names to put in the certificate's subject alternative names.
///
/// Pinning ignores these, but a certificate that names the addresses it is
/// reached on is easier to inspect when something goes wrong.
fn subject_names(local_addr: SocketAddr) -> Vec<String> {
    let mut names = vec!["localhost".to_string(), "127.0.0.1".to_string()];
    if local_addr.is_ipv6() {
        names.push("::1".to_string());
    }
    names.extend(local_ipv4_addrs().into_iter().map(|ip| ip.to_string()));
    names.sort();
    names.dedup();
    names
}

/// Best-effort list of this machine's LAN IPv4 addresses.
///
/// Uses the "connect a UDP socket and read back the chosen local address"
/// trick: no packets are sent, and a machine with no route simply gets an
/// empty list rather than an error.
#[must_use]
pub fn local_ipv4_addrs() -> Vec<Ipv4Addr> {
    let Ok(socket) = UdpSocket::bind("0.0.0.0:0") else {
        return Vec::new();
    };
    // TEST-NET-1 is never routed anywhere real; this only asks the kernel which
    // interface it would use for the default route.
    if socket.connect("192.0.2.1:9").is_err() {
        return Vec::new();
    }
    match socket.local_addr() {
        Ok(SocketAddr::V4(addr)) if !addr.ip().is_unspecified() => vec![*addr.ip()],
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults_to_the_control_port() {
        let config = ServerConfig::default();
        assert_eq!(config.bind.port(), TLS_PORT);
        assert_eq!(config.alias, DEFAULT_ALIAS);
        assert!(config.caps.contains(&caps::RESUME.to_string()));
    }

    #[test]
    fn we_advertise_what_we_can_serve_and_nothing_else() {
        let caps = implemented_caps();
        assert!(caps.contains(&caps::PACK.to_string()));
        assert!(!caps.contains(&caps::PROGRESS_SSE.to_string()));
    }

    #[test]
    fn subject_names_cover_loopback_and_are_deduplicated() {
        let names = subject_names(SocketAddr::from(([127, 0, 0, 1], 47821)));
        assert!(names.contains(&"localhost".to_string()));
        assert!(names.contains(&"127.0.0.1".to_string()));
        let mut sorted = names.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(names, sorted);
    }
}
