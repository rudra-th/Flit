//! The running engine: one receiver, the devices around it, and sends.
//!
//! A UI opens an [`Engine`], subscribes to [`Event`]s, and calls methods. It
//! never dials a socket itself and never builds a manifest. This is the whole
//! boundary the desktop and Android shells are allowed to cross.
//!
//! Discovery is best-effort by design: a machine whose network filters
//! multicast still needs to receive files and still needs to send them to an
//! address typed from a QR code. A discovery failure is announced as
//! [`Event::DiscoveryUnavailable`] and never stops the engine from starting.

use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use flit_api::{DeviceType, Info};
use flit_client::{Client, ClientError, FileOutcome, SendTarget};
use flit_crypto::{Badge, Identity, badge_of};
use flit_discovery::{Discovery, DiscoveryConfig, Peer, PeerSource, announce_for};
use flit_server::{Server, ServerConfig, ServerError};
use tokio::{
    sync::{Mutex, broadcast, oneshot},
    task::JoinHandle,
};

/// How many events a slow subscriber may fall behind before it misses some.
///
/// Progress and peer churn are not worth blocking a UI for; a subscriber that
/// falls this far behind is told it missed events and should read the current
/// state instead.
pub const EVENT_BUFFER: usize = 256;

/// How often the engine compares the discovery table with what it last said.
const PEER_POLL: Duration = Duration::from_secs(1);

/// How long [`Engine::stop`] waits for the listener to close.
const STOP_TIMEOUT: Duration = Duration::from_secs(5);

/// Identifier for one send, unique within an engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TransferId(u64);

impl TransferId {
    /// The underlying number, for logs and for a UI that keys rows by it.
    #[must_use]
    pub fn as_u64(self) -> u64 {
        self.0
    }
}

impl std::fmt::Display for TransferId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "t{}", self.0)
    }
}

/// A device the engine can see, as the UI should present it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerView {
    /// Stable identifier from the announce.
    pub device_id: String,
    /// Display name. Cosmetic and never trusted.
    pub alias: String,
    /// Key fingerprint to pin. This is the identity that matters.
    pub fingerprint: String,
    /// Cosmetic device class.
    pub device_type: DeviceType,
    /// Where the control plane answers.
    pub addr: SocketAddr,
    /// How this device was learned about.
    pub source: PeerSource,
    /// Emoji and word badge derived from the fingerprint, when it is valid.
    pub badge: Option<Badge>,
}

impl PeerView {
    fn from_peer(peer: &Peer) -> Self {
        let announce = peer.announce();
        Self {
            device_id: announce.device_id.clone(),
            alias: announce.alias.clone(),
            fingerprint: announce.fingerprint.clone(),
            device_type: announce.device_type,
            addr: peer.addr(),
            source: peer.source(),
            badge: badge_of(&announce.fingerprint),
        }
    }
}

/// One item's result inside a finished transfer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemOutcome {
    /// Local file that was offered.
    pub path: PathBuf,
    /// Name the receiver was asked to save it as.
    pub name: String,
    /// Where it actually landed, after conflict resolution.
    pub saved_as: Option<String>,
    /// Bytes the receiver verified.
    pub bytes: u64,
    /// Why it failed, in the receiver's words, if it did.
    pub error: Option<String>,
}

impl ItemOutcome {
    fn from_file_outcome(outcome: FileOutcome) -> Self {
        match outcome.result {
            Ok(saved) => Self {
                path: outcome.path,
                name: outcome.name,
                saved_as: Some(saved.saved_as),
                bytes: saved.bytes,
                error: None,
            },
            Err(err) => Self {
                path: outcome.path,
                name: outcome.name,
                saved_as: None,
                bytes: 0,
                error: Some(err.to_string()),
            },
        }
    }
}

