//! The Android surface of the engine.
//!
//! Everything below is deliberately small: start, who am I, who is nearby,
//! rename, stop. The protocol, the identity and the listener stay in
//! `flit-core` and `flit-crypto`; this crate only moves plain values across the
//! JNI boundary. A UI that has to know about manifests is a UI that will
//! disagree with the desktop one.

use std::{
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
    sync::Arc,
};

use flit_api::{DeviceType, ItemKind};
use flit_core::{
    DiscoveryState, Engine, EngineConfig, Event, OfferId, OfferOutcome, ShareTarget, TransferId,
    collect_targets,
};
use flit_crypto::{IdentityFile, NameFile};
use tokio::sync::broadcast::error::RecvError;

uniffi::setup_scaffolding!();

/// What stopped a start, a rename or a send.
///
/// One variant, one sentence: the UI's job is to show it, not to branch on it.
/// `flat_error` keeps the Kotlin side a plain exception whose message is the
/// sentence — the alternative, a field-carrying variant, generates
/// `"message=…"` debug text and a property that collides with `Throwable`.
#[derive(Debug, thiserror::Error, uniffi::Error)]
#[uniffi(flat_error)]
pub enum MobileError {
    #[error("{reason}")]
    Start { reason: String },
}

impl MobileError {
    fn start(err: impl std::fmt::Display) -> Self {
        Self::Start {
            reason: err.to_string(),
        }
    }
}

/// One step of a send, flattened for Kotlin.
///
/// A record instead of a variant per event: the UI draws one row with a label,
/// a bar and a button, and a flat shape keeps that row from branching on six
/// generated classes.
#[derive(Debug, Clone, uniffi::Record)]
pub struct TransferEvent {
    /// `started`, `progress`, `finished`, `failed` or `cancelled`.
    pub kind: String,
    /// Which send, as returned by [`MobileEngine::send_files`].
    pub id: u64,
    /// A short name for the whole send.
    pub label: String,
    /// Why the send failed, or the first file the receiver refused, when
    /// `kind` is `failed` or `finished`. Empty otherwise.
    pub detail: String,
    pub bytes: u64,
    pub sent: u64,
    pub files: u32,
    pub files_done: u32,
}

/// A file or folder that landed in the receive folder.
#[derive(Debug, Clone, uniffi::Record)]
pub struct ReceivedFile {
    pub name: String,
    /// Bytes on disk; for a folder, the sum of its files.
    pub bytes: u64,
    /// Seconds since the Unix epoch, for sorting.
    pub modified: u64,
    pub folder: bool,
}

/// What the app is told about a send while it happens.
///
/// Called on a background thread, never on the main one: a UI must post to its
/// own loop before touching state.
#[uniffi::export(callback_interface)]
pub trait TransferListener: Send + Sync {
    fn on_event(&self, event: TransferEvent);
}

/// One item inside an incoming offer.
#[derive(Debug, Clone, uniffi::Record)]
pub struct OfferItem {
    /// Sender's opaque id; pass it back to accept this item alone.
    pub id: String,
    /// Name it would be saved under, relative to the receive folder.
    pub name: String,
    /// Payload bytes, counting inline text.
    pub size: u64,
    /// `file` or `text`.
    pub kind: String,
}

/// A device asking to send something and waiting for an answer.
#[derive(Debug, Clone, uniffi::Record)]
pub struct OfferEvent {
    /// Which offer, for [`MobileEngine::answer_offer`].
    pub id: u64,
    /// Key proven by the sender's certificate. Empty when it presented none,
    /// in which case `remember` cannot work: there is nothing to remember.
    pub fingerprint: String,
    /// Claimed device id, empty when none was announced.
    pub device_id: String,
    /// Claimed name, or a neutral sentence when the sender did not give one.
    pub alias: String,
    /// Bytes the whole offer covers.
    pub bytes: u64,
    /// Everything the sender wants to send.
    pub items: Vec<OfferItem>,
}

