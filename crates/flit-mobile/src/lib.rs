//! The Android surface of the engine.
//!
//! Everything below is deliberately small: start, who am I, who is nearby,
//! rename, stop. The protocol, the identity and the listener stay in
//! `flit-core` and `flit-crypto`; this crate only moves plain values across the
//! JNI boundary. A UI that has to know about manifests is a UI that will
//! disagree with the desktop one.

use std::{
    net::{IpAddr, SocketAddr},
    path::PathBuf,
    sync::Arc,
};

use flit_api::DeviceType;
use flit_core::{DiscoveryState, Engine, EngineConfig};
use flit_crypto::{IdentityFile, NameFile};

uniffi::setup_scaffolding!();

/// What can stop the phone from joining the network.
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
    #[uniffi::constructor]
    pub fn start(data_dir: String) -> Result<Arc<Self>, MobileError> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(|err| MobileError::start(format!("no network runtime: {err}")))?;

        let dir = PathBuf::from(data_dir);
        std::fs::create_dir_all(&dir)
            .map_err(|err| MobileError::start(format!("cannot use the app folder: {err}")))?;

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

/// The address another device should dial, not the `0.0.0.0` the listener binds.
fn reachable(bound: SocketAddr) -> String {
    let ip = primary_ipv4().unwrap_or_else(|| bound.ip());
    SocketAddr::new(ip, bound.port()).to_string()
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

fn source_name(source: flit_discovery::PeerSource) -> &'static str {
    match source {
        flit_discovery::PeerSource::Multicast => "multicast",
        flit_discovery::PeerSource::Scan => "scan",
        flit_discovery::PeerSource::Manual => "manual",
    }
}