/// Something a UI may want to show. Every variant is cheap to clone.
#[derive(Debug, Clone)]
pub enum Event {
    /// A device appeared, or its announce changed.
    PeerSeen(PeerView),
    /// A device stopped answering.
    PeerLost {
        /// Identifier of the device that went away.
        device_id: String,
    },
    /// Discovery could not start. The engine is still usable.
    DiscoveryUnavailable {
        /// Why, in words a support issue can quote.
        reason: String,
    },
    /// A send was handed to the network.
    TransferStarted {
        /// Which send.
        id: TransferId,
        /// How many files it covers.
        files: usize,
        /// Total bytes declared by the local files.
        bytes: u64,
        /// Receiver being sent to.
        peer: SocketAddr,
    },
    /// Every item of a send has a result.
    TransferFinished {
        /// Which send.
        id: TransferId,
        /// One entry per file, in the order they were offered.
        items: Vec<ItemOutcome>,
    },
    /// The session itself failed, before any per-item result existed.
    TransferFailed {
        /// Which send.
        id: TransferId,
        /// Why, in the sender's words.
        reason: String,
    },
    /// A send was stopped by [`Engine::cancel`].
    TransferCancelled {
        /// Which send.
        id: TransferId,
    },
}

/// Why the engine could not do what was asked.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    /// The receiver could not bind, load its identity, or start.
    #[error("could not start the receiver: {0}")]
    Server(#[from] ServerError),
    /// The peer's fingerprint is not usable, so no connection was attempted.
    #[error("{0}")]
    Client(#[from] ClientError),
    /// Nothing was given to send.
    #[error("nothing to send")]
    NothingToSend,
    /// No visible device has that identifier.
    #[error("no nearby device with id {0}")]
    UnknownPeer(String),
}

/// How to start an engine.
#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// Name shown to other devices.
    pub alias: String,
    /// Device class shown in the peer list.
    pub device_type: DeviceType,
    /// Address to bind. Port 0 asks the operating system for a free port.
    pub bind: SocketAddr,
    /// Directory that received files are written into.
    pub root: PathBuf,
    /// PIN required from senders, if any.
    pub pin: Option<String>,
    /// Discovery settings, or `None` to run without it.
    ///
    /// `None` is what tests use, and what a headless receiver can use on a
    /// network where multicast is known to be filtered.
    pub discovery: Option<DiscoveryConfig>,
}

impl Default for EngineConfig {
    fn default() -> Self {
        // The receiver's defaults are the engine's defaults, so the two can
        // never disagree about where files land or which port is used.
        let server = ServerConfig::default();
        Self {
            alias: server.alias,
            device_type: server.device_type,
            bind: server.bind,
            root: server.root,
            pin: server.pin,
            discovery: Some(DiscoveryConfig::default()),
        }
    }
}

/// The engine handle every UI holds.
///
/// Cheap to clone: all clones share one receiver, one peer table and one event
/// stream. Dropping the last clone does **not** stop the receiver; call
/// [`Engine::stop`] on the way out.
#[derive(Clone)]
pub struct Engine {
    inner: Arc<Inner>,
}

struct Inner {
    info: Info,
    addr: SocketAddr,
    root: PathBuf,
    pin: Option<String>,
    events: broadcast::Sender<Event>,
    server: Mutex<Option<JoinHandle<()>>>,
    shutdown: Mutex<Option<oneshot::Sender<()>>>,
    discovery: Mutex<Option<Discovery>>,
    watch: Mutex<Option<JoinHandle<()>>>,
    transfers: Mutex<HashMap<TransferId, JoinHandle<()>>>,
    next_transfer: AtomicU64,
}