/// What the app is told when an offer closes without an answer.
///
/// Called on a background thread, like [`TransferListener`].
#[uniffi::export(callback_interface)]
pub trait OfferListener: Send + Sync {
    /// Someone is waiting: show the sheet and answer it, or it times out.
    fn on_offer(&self, offer: OfferEvent);
    /// The offer is settled: `accepted`, `declined` or `timed_out`.
    ///
    /// A sheet still on screen must close. An answer sent after this returns
    /// false and changes nothing.
    fn on_offer_closed(&self, id: u64, outcome: String);
}

/// This phone, as another device sees it.
#[derive(Debug, Clone, uniffi::Record)]
pub struct DeviceInfo {
    /// The label other devices show. Not proof of anything: the fingerprint is.
    pub alias: String,
    pub device_id: String,
    pub fingerprint: String,
    /// The address another device should dial, `ip:port`.
    pub addr: String,
    /// `running`, `off`, or `unavailable`.
    pub discovery: String,
}

/// A device that has announced itself lately.
#[derive(Debug, Clone, uniffi::Record)]
pub struct PeerInfo {
    pub device_id: String,
    /// The label it chose. Two devices may pick the same one.
    pub alias: String,
    /// The key it must prove it holds before anything is sent.
    pub fingerprint: String,
    pub addr: String,
    pub device_type: String,
    /// `multicast`, `scan` or `manual`.
    pub source: String,
}

/// A browser share a person can read out to another device.
///
/// No token field: the address carries it, and splitting the two in the UI only
/// invites showing a URL that is missing half of what makes it work.
#[derive(Debug, Clone, uniffi::Record)]
pub struct ShareInfo {
    /// The address to type into a browser.
    pub url: String,
    /// How many files the page lists.
    pub files: u32,
    /// Whole minutes left before the address stops working, rounded up.
    pub minutes: u32,
}

/// The engine, held open by the app for as long as the screen lives.
#[derive(uniffi::Object)]
pub struct MobileEngine {
    /// Kept for the engine's lifetime: dropping it would abort every task the
    /// receiver and discovery spawned.
    runtime: tokio::runtime::Runtime,
    engine: Engine,
    /// Where the identity and the name live, so a rename outlives a restart.
    store: IdentityFile,
    device: DeviceInfo,
}

#[uniffi::export]
impl MobileEngine {
    /// Start receiving and announcing.
    ///
    /// `data_dir` is the app's private files directory. The identity, the
    /// display name and the received folder live under it, so the phone needs
    /// no storage permission to receive. On first run the name is two random
    /// words; after that it is whatever the person chose.
    ///
    /// `auto_accept` and `trusted` are the consent policy the shell persisted.
    /// They are installed before the receiver binds, so a device that sends the
    /// moment this phone appears is answered instead of waiting for a sheet
    /// that has not been drawn yet.
    #[uniffi::constructor]
    pub fn start(
        data_dir: String,
        auto_accept: bool,
        trusted: Vec<String>,
    ) -> Result<Arc<Self>, MobileError> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(|err| MobileError::start(format!("no network runtime: {err}")))?;

        let dir = PathBuf::from(data_dir);
        std::fs::create_dir_all(&dir)
            .map_err(|err| MobileError::start(format!("cannot use the app folder: {err}")))?;
        install_logger(&dir);

        let store = IdentityFile::new(dir.join("identity.key"));
        let identity = store
            .load_or_create()
            .map_err(|err| MobileError::start(format!("cannot keep an identity: {err}")))?;
        let fingerprint = identity.fingerprint();
        let name = NameFile::beside(&store)
            .load_or_create()
            .map_err(|err| MobileError::start(format!("cannot keep a name: {err}")))?;

        let config = EngineConfig {
            alias: name,
            device_type: DeviceType::Phone,
            root: dir.join("received"),
            auto_accept,
            trusted,
            ..EngineConfig::default()
        };
        let engine = runtime
            .block_on(Engine::start(Arc::new(identity), config))
            .map_err(|err| MobileError::start(format!("cannot start the receiver: {err}")))?;

        let info = engine.info();
        let device = DeviceInfo {
            alias: info.alias.clone(),
            device_id: info.device_id.clone(),
            fingerprint,
            addr: reachable(engine.addr()),
            discovery: discovery_name(engine.discovery()).to_string(),
        };

