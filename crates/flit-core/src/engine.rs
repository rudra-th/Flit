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
        Arc, Mutex as StdMutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use flit_api::{DeviceType, Info, ItemKind, ManifestItem};
use flit_client::{Client, ClientError, FileOutcome, SavedFile, SendProgress, SendTarget};
use flit_crypto::Identity;
use flit_discovery::{Discovery, DiscoveryConfig, Peer, PeerSource, announce_for};
use flit_server::{Offer, OfferDecision, OfferGate, Server, ServerConfig, ServerError};
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

/// How often a running send reports bytes moved.
///
/// Faster than a UI can draw and slower than the wire: progress is a courtesy,
/// never a per-frame cost.
const PROGRESS_TICK: Duration = Duration::from_millis(200);

/// How long [`Engine::stop`] waits for the listener to close.
const STOP_TIMEOUT: Duration = Duration::from_secs(5);

/// How long an incoming offer waits for a person to answer it.
///
/// After this the sender is told nobody answered, the sheet closes, and the
/// receiver goes back to being available. Long enough to read a file list,
/// short enough that a sender is not left staring at a dead screen.
pub const OFFER_TIMEOUT: Duration = Duration::from_secs(60);

/// The sentence shown when a sender did not announce a usable name.
const UNNAMED_SENDER: &str = "A nearby device";

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
        }
    }
}

/// What multicast discovery is doing, as the engine knows it.
///
/// A shell shows this instead of guessing: an unavailable discovery is a
/// warning in the window, not a reason to refuse to send or receive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscoveryState {
    /// Announcing and listening.
    Running,
    /// Started without discovery, by configuration.
    Off,
    /// Could not start. The engine still sends and receives.
    Unavailable(String),
}

/// One item's result inside a finished transfer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemOutcome {
    /// Local file that was offered; empty for an inline text item.
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

/// A short name for a whole send: the folder the files came out of, or the
/// first file's name. Takes the first path segment so a folder send says
/// "Q3 handover" instead of "Q3 handover/floorplan.pdf".
fn send_label(targets: &[SendTarget]) -> String {
    let Some(first) = targets.first() else {
        return String::new();
    };
    if let Some(name) = &first.name {
        return name.split('/').next().unwrap_or(name).to_owned();
    }
    first
        .path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
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

    /// An inline text item has no local file; its name is the only thing the
    /// lane can show before it lands.
    fn from_text(name: &str, saved: SavedFile) -> Self {
        Self {
            path: PathBuf::new(),
            name: name.to_owned(),
            saved_as: Some(saved.saved_as),
            bytes: saved.bytes,
            error: None,
        }
    }
}

/// Identifier of an incoming offer, unique for the engine's lifetime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OfferId(u64);

impl OfferId {
    /// The raw number, for shells that carry it as a plain integer.
    #[must_use]
    pub fn as_u64(self) -> u64 {
        self.0
    }

    /// Rebuild an id from the number a shell carried across a boundary.
    ///
    /// Only a number taken from an [`Event::IncomingOffer`] names an offer;
    /// anything else answers nothing and returns false.
    #[must_use]
    pub fn from_raw(number: u64) -> Self {
        Self(number)
    }
}

/// Who is asking to send something.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfferSender {
    /// Fingerprint proven by the sender's client certificate.
    ///
    /// This is the proof, and the only key consent is ever remembered under.
    /// `None` means the sender presented no certificate, so trust can never be
    /// remembered for it.
    pub fingerprint: Option<String>,
    /// Claimed device id. Matches discovery when the sender is also announcing.
    pub device_id: Option<String>,
    /// Claimed display name, or a plain placeholder when none was given.
    pub alias: String,
}

/// One thing inside an offer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfferedItem {
    /// Sender's opaque id, used again in [`Engine::answer_offer`].
    pub id: String,
    /// Relative name it would be saved under.
    pub name: String,
    /// Payload bytes.
    pub size: u64,
    /// File or inline text.
    pub kind: ItemKind,
}

impl OfferedItem {
    fn from_manifest(item: &ManifestItem) -> Self {
        Self {
            id: item.id.clone(),
            name: item.name.clone(),
            size: item_size(item),
            kind: item.kind,
        }
    }
}