impl Engine {
    /// Bind the receiver, start discovery if asked, and return a handle.
    pub async fn start(identity: Arc<Identity>, config: EngineConfig) -> Result<Self, EngineError> {
        let mut server_config = ServerConfig::default()
            .with_alias(config.alias)
            .with_device_type(config.device_type)
            .with_bind(config.bind)
            .with_root(&config.root);
        if let Some(pin) = &config.pin {
            server_config = server_config.with_pin(pin.clone());
        }

        let server = Server::bind(&identity, server_config).await?;
        let info = server.info().clone();
        let addr = server.local_addr();
        let root = server.root().to_path_buf();

        let (shutdown, stopped) = oneshot::channel();
        let server_task = tokio::spawn(async move {
            if let Err(err) = server
                .run_until(async move {
                    let _ = stopped.await;
                })
                .await
            {
                log::error!("receiver stopped accepting connections: {err}");
            }
        });

        let (events, _) = broadcast::channel(EVENT_BUFFER);

        let discovery = match config.discovery {
            Some(mut discovery_config) => {
                // The announce must carry the port actually bound, which is
                // only known after the listener exists.
                discovery_config.announce = true;
                match Discovery::start(announce_for(&info), discovery_config).await {
                    Ok(discovery) => Some(discovery),
                    Err(err) => {
                        log::warn!("discovery is unavailable: {err}");
                        let _ = events.send(Event::DiscoveryUnavailable {
                            reason: err.to_string(),
                        });
                        None
                    }
                }
            }
            None => None,
        };

        let watch = discovery.as_ref().map(|discovery| {
            let table = discovery.table();
            let events = events.clone();
            tokio::spawn(async move {
                let mut ticker = tokio::time::interval(PEER_POLL);
                // `interval` fires immediately; peers were not observed yet.
                ticker.tick().await;
                let mut known: HashMap<String, PeerView> = HashMap::new();
                loop {
                    ticker.tick().await;
                    let now = Instant::now();
                    let alive: Vec<PeerView> = {
                        let mut guard = table.write().await;
                        let _ = guard.prune(now);
                        guard
                            .snapshot(now)
                            .into_iter()
                            .filter(|peer| !peer.is_expired(now))
                            .map(PeerView::from_peer)
                            .collect()
                    };

                    let mut seen: HashSet<String> = HashSet::with_capacity(alive.len());
                    for peer in alive {
                        seen.insert(peer.device_id.clone());
                        if known.get(&peer.device_id) != Some(&peer) {
                            known.insert(peer.device_id.clone(), peer.clone());
                            let _ = events.send(Event::PeerSeen(peer));
                        }
                    }

                    let mut lost = Vec::new();
                    known.retain(|device_id, _| {
                        if seen.contains(device_id) {
                            true
                        } else {
                            lost.push(device_id.clone());
                            false
                        }
                    });
                    for device_id in lost {
                        let _ = events.send(Event::PeerLost { device_id });
                    }
                }
            })
        });

        Ok(Self {
            inner: Arc::new(Inner {
                info,
                addr,
                root,
                pin: config.pin,
                events,
                server: Mutex::new(Some(server_task)),
                shutdown: Mutex::new(Some(shutdown)),
                discovery: Mutex::new(discovery),
                watch: Mutex::new(watch),
                transfers: Mutex::new(HashMap::new()),
                next_transfer: AtomicU64::new(1),
            }),
        })
    }

    /// What this engine advertises to peers.
    #[must_use]
    pub fn info(&self) -> &Info {
        &self.inner.info
    }

    /// The address the receiver actually bound.
    #[must_use]
    pub fn addr(&self) -> SocketAddr {
        self.inner.addr
    }