        Ok(Arc::new(Self {
            runtime,
            engine,
            store,
            device,
        }))
    }

    /// This phone: the name is a label, the fingerprint is the identity.
    pub fn device(&self) -> DeviceInfo {
        let mut device = self.device.clone();
        // Discovery can change under a running engine; the screen should show
        // the state now, not the state at launch.
        device.discovery = discovery_name(self.engine.discovery()).to_string();
        device
    }

    /// Everyone who has announced themselves lately.
    ///
    /// Sync so the UI can call it from a coroutine on any dispatcher. A scan
    /// result and a multicast result are the same row; `source` says which.
    pub fn peers(&self) -> Vec<PeerInfo> {
        self.runtime
            .block_on(self.engine.peers())
            .into_iter()
            .map(|peer| PeerInfo {
                device_id: peer.device_id,
                alias: peer.alias,
                fingerprint: peer.fingerprint,
                addr: peer.addr.to_string(),
                device_type: device_type_name(peer.device_type).to_string(),
                source: source_name(peer.source).to_string(),
            })
            .collect()
    }

    /// Listen for send progress until the engine stops.
    ///
    /// One listener per engine: the app attaches its callback once and keeps
    /// the object alive for as long as it wants events.
    pub fn watch(&self, listener: Box<dyn TransferListener>) {
        let mut events = self.engine.subscribe();
        self.runtime.spawn(async move {
            loop {
                let event = match events.recv().await {
                    Ok(event) => event,
                    // A slow listener skipped some ticks; the next one carries
                    // absolute numbers, so nothing is lost that a bar needs.
                    Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => return,
                };
                if let Some(event) = transfer_event(event) {
                    listener.on_event(event);
                }
            }
        });
    }

    /// Listen for incoming offers until the engine stops.
    ///
    /// Separate from [`MobileEngine::watch`] because an offer is a question the
    /// person must answer, not a progress tick: the app shows a sheet and calls
    /// [`MobileEngine::answer_offer`]. Leaving it unanswered for a minute
    /// declines the transfer and tells the sender.
    pub fn watch_offers(&self, listener: Box<dyn OfferListener>) {
        let mut events = self.engine.subscribe();
        self.runtime.spawn(async move {
            loop {
                let event = match events.recv().await {
                    Ok(event) => event,
                    Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => return,
                };
                match event {
                    Event::IncomingOffer {
                        id,
                        sender,
                        items,
                        bytes,
                    } => listener.on_offer(OfferEvent {
                        id: id.as_u64(),
                        fingerprint: sender.fingerprint.unwrap_or_default(),
                        device_id: sender.device_id.unwrap_or_default(),
                        alias: sender.alias,
                        bytes,
                        items: items
                            .into_iter()
                            .map(|item| OfferItem {
                                id: item.id,
                                name: item.name,
                                size: item.size,
                                kind: item_kind_name(item.kind).to_string(),
                            })
                            .collect(),
                    }),
                    Event::OfferClosed { id, outcome } => {
                        listener.on_offer_closed(id.as_u64(), outcome_name(outcome).to_string());
                    }
                    _ => {}
                }
            }
        });
    }

    /// Answer an offer announced by [`OfferListener::on_offer`].
    ///
    /// `accepted` carries the item ids that may be saved; pass every id for the
    /// whole transfer, or nothing to decline. `remember` adds the sender's
    /// fingerprint to the trusted list, which the app must persist; a sender
    /// that presented no certificate cannot be remembered. Returns false when
    /// the offer already closed, which the UI can ignore.
    pub fn answer_offer(&self, id: u64, accepted: Vec<String>, remember: bool) -> bool {
        self.runtime.block_on(
            self.engine
                .answer_offer(OfferId::from_raw(id), accepted, remember),
        )
    }

    /// Replace the consent policy. The app persists both halves.
    ///
    /// `enabled` accepts every offer without asking; `trusted` lists the
    /// fingerprints accepted on their own even when it is off. Called once at
    /// startup with what was saved, and again whenever the person changes it.
    pub fn set_auto_accept(&self, enabled: bool, trusted: Vec<String>) {
        self.engine.set_auto_accept(enabled, trusted);
    }

    /// Whether every offer is accepted without asking.
    pub fn auto_accept(&self) -> bool {
        self.engine.auto_accept()
    }

    /// The fingerprints remembered as trusted senders, sorted.
    pub fn trusted_devices(&self) -> Vec<String> {
        self.engine.trusted_devices()
    }

    /// Send files to a device from the peer list, and return the send's id.
    ///
    /// `paths` are real filesystem paths. The app copies picked `content://`
    /// URIs into its cache first, because the engine streams from files and has
    /// no business holding a content resolver.
    pub fn send_files(&self, device_id: String, paths: Vec<String>) -> Result<u64, MobileError> {
        let peer = self
            .runtime
            .block_on(self.engine.peers())
            .into_iter()
            .find(|peer| peer.device_id == device_id)
            .ok_or_else(|| MobileError::start("that device is no longer on this network"))?;

        self.start_send(peer.addr, &peer.fingerprint, paths)
    }

    /// Send files to an address typed by the person, not a discovered peer.
    ///
    /// The fingerprint is still required: an address is a location, never an
    /// identity. This is the phone's version of the CLI's `--to`, and the only
    /// way to reach a device whose announcement never arrived.
    pub fn send_files_to(
        &self,
        addr: String,
        fingerprint: String,
        paths: Vec<String>,
    ) -> Result<u64, MobileError> {
        let addr = parse_addr(&addr)?;
        self.start_send(addr, &fingerprint, paths)
    }

    /// Stop a send. False when it had already ended.
    pub fn cancel(&self, id: u64) -> bool {
        let transfer = self
            .runtime
            .block_on(self.engine.running())
            .into_iter()
            .find(|transfer| transfer.as_u64() == id);

        match transfer {
            Some(transfer) => self.runtime.block_on(self.engine.cancel(transfer)),
            None => false,
        }
    }

    /// What has landed in the receive folder, newest first.
    pub fn received(&self) -> Vec<ReceivedFile> {
        list_received(self.engine.root())
    }

    /// Offer these files to a browser on the same network.
    ///
    /// The phone serves; the other computer only reads. Nothing is uploaded to
    /// this phone, and nothing is installed on the computer. The share ends on
    /// its own, on [`MobileEngine::close_share`], or when the browser says it
    /// is done.
    pub fn open_share(&self, paths: Vec<String>) -> Result<ShareInfo, MobileError> {
        let paths: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
        self.runtime
            .block_on(self.engine.open_share(paths))
            .map(share_info)
            .map_err(MobileError::start)
    }

    /// Stop the browser share, if one is open.
    pub fn close_share(&self) {
        self.runtime.block_on(self.engine.close_share());
    }

    /// The share a browser could open right now, if any.
    ///
    /// Call this when the screen comes back: a share that timed out while the
    /// app was in the background must not still be shown with an address.
    pub fn share(&self) -> Option<ShareInfo> {
        self.runtime.block_on(self.engine.share()).map(share_info)
    }

    /// Change the name other devices see, and return the cleaned form kept.
    ///
    /// The announce carries the name the receiver was started with, so the app
    /// has to call [`MobileEngine::stop`] and start a new engine for the other
    /// screen to see it. Same rule as the desktop shell, which restarts.
    pub fn rename(&self, name: String) -> Result<String, MobileError> {
        NameFile::beside(&self.store)
            .save(&name)
            .map_err(|err| MobileError::start(err.to_string()))
    }

    /// Stop listening. The object stays readable; it cannot start again.
    pub fn stop(&self) {
        self.runtime.block_on(self.engine.stop());
    }
}