/// How an incoming offer ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfferOutcome {
    /// The person accepted at least one item.
    Accepted,
    /// The person declined it, or answered with no items.
    Declined,
    /// Nobody answered before [`OFFER_TIMEOUT`].
    TimedOut,
}

/// Payload bytes an offered item represents, counting inline text.
fn item_size(item: &ManifestItem) -> u64 {
    match item.kind {
        ItemKind::Text => item.text.as_deref().map_or(0, |text| text.len() as u64),
        ItemKind::File => item.size,
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
    /// Another device wants to send something and is waiting for an answer.
    ///
    /// The sender is blocked in its prepare request until the person answers,
    /// the offer times out, or auto-accept answers it without a sheet. Call
    /// [`Engine::answer_offer`] with the ids that should be accepted.
    IncomingOffer {
        /// Which offer, for the answer.
        id: OfferId,
        /// Who is asking. The fingerprint, not the name, is the identity.
        sender: OfferSender,
        /// Everything the sender wants to send, in order.
        items: Vec<OfferedItem>,
        /// Total payload bytes.
        bytes: u64,
    },
    /// An incoming offer reached its final answer.
    OfferClosed {
        /// Which offer.
        id: OfferId,
        /// What happened.
        outcome: OfferOutcome,
    },
    /// A send was handed to the network.
    TransferStarted {
        /// Which send.
        id: TransferId,
        /// A short name for the whole send: the first file's name, or the
        /// folder the files came out of. Shown before any progress exists, so
        /// a lane can say what is moving instead of "preparing".
        label: String,
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
    /// A send moved forward, at most a few times a second.
    ///
    /// Sent only when something changed, so a stalled transfer is silent
    /// instead of emitting the same numbers.
    TransferProgress {
        /// Which send.
        id: TransferId,
        /// Bytes handed to the network, including bytes a resumed session
        /// already held.
        sent: u64,
        /// Bytes the send covers in total.
        total: u64,
        /// Files with a final result.
        files_done: usize,
        /// Files in the send.
        files_total: usize,
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
    /// Accept every incoming offer without asking.
    ///
    /// Installed before the receiver binds, so a device that sends the moment
    /// this app appears is answered by policy rather than waiting for a sheet
    /// that does not exist yet.
    pub auto_accept: bool,
    /// Fingerprints accepted on their own even when [`Self::auto_accept`] is off.
    pub trusted: Vec<String>,
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
            auto_accept: false,
            trusted: Vec::new(),
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
    /// The identity the receiver was started with. Sends present it as a client
    /// certificate so the other side can bind trust to this device.
    identity: Arc<Identity>,
    /// Offers, answers and the trust policy they share with the receiver.
    consent: Arc<Consent>,
    /// Fixed at start: discovery either came up, was disabled, or failed.
    discovery_state: DiscoveryState,
    events: broadcast::Sender<Event>,
    server: Mutex<Option<JoinHandle<()>>>,
    shutdown: Mutex<Option<oneshot::Sender<()>>>,
    discovery: Mutex<Option<Discovery>>,
    watch: Mutex<Option<JoinHandle<()>>>,
    transfers: Mutex<HashMap<TransferId, Running>>,
    next_transfer: AtomicU64,
}

/// What a person or policy decided about one offer.
enum OfferAnswer {
    Accept(Vec<String>),
    Decline,
}

/// One offer waiting for an answer.
struct PendingOffer {
    answer: oneshot::Sender<OfferAnswer>,
    fingerprint: Option<String>,
}

/// The engine's side of the consent conversation.
///
/// The server asks [`Consent::decide`] before opening a session; the UI answers
/// through [`Consent::answer`]. Policy is two things only: a global auto-accept
/// switch and a set of trusted fingerprints. A name never enters this type.
struct Consent {
    events: broadcast::Sender<Event>,
    trusted: StdMutex<HashSet<String>>,
    auto_accept: AtomicBool,
    pending: Mutex<HashMap<u64, PendingOffer>>,
    next: AtomicU64,
}

impl Consent {
    fn new(events: broadcast::Sender<Event>, auto_accept: bool, trusted: Vec<String>) -> Self {
        let consent = Self {
            events,
            trusted: StdMutex::new(HashSet::new()),
            auto_accept: AtomicBool::new(false),
            pending: Mutex::new(HashMap::new()),
            next: AtomicU64::new(1),
        };
        consent.set_policy(auto_accept, trusted);
        consent
    }

    /// Replace the policy with what the shell persisted.
    fn set_policy(&self, enabled: bool, trusted: Vec<String>) {
        self.auto_accept.store(enabled, Ordering::Relaxed);
        let mut known = self.trusted.lock().expect("poisoned trust lock");
        known.clear();
        for fingerprint in trusted {
            let fingerprint = fingerprint.trim().to_ascii_lowercase();
            if !fingerprint.is_empty() {
                known.insert(fingerprint);
            }
        }
        // Logged because "why was that accepted without asking?" is otherwise a
        // question only the person's memory can answer.
        log::debug!(
            "consent policy: auto-accept {enabled}, {} trusted device(s)",
            known.len()
        );
    }

    fn is_trusted(&self, fingerprint: Option<&str>) -> bool {
        fingerprint.is_some_and(|fingerprint| {
            self.trusted
                .lock()
                .expect("poisoned trust lock")
                .contains(fingerprint)
        })
    }

    fn auto_accept(&self) -> bool {
        self.auto_accept.load(Ordering::Relaxed)
    }

    fn trusted_devices(&self) -> Vec<String> {
        let mut trusted: Vec<String> = self
            .trusted
            .lock()
            .expect("poisoned trust lock")
            .iter()
            .cloned()
            .collect();
        trusted.sort();
        trusted
    }

    /// Decide an offer, emitting the event a UI draws when a person must answer.
    async fn decide(&self, offer: Offer) -> OfferDecision {
        let all = offer.all_ids();
        if self.auto_accept.load(Ordering::Relaxed) || self.is_trusted(offer.fingerprint.as_deref())
        {
            log::debug!(
                "accepted an offer from {:?} without asking (auto-accept, or trusted)",
                offer.fingerprint
            );
            return OfferDecision::Accept(all);
        }

        let number = self.next.fetch_add(1, Ordering::Relaxed);
        let id = OfferId(number);
        let (answer, receiver) = oneshot::channel();
        self.pending.lock().await.insert(
            number,
            PendingOffer {
                answer,
                fingerprint: offer.fingerprint.clone(),
            },
        );

        let alias = offer
            .alias
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .unwrap_or(UNNAMED_SENDER)
            .to_string();
        let _ = self.events.send(Event::IncomingOffer {
            id,
            sender: OfferSender {
                fingerprint: offer.fingerprint,
                device_id: offer.device_id,
                alias,
            },
            items: offer.items.iter().map(OfferedItem::from_manifest).collect(),
            bytes: offer.bytes,
        });

        let (outcome, decision) = match tokio::time::timeout(OFFER_TIMEOUT, receiver).await {
            Ok(Ok(OfferAnswer::Accept(ids))) if !ids.is_empty() => {
                (OfferOutcome::Accepted, OfferDecision::Accept(ids))
            }
            Ok(Ok(_)) | Ok(Err(_)) => (OfferOutcome::Declined, OfferDecision::Decline),
            Err(_) => {
                // Nobody answered. Take the entry out so a late answer cannot
                // claim to have settled an offer that already closed.
                self.pending.lock().await.remove(&number);
                (OfferOutcome::TimedOut, OfferDecision::NoAnswer)
            }
        };
        let _ = self.events.send(Event::OfferClosed { id, outcome });
        decision
    }

    /// Answer one offer. False when it already closed.
    async fn answer(&self, id: OfferId, accepted: Vec<String>, remember: bool) -> bool {
        let Some(PendingOffer {
            answer,
            fingerprint,
        }) = self.pending.lock().await.remove(&id.0)
        else {
            return false;
        };
        let accepted = if accepted.is_empty() {
            OfferAnswer::Decline
        } else {
            OfferAnswer::Accept(accepted)
        };
        // Only an accepted transfer makes a device trusted. Declining while the
        // box happened to be ticked must not quietly grant future access.
        let trusting = remember && matches!(accepted, OfferAnswer::Accept(_));
        // Trust is recorded only once the answer actually reached the waiting
        // offer. A tap that lands after the offer timed out must not leave a
        // device trusted behind a question nobody was still being asked: the
        // sheet reports "already closed", so the person never saw a transfer
        // that would justify it. Sending first leaves a window where the very
        // next offer from the same device can still ask, which is the safe
        // direction to be wrong in: a redundant sheet, never a silent one.
        if answer.send(accepted).is_err() {
            log::debug!("offer {id:?} closed before the answer arrived");
            return false;
        }
        if let Some(fingerprint) = fingerprint.filter(|_| trusting) {
            self.trusted
                .lock()
                .expect("poisoned trust lock")
                .insert(fingerprint);
            log::info!("trusted {fingerprint} after an accepted transfer");
        }
        true
    }
}

/// One send that has been handed to the network.
struct Running {
    handle: JoinHandle<()>,
    /// Shared with the send task: cancelling reads and writes these counters,
    /// so no message has to reach a blocked socket.
    progress: SendProgress,
}

/// What a send carries, fixed before it starts.
///
/// Two shapes rather than one optional field: a file send reads local paths
/// while it streams, and a text item is already in the manifest. Keeping them
/// apart means the task never has to ask "which kind is this" mid-flight.
enum Content {
    /// Local files, one session.
    Files(Vec<SendTarget>),
    /// One inline note, link or clipboard payload, saved under `name`.
    Text {
        /// The payload itself, UTF-8.
        text: String,
        /// File name the receiver should give it.
        name: String,
    },
}

impl Content {
    /// The lane label, item count and declared bytes, before any work.
    fn totals(&self) -> (String, usize, u64) {
        match self {
            Self::Files(targets) => (
                send_label(targets),
                targets.len(),
                targets
                    .iter()
                    .filter_map(|target| std::fs::metadata(&target.path).ok())
                    .map(|metadata| metadata.len())
                    .sum(),
            ),
            Self::Text { text, name } => (name.clone(), 1, text.len() as u64),
        }
    }
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

        // The event channel exists before the receiver, because the consent gate
        // installed below needs somewhere to announce an offer. The policy is
        // installed here too: an offer that arrives while the shell is still
        // reading its settings must not be lost.
        let (events, _) = broadcast::channel(EVENT_BUFFER);
        let consent = Arc::new(Consent::new(
            events.clone(),
            config.auto_accept,
            config.trusted,
        ));
        let gate: OfferGate = {
            let consent = Arc::clone(&consent);
            Arc::new(move |offer| {
                let consent = Arc::clone(&consent);
                Box::pin(async move { consent.decide(offer).await })
            })
        };
        server_config = server_config.with_offer_gate(gate);

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

        let mut discovery_state = DiscoveryState::Off;
        let discovery = match config.discovery {
            Some(mut discovery_config) => {
                // The announce must carry the port actually bound, which is
                // only known after the listener exists.
                discovery_config.announce = true;
                match Discovery::start(announce_for(&info), discovery_config).await {
                    Ok(discovery) => {
                        discovery_state = DiscoveryState::Running;
                        Some(discovery)
                    }
                    Err(err) => {
                        let reason = err.to_string();
                        log::warn!("discovery is unavailable: {reason}");
                        let _ = events.send(Event::DiscoveryUnavailable {
                            reason: reason.clone(),
                        });
                        discovery_state = DiscoveryState::Unavailable(reason);
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
                identity,
                consent,
                discovery_state,
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

    /// What multicast discovery is doing.
    #[must_use]
    pub fn discovery(&self) -> &DiscoveryState {
        &self.inner.discovery_state
    }

    /// Subscribe to events. Each subscriber gets its own backlog from this point.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.inner.events.subscribe()
    }

    /// Replace the consent policy with what the shell persisted.
    ///
    /// `enabled` accepts every offer without asking. `trusted` holds the
    /// fingerprints of devices that are accepted on their own even when it is
    /// off. A fingerprint is the identity proven by the client certificate, so
    /// this list is the only thing that survives a rename.
    pub fn set_auto_accept(&self, enabled: bool, trusted: Vec<String>) {
        self.inner.consent.set_policy(enabled, trusted);
    }

    /// Whether every offer is accepted without asking.
    #[must_use]
    pub fn auto_accept(&self) -> bool {
        self.inner.consent.auto_accept()
    }

    /// The fingerprints remembered as trusted senders, sorted.
    #[must_use]
    pub fn trusted_devices(&self) -> Vec<String> {
        self.inner.consent.trusted_devices()
    }

    /// Answer an offer announced by [`Event::IncomingOffer`].
    ///
    /// `accepted` carries the item ids that may be saved; pass every id to
    /// accept the whole transfer, or nothing to decline it. When `remember` is
    /// true the sender's fingerprint is added to the trusted list and must be
    /// persisted by the shell, so the next offer from the same device skips the
    /// sheet. Returns false when the offer already closed (timeout or a second
    /// answer), which a UI can ignore.
    pub async fn answer_offer(&self, id: OfferId, accepted: Vec<String>, remember: bool) -> bool {
        self.inner.consent.answer(id, accepted, remember).await
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

    /// Find one visible device by id.
    async fn peer(&self, device_id: &str) -> Result<PeerView, EngineError> {
        self.peers()
            .await
            .into_iter()
            .find(|peer| peer.device_id == device_id)
            .ok_or_else(|| EngineError::UnknownPeer(device_id.to_string()))
    }

    /// Send files to one device from the peer list.
    pub async fn send_to_peer(
        &self,
        device_id: &str,
        targets: Vec<SendTarget>,
    ) -> Result<TransferId, EngineError> {
        let peer = self.peer(device_id).await?;
        self.send_to(peer.addr, &peer.fingerprint, targets).await
    }

    /// Send one text item — a link, a note, the clipboard — to a device from
    /// the peer list.
    pub async fn send_text_to_peer(
        &self,
        device_id: &str,
        text: String,
        name: String,
    ) -> Result<TransferId, EngineError> {
        let peer = self.peer(device_id).await?;
        self.send_text_to(peer.addr, &peer.fingerprint, text, name)
            .await
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
        self.begin_send(addr, fingerprint, Content::Files(targets))
            .await
    }

    /// Send one inline text item to an address and a fingerprint.
    ///
    /// The same manual path as [`Engine::send_to`], for a sentence, a link or
    /// whatever was on the clipboard. The receiver saves it under `name`, so
    /// the caller should pass a name that reads well as a file
    /// (`clipboard.txt`), not a label.
    pub async fn send_text_to(
        &self,
        addr: SocketAddr,
        fingerprint: &str,
        text: String,
        name: String,
    ) -> Result<TransferId, EngineError> {
        if text.is_empty() || name.is_empty() {
            return Err(EngineError::NothingToSend);
        }
        self.begin_send(addr, fingerprint, Content::Text { text, name })
            .await
    }

    /// Hand one send to the network and return its id before it connects.
    async fn begin_send(
        &self,
        addr: SocketAddr,
        fingerprint: &str,
        content: Content,
    ) -> Result<TransferId, EngineError> {
        // Build the client here so a malformed fingerprint fails the call
        // instead of becoming an event nobody is watching yet.
        let mut client = Client::new(addr, fingerprint)?;
        // Present the device's identity as a client certificate, and announce
        // the same name discovery shows. The certificate is what lets the
        // receiver bind "always accept from this device" to something a rename
        // cannot change; the alias is only a label on the consent sheet.
        client = client
            .with_identity(Arc::clone(&self.inner.identity))?
            .with_alias(self.inner.info.alias.clone());
        if let Some(pin) = &self.inner.pin {
            client = client.with_pin(pin.clone());
        }

        let id = TransferId(self.inner.next_transfer.fetch_add(1, Ordering::Relaxed));
        let (label, files, bytes) = content.totals();
        let events = self.inner.events.clone();
        let progress = SendProgress::new();
        progress.set_totals(bytes, files);

        // Progress is read from the counters on a timer rather than pushed from
        // the sender: a UI that is not drawing must cost the wire nothing.
        let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let ticker = {
            let events = events.clone();
            let progress = progress.clone();
            let done = Arc::clone(&done);
            tokio::spawn(async move {
                let mut tick = tokio::time::interval(PROGRESS_TICK);
                tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                let mut last = (0u64, 0usize);
                loop {
                    tick.tick().await;
                    if done.load(Ordering::Relaxed) {
                        return;
                    }
                    let now = (progress.sent_bytes(), progress.files_done());
                    if now == last {
                        continue;
                    }
                    last = now;
                    let _ = events.send(Event::TransferProgress {
                        id,
                        sent: now.0,
                        total: progress.total_bytes(),
                        files_done: now.1,
                        files_total: progress.total_files(),
                    });
                }
            })
        };

        let sending = progress.clone();
        let handle = tokio::spawn(async move {
            let _ = events.send(Event::TransferStarted {
                id,
                label,
                files,
                bytes,
                peer: addr,
            });

            let mut connection = match client.connect().await {
                Ok(connection) => connection,
                Err(err) => {
                    done.store(true, Ordering::Relaxed);
                    ticker.abort();
                    let _ = events.send(Event::TransferFailed {
                        id,
                        reason: err.to_string(),
                    });
                    return;
                }
            };
            let result: Result<Vec<ItemOutcome>, ClientError> = match content {
                Content::Files(targets) => connection
                    .send_files_with(&targets, Some(&sending))
                    .await
                    .map(|outcomes| {
                        outcomes
                            .into_iter()
                            .map(ItemOutcome::from_file_outcome)
                            .collect()
                    }),
                Content::Text { text, name } => {
                    // Text travels inside the manifest, so nothing is streamed:
                    // the counter settles the moment the receiver has it.
                    let bytes = text.len() as u64;
                    let result = connection.send_text(&text, &name).await;
                    if result.is_ok() {
                        sending.add_bytes(bytes);
                        sending.item_done();
                    }
                    result.map(|saved| vec![ItemOutcome::from_text(&name, saved)])
                }
            };
            connection.close();

            // No progress may follow a terminal event, or a UI shows a bar
            // moving for a send it was already told had finished.
            done.store(true, Ordering::Relaxed);
            ticker.abort();

            match result {
                Ok(items) => {
                    let _ = events.send(Event::TransferFinished { id, items });
                }
                Err(ClientError::Cancelled) => {
                    let _ = events.send(Event::TransferCancelled { id });
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
        transfers.retain(|_, running| !running.handle.is_finished());
        transfers.insert(id, Running { handle, progress });
        Ok(id)
    }

    /// Stop a send that is still running.
    ///
    /// Returns `false` when the id is unknown or already finished. Cancelling is
    /// cooperative: the sender abandons the request at the next chunk, tells the
    /// receiver to drop the session, and reports [`Event::TransferCancelled`].
    /// Files the receiver already verified stay installed; nothing is left
    /// half-written.
    pub async fn cancel(&self, id: TransferId) -> bool {
        let mut transfers = self.inner.transfers.lock().await;
        transfers.retain(|_, running| !running.handle.is_finished());
        match transfers.get(&id) {
            Some(running) => {
                running.progress.cancel();
                true
            }
            None => false,
        }
    }

    /// Ids of sends that have not finished or been cancelled.
    pub async fn running(&self) -> Vec<TransferId> {
        let mut transfers = self.inner.transfers.lock().await;
        transfers.retain(|_, running| !running.handle.is_finished());
        let mut ids: Vec<TransferId> = transfers.keys().copied().collect();
        ids.sort_unstable();
        ids
    }

    /// Stop sending, stop discovering and close the listener.
    ///
    /// The engine is finished afterwards; start a new one to receive again.
    pub async fn stop(&self) {
        for (_, running) in self.inner.transfers.lock().await.drain() {
            // Shutting down is not a user cancel: there is no window left to
            // send a `DELETE`, so the receiver sweeps the session.
            running.handle.abort();
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A tap that lands after the question closed must not leave the device
    /// trusted behind it. The sheet says "already closed", so no transfer the
    /// person actually agreed to justifies remembering them.
    #[tokio::test]
    async fn a_late_accept_does_not_grant_trust() {
        let (events, _keep) = broadcast::channel(8);
        let consent = Consent::new(events, false, Vec::new());
        let (answer, receiver) = oneshot::channel();
        // The offer timed out while the person was tapping: the waiting side is
        // gone, exactly as if `decide` had given up on the channel.
        drop(receiver);
        consent.pending.lock().await.insert(
            7,
            PendingOffer {
                answer,
                fingerprint: Some("a65cd186".to_string()),
            },
        );

        assert!(
            !consent
                .answer(OfferId(7), vec!["i1".to_string()], true)
                .await,
            "an answer nobody is waiting for is not an answer"
        );
        assert!(
            consent.trusted_devices().is_empty(),
            "trust needs a transfer that actually happened"
        );
    }
}