    /// Where received files are written.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.inner.root
    }

    /// Subscribe to events. Each subscriber gets its own backlog from this point.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.inner.events.subscribe()
    }

    /// The devices currently visible, sorted by name.
    pub async fn peers(&self) -> Vec<PeerView> {
        // Clone the table's `Arc` out of the guard: holding the engine lock
        // while reading the peer table would block [`Engine::stop`].
        let table = {
            let discovery = self.inner.discovery.lock().await;
            let Some(discovery) = discovery.as_ref() else {
                return Vec::new();
            };
            discovery.table()
        };
        let table = table.read().await;
        let now = Instant::now();
        let mut peers: Vec<PeerView> = table
            .snapshot(now)
            .into_iter()
            .filter(|peer| !peer.is_expired(now))
            .map(PeerView::from_peer)
            .collect();
        peers.sort_by_key(|peer| (peer.alias.to_lowercase(), peer.device_id.clone()));
        peers
    }

    /// Send files to one device from the peer list.
    pub async fn send_to_peer(
        &self,
        device_id: &str,
        targets: Vec<SendTarget>,
    ) -> Result<TransferId, EngineError> {
        let peer = self
            .peers()
            .await
            .into_iter()
            .find(|peer| peer.device_id == device_id)
            .ok_or_else(|| EngineError::UnknownPeer(device_id.to_string()))?;
        self.send_to(peer.addr, &peer.fingerprint, targets).await
    }

    /// Send files to an address and a fingerprint, without discovery.
    ///
    /// This is the manual path: a QR code, an address a user typed, or a
    /// device learned from a subnet scan.
    pub async fn send_to(
        &self,
        addr: SocketAddr,
        fingerprint: &str,
        targets: Vec<SendTarget>,
    ) -> Result<TransferId, EngineError> {
        if targets.is_empty() {
            return Err(EngineError::NothingToSend);
        }

        // Build the client here so a malformed fingerprint fails the call
        // instead of becoming an event nobody is watching yet.
        let mut client = Client::new(addr, fingerprint)?;
        if let Some(pin) = &self.inner.pin {
            client = client.with_pin(pin.clone());
        }

        let id = TransferId(self.inner.next_transfer.fetch_add(1, Ordering::Relaxed));
        let files = targets.len();
        let bytes: u64 = targets
            .iter()
            .filter_map(|target| std::fs::metadata(&target.path).ok())
            .map(|metadata| metadata.len())
            .sum();
        let events = self.inner.events.clone();

        let handle = tokio::spawn(async move {
            let _ = events.send(Event::TransferStarted {
                id,
                files,
                bytes,
                peer: addr,
            });

            let mut connection = match client.connect().await {
                Ok(connection) => connection,
                Err(err) => {
                    let _ = events.send(Event::TransferFailed {
                        id,
                        reason: err.to_string(),
                    });
                    return;
                }
            };
            let result = connection.send_files(&targets).await;
            connection.close();

            match result {
                Ok(outcomes) => {
                    let items = outcomes
                        .into_iter()
                        .map(ItemOutcome::from_file_outcome)
                        .collect();
                    let _ = events.send(Event::TransferFinished { id, items });
                }
                Err(err) => {
                    let _ = events.send(Event::TransferFailed {
                        id,
                        reason: err.to_string(),
                    });
                }
            }
        });

        let mut transfers = self.inner.transfers.lock().await;
        transfers.retain(|_, handle| !handle.is_finished());
        transfers.insert(id, handle);
        Ok(id)
    }

    /// Stop a send that is still running.
    ///
    /// Returns `false` when the id is unknown or already finished. The sender
    /// closes the connection; the receiver keeps whatever it verified and the
    /// session it had open, which a later send of the same files resumes.
    pub async fn cancel(&self, id: TransferId) -> bool {
        let mut transfers = self.inner.transfers.lock().await;
        transfers.retain(|_, handle| !handle.is_finished());
        match transfers.remove(&id) {
            Some(handle) => {
                handle.abort();
                let _ = self.inner.events.send(Event::TransferCancelled { id });
                true
            }
            None => false,
        }
    }

    /// Ids of sends that have not finished or been cancelled.
    pub async fn running(&self) -> Vec<TransferId> {
        let mut transfers = self.inner.transfers.lock().await;
        transfers.retain(|_, handle| !handle.is_finished());
        let mut ids: Vec<TransferId> = transfers.keys().copied().collect();
        ids.sort_unstable();
        ids
    }

    /// Stop sending, stop discovering and close the listener.
    ///
    /// The engine is finished afterwards; start a new one to receive again.
    pub async fn stop(&self) {
        for (_, handle) in self.inner.transfers.lock().await.drain() {
            handle.abort();
        }
        if let Some(watch) = self.inner.watch.lock().await.take() {
            watch.abort();
        }
        drop(self.inner.discovery.lock().await.take());
        if let Some(shutdown) = self.inner.shutdown.lock().await.take() {
            let _ = shutdown.send(());
        }
        if let Some(server) = self.inner.server.lock().await.take() {
            let _ = tokio::time::timeout(STOP_TIMEOUT, server).await;
        }
    }
}