/// Small helpers the exported impl shares. Not part of the FFI surface: a
/// `SocketAddr` is not one of the types UniFFI can carry.
impl MobileEngine {
    /// Turn paths into targets and start a send to `addr`.
    fn start_send(
        &self,
        addr: SocketAddr,
        fingerprint: &str,
        paths: Vec<String>,
    ) -> Result<u64, MobileError> {
        let paths: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
        let collected = collect_targets(&paths, None).map_err(MobileError::start)?;
        if collected.targets.is_empty() {
            return Err(MobileError::start("there is nothing to send"));
        }

        let id = self
            .runtime
            .block_on(self.engine.send_to(addr, fingerprint, collected.targets))
            .map_err(MobileError::start)?;
        Ok(id.as_u64())
    }
}

/// Where the engine's own log lines go, set once per process.
static LOG_PATH: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// The app's log file, appended to by the `log` crate.
///
/// Android sends a process's stdout nowhere, so the only way to see why a
/// receive was refused on a phone is to write the lines down in a file the
/// shell can read with `run-as`. The engine is already full of useful
/// `warn!`/`debug!` calls; this gives them somewhere to land.
struct FileLogger;

impl log::Log for FileLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Debug
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let Some(path) = LOG_PATH.get() else {
            return;
        };
        let seconds = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs());
        let line = format!("{seconds} {:<5} {}\n", record.level(), record.args());
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .and_then(|mut file| std::io::Write::write_all(&mut file, line.as_bytes()));
    }

    fn flush(&self) {}
}

