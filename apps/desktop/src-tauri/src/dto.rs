//! The JSON shape between this shell and the window.
//!
//! Every field here is mirrored in `apps/desktop/src/lib/types.ts`. The two
//! files are a contract: a rename on one side is a compile error (or a silent
//! `undefined`) on the other, so they change together.
//!
//! Nothing in this module decides anything. It translates engine types into
//! the strings and numbers a webview can hold, and the only judgement it makes
//! is how to spell an enum.

use flit_api::DeviceType;
use flit_core::{DiscoveryState, ItemOutcome, PeerSource, PeerView};
use flit_crypto::Badge;
use serde::Serialize;

/// The four pictures and four words a person compares between screens.
#[derive(Debug, Clone, Serialize)]
pub struct BadgeDto {
    /// Four emoji, no separator.
    pub emoji: String,
    /// Four hyphen-separated words.
    pub words: String,
}

impl From<&Badge> for BadgeDto {
    fn from(badge: &Badge) -> Self {
        Self {
            emoji: badge.emoji.clone(),
            words: badge.words.clone(),
        }
    }
}

/// One device the engine can see.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Peer {
    /// Stable identifier from the announce.
    pub device_id: String,
    /// Display name. Cosmetic and never trusted.
    pub alias: String,
    /// Key fingerprint to pin.
    pub fingerprint: String,
    /// Cosmetic device class.
    pub device_type: &'static str,
    /// Where the control plane answers, as `ip:port`.
    pub addr: String,
    /// How this device was learned about.
    pub source: &'static str,
    /// The safety badge, when the fingerprint is a real SHA-256.
    pub badge: Option<BadgeDto>,
}

impl From<&PeerView> for Peer {
    fn from(view: &PeerView) -> Self {
        Self {
            device_id: view.device_id.clone(),
            alias: view.alias.clone(),
            fingerprint: view.fingerprint.clone(),
            device_type: device_type(view.device_type),
            addr: view.addr.to_string(),
            source: source(view.source),
            badge: view.badge.as_ref().map(BadgeDto::from),
        }
    }
}

/// The wire spelling of [`DeviceType`], matching the TypeScript union.
fn device_type(kind: DeviceType) -> &'static str {
    match kind {
        DeviceType::Desktop => "desktop",
        DeviceType::Laptop => "laptop",
        DeviceType::Phone => "phone",
        DeviceType::Tablet => "tablet",
        DeviceType::Headless => "headless",
    }
}

/// The wire spelling of [`PeerSource`], matching the TypeScript union.
fn source(source: PeerSource) -> &'static str {
    match source {
        PeerSource::Multicast => "multicast",
        PeerSource::Scan => "scan",
        PeerSource::Manual => "manual",
    }
}

/// What multicast discovery is doing.
///
/// Tagged on `state`, because the TypeScript side is a union of three literal
/// objects rather than a string with an optional reason.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum Discovery {
    /// Announcing and listening.
    Running,
    /// Started without discovery.
    Off,
    /// Could not start. Sending and receiving still work.
    Unavailable {
        /// Why, in words a support issue can quote.
        reason: String,
    },
}

impl From<&DiscoveryState> for Discovery {
    fn from(state: &DiscoveryState) -> Self {
        match state {
            DiscoveryState::Running => Self::Running,
            DiscoveryState::Off => Self::Off,
            DiscoveryState::Unavailable(reason) => Self::Unavailable {
                reason: reason.clone(),
            },
        }
    }
}

/// This device, as the window says it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// Name other devices see.
    pub alias: String,
    /// Fingerprint they pin.
    pub fingerprint: String,
    /// Four emoji and four words derived from the fingerprint.
    pub badge: Option<BadgeDto>,
    /// Address a peer can reach, as `ip:port`.
    pub addr: String,
    /// Folder received files land in.
    pub root: String,
    /// Wire protocol version.
    pub proto: u16,
    /// Application version.
    pub version: String,
    /// What discovery is doing right now.
    pub discovery: Discovery,
}

/// One file's result inside a finished transfer.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    /// Local file that was offered.
    pub path: String,
    /// Name the receiver was asked to save it as.
    pub name: String,
    /// Where it landed, after conflict resolution.
    pub saved_as: Option<String>,
    /// Bytes the receiver verified.
    pub bytes: u64,
    /// Why it failed, in the receiver's words.
    pub error: Option<String>,
}

impl From<&ItemOutcome> for Item {
    fn from(outcome: &ItemOutcome) -> Self {
        Self {
            path: outcome.path.to_string_lossy().into_owned(),
            name: outcome.name.clone(),
            saved_as: outcome.saved_as.clone(),
            bytes: outcome.bytes,
            error: outcome.error.clone(),
        }
    }
}

/// One past send, as the history file stores it.
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct HistoryEntry {
    /// When the send started, in milliseconds since the Unix epoch.
    pub at: u64,
    /// Alias of the device it went to.
    pub peer: String,
    /// Its badge, if it had one.
    pub emoji: Option<String>,
    /// Fingerprint that was pinned.
    pub fingerprint: String,
    /// Address it went to, as `ip:port`.
    pub addr: String,
    /// Files the send covered.
    pub files: usize,
    /// Bytes the receiver verified.
    pub bytes: u64,
    /// How long it took, in milliseconds.
    pub ms: u64,
    /// True when every item was accepted.
    pub ok: bool,
    /// Why it failed, or the first item the receiver refused.
    pub error: Option<String>,
    /// What the user chose, kept so a past send can be repeated.
    pub paths: Vec<String>,
}

/// One row of the network doctor.
#[derive(Debug, Clone, Serialize)]
pub struct Check {
    /// Stable identifier, so the window can order or hide rows.
    pub id: &'static str,
    /// What is being checked, in a person's words.
    pub name: &'static str,
    /// `ok`, `bad`, `wait` or `unknown`.
    pub state: &'static str,
    /// The result in one line.
    pub detail: String,
    /// A second line, when the result needs one.
    pub sub: Option<String>,
    /// A copyable value, when there is one.
    pub value: Option<String>,
}

/// One network interface, as the doctor lists it.
#[derive(Debug, Clone, Serialize)]
pub struct NetInterface {
    /// Operating-system name.
    pub name: String,
    /// Address in CIDR form, e.g. `192.168.1.18/24`.
    pub addr: String,
    /// True for the loopback address.
    pub loopback: bool,
    /// True when this build believes the interface can carry traffic.
    pub up: bool,
}

/// Everything the network screen shows.
#[derive(Debug, Clone, Serialize)]
pub struct Doctor {
    /// The checks, in the order a person should read them.
    pub checks: Vec<Check>,
    /// The machine's addresses.
    pub interfaces: Vec<NetInterface>,
}

/// One thing in the tray: what a dropped path expands to.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrayEntry {
    /// Path the user dropped or picked.
    pub path: String,
    /// The name the row shows.
    pub name: String,
    /// True when the path is a folder.
    pub is_dir: bool,
    /// How many files it covers.
    pub files: usize,
    /// How many bytes those files hold.
    pub bytes: u64,
}