static LOGGER: FileLogger = FileLogger;

/// Cap the log so a long-lived app cannot fill the data directory.
const MAX_LOG_BYTES: u64 = 512 * 1024;

/// Point the `log` crate at `dir/flit.log`. Safe to call more than once.
fn install_logger(dir: &Path) {
    let path = dir.join("flit.log");
    if std::fs::metadata(&path).is_ok_and(|meta| meta.len() > MAX_LOG_BYTES) {
        let _ = std::fs::remove_file(&path);
    }
    if LOG_PATH.set(path).is_ok() {
        let _ = log::set_logger(&LOGGER);
        log::set_max_level(log::LevelFilter::Debug);
    }
}

/// Turn an engine event into the flat record Kotlin draws, or `None` for
/// events the transfer row has no use for.
fn transfer_event(event: Event) -> Option<TransferEvent> {
    let blank = |kind: &str, id: TransferId| TransferEvent {
        kind: kind.to_string(),
        id: id.as_u64(),
        label: String::new(),
        detail: String::new(),
        bytes: 0,
        sent: 0,
        files: 0,
        files_done: 0,
    };

    match event {
        Event::TransferStarted {
            id,
            label,
            files,
            bytes,
            ..
        } => Some(TransferEvent {
            label,
            files: files as u32,
            bytes,
            ..blank("started", id)
        }),
        Event::TransferProgress {
            id,
            sent,
            total,
            files_done,
            files_total,
        } => Some(TransferEvent {
            bytes: total,
            sent,
            files: files_total as u32,
            files_done: files_done as u32,
            ..blank("progress", id)
        }),
        Event::TransferFinished { id, items } => {
            let failed = items.iter().find(|item| item.error.is_some());
            Some(TransferEvent {
                detail: failed
                    .and_then(|item| item.error.clone())
                    .unwrap_or_default(),
                sent: items.iter().map(|item| item.bytes).sum(),
                files: items.len() as u32,
                files_done: items.iter().filter(|item| item.error.is_none()).count() as u32,
                ..blank("finished", id)
            })
        }
        Event::TransferFailed { id, reason } => Some(TransferEvent {
            detail: reason,
            ..blank("failed", id)
        }),
        Event::TransferCancelled { id } => Some(blank("cancelled", id)),
        Event::PeerSeen(_)
        | Event::PeerLost { .. }
        | Event::DiscoveryUnavailable { .. }
        | Event::IncomingOffer { .. }
        | Event::OfferClosed { .. } => None,
    }
}

/// The receive folder, newest first. Partial files are not shown: a `.part`
/// with no name beside it is work in progress, not a file that arrived.
fn list_received(root: &Path) -> Vec<ReceivedFile> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };

    let mut files: Vec<ReceivedFile> = entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            // `.flit` is the engine's own staging area and `.part` files are
            // work in progress; neither is something that arrived.
            if name.starts_with('.') || name.ends_with(".part") {
                return None;
            }
            let metadata = entry.metadata().ok()?;
            let folder = metadata.is_dir();
            Some(ReceivedFile {
                bytes: if folder {
                    folder_size(&entry.path())
                } else {
                    metadata.len()
                },
                modified: modified_secs(&metadata),
                name,
                folder,
            })
        })
        .collect();

    files.sort_by(|a, b| {
        b.modified
            .cmp(&a.modified)
            .then_with(|| a.name.cmp(&b.name))
    });
    files
}

/// Bytes under `path`, summed. A folder the app cannot read counts as zero
/// rather than failing the whole list.
fn folder_size(path: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    entries
        .filter_map(|entry| entry.ok())
        .map(|entry| {
            entry.metadata().map_or(0, |metadata| {
                if metadata.is_dir() {
                    folder_size(&entry.path())
                } else {
                    metadata.len()
                }
            })
        })
        .sum()
}

fn modified_secs(metadata: &std::fs::Metadata) -> u64 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// A browser share, flattened for Kotlin.
///
/// Minutes, not a deadline: the shell draws "about 8 minutes left", and a
/// countdown that ticks once a second would need the engine to be polled for a
/// number it cannot know exactly anyway.
fn share_info(target: ShareTarget) -> ShareInfo {
    let seconds = target.expires_in.as_secs();
    ShareInfo {
        url: target.url,
        files: u32::try_from(target.files).unwrap_or(u32::MAX),
        minutes: u32::try_from(seconds.div_ceil(60)).unwrap_or(u32::MAX),
    }
}

/// The address another device should dial, not the `0.0.0.0` the listener binds.
fn reachable(bound: SocketAddr) -> String {
    let ip = primary_ipv4().unwrap_or_else(|| bound.ip());
    SocketAddr::new(ip, bound.port()).to_string()
}

/// Read a typed address, filling in the control port when it was left out.
///
/// People copy an address off the other screen, where it is written with a
/// port; people also type just the IP. Both should work, and a typo should say
/// what an address looks like instead of "invalid socket address".
fn parse_addr(raw: &str) -> Result<SocketAddr, MobileError> {
    let text = raw.trim();
    if let Ok(addr) = text.parse::<SocketAddr>() {
        return Ok(addr);
    }
    if let Ok(ip) = text.parse::<IpAddr>() {
        return Ok(SocketAddr::new(ip, flit_api::TLS_PORT));
    }
    Err(MobileError::start(format!(
        "{text} is not an address like 192.168.1.20:47821"
    )))
}

/// The machine's routable IPv4 address, if it has one.
fn primary_ipv4() -> Option<IpAddr> {
    let mut candidates: Vec<IpAddr> = if_addrs::get_if_addrs()
        .ok()?
        .into_iter()
        .map(|iface| iface.ip())
        .filter(|ip| ip.is_ipv4() && !ip.is_loopback())
        .collect();
    candidates.sort();
    candidates.first().copied()
}

fn discovery_name(state: &DiscoveryState) -> &'static str {
    match state {
        DiscoveryState::Running => "running",
        DiscoveryState::Off => "off",
        DiscoveryState::Unavailable(_) => "unavailable",
    }
}

fn device_type_name(kind: DeviceType) -> &'static str {
    match kind {
        DeviceType::Desktop => "desktop",
        DeviceType::Laptop => "laptop",
        DeviceType::Phone => "phone",
        DeviceType::Tablet => "tablet",
        DeviceType::Headless => "headless",
    }
}

fn item_kind_name(kind: ItemKind) -> &'static str {
    match kind {
        ItemKind::File => "file",
        ItemKind::Text => "text",
    }
}

fn outcome_name(outcome: OfferOutcome) -> &'static str {
    match outcome {
        OfferOutcome::Accepted => "accepted",
        OfferOutcome::Declined => "declined",
        OfferOutcome::TimedOut => "timed_out",
    }
}

fn source_name(source: flit_discovery::PeerSource) -> &'static str {
    match source {
        flit_discovery::PeerSource::Multicast => "multicast",
        flit_discovery::PeerSource::Scan => "scan",
        flit_discovery::PeerSource::Manual => "manual",
    }
}
