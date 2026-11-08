//! Wire types for the Flit local-network protocol.
//!
//! This crate is deliberately UI- and transport-agnostic: it describes what
//! crosses the wire, not how it gets there. The normative description lives in
//! `docs/protocol.md`; when the two disagree, that document wins and this crate
//! is the bug.
//!
//! Compatibility rules:
//!
//! * `PROTO_VERSION` is the major version. It is carried in `X-Flit-Proto` and
//!   echoed by `/api/v1/info`. A peer that does not understand it must refuse
//!   politely rather than half-work.
//! * Unknown JSON fields are ignored on deserialization, and new fields are
//!   always optional, so a newer minor peer can talk to an older one.
//! * Adding a field is not a version bump. Changing the meaning of an existing
//!   field, or removing one, is.

use serde::{Deserialize, Serialize};

/// Major protocol version, advertised in `X-Flit-Proto` and `/api/v1/info`.
pub const PROTO_VERSION: u16 = 1;

/// Path prefix for every control endpoint.
pub const API_PREFIX: &str = "/api/v1";

/// UDP port for discovery announcements. See `docs/protocol.md` §2.
pub const DISCOVERY_PORT: u16 = 47_820;

/// TCP port for the TLS control and data plane.
pub const TLS_PORT: u16 = 47_821;

/// TCP port for a browser receive session. Bound only while a session is open.
pub const BROWSER_PORT: u16 = 47_822;

/// IPv4 multicast group used for discovery announcements.
pub const MULTICAST_GROUP: [u8; 4] = [239, 255, 42, 99];

/// Default share alias shown to other devices.
pub const DEFAULT_ALIAS: &str = "Flit";

/// Files at or below this size are eligible for the pack endpoint.
pub const PACK_THRESHOLD: u64 = 256 * 1024;

/// Upper bound on items accepted in a single prepare request.
pub const MAX_ITEMS: usize = 100_000;

/// Upper bound on the streamed manifest body, in bytes.
pub const MAX_MANIFEST_BYTES: usize = 64 * 1024 * 1024;

/// Upper bound on a single declared item size (1 TiB).
pub const MAX_FILE_BYTES: u64 = 1 << 40;

/// Upper bound on the declared total of one session (8 TiB).
pub const MAX_TOTAL_BYTES: u64 = 8 << 40;

/// Capability tokens carried in `caps[]`. Peers must ignore tokens they do not
/// know; the absence of a token is the only signal that matters.
pub mod caps {
    /// Resumable uploads via `Content-Range`.
    pub const RESUME: &str = "resume";
    /// Batched small-file uploads.
    pub const PACK: &str = "pack";
    /// Post-write verification against a declared SHA-256.
    pub const VERIFY: &str = "verify";
    /// Text and clipboard items.
    pub const TEXT: &str = "text";
    /// Live progress over server-sent events.
    pub const PROGRESS_SSE: &str = "progress-sse";
}

/// What kind of device is advertising itself. Purely cosmetic in the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DeviceType {
    /// A desktop machine.
    #[default]
    Desktop,
    /// A portable computer.
    Laptop,
    /// A phone.
    Phone,
    /// A tablet.
    Tablet,
    /// A server or headless box.
    Headless,
}

/// What an item in a manifest represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    /// A file whose bytes follow in a `PUT` or a pack body.
    #[default]
    File,
    /// A text or link item carried inline in the manifest.
    Text,
}

/// The response body of `GET /api/v1/info`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Info {
    /// Major protocol version.
    pub proto: u16,
    /// Human-readable name, chosen by the user. Not unique, not trusted.
    pub alias: String,
    /// Stable device identifier: the first 8 bytes of the public key hash.
    pub device_id: String,
    /// Full SHA-256 of the Ed25519 public key, lowercase hex.
    pub fingerprint: String,
    /// Cosmetic device class.
    #[serde(default)]
    pub device_type: DeviceType,
    /// The port actually bound, which is not always [`TLS_PORT`].
    pub port: u16,
    /// Application version string, e.g. `0.1.0`.
    pub version: String,
    /// Capability tokens.
    #[serde(default)]
    pub caps: Vec<String>,
    /// Whether this device currently demands a PIN for a new peer.
    #[serde(default)]
    pub pin_required: bool,
}

impl Info {
    /// Returns true when this peer can talk to a peer advertising `peer`.
    ///
    /// Major versions must match exactly. Minor drift is carried by
    /// capabilities, never by guessing.
    #[must_use]
    pub fn accepts_peer(&self, peer: u16) -> bool {
        peer == self.proto
    }

    /// True when the peer advertises every capability in `required`.
    #[must_use]
    pub fn has_caps(&self, required: &[&str]) -> bool {
        required.iter().all(|r| self.caps.iter().any(|c| c == r))
    }
}

/// A discovery announcement, sent to the multicast group every two seconds.
///
/// Deliberately a smaller payload than [`Info`]: it carries nothing that a
/// passive listener on the LAN should not learn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Announce {
    /// Major protocol version.
    pub proto: u16,
    /// Display name.
    pub alias: String,
    /// Truncated public key hash.
    pub device_id: String,
    /// Full public key fingerprint, used for TLS pinning.
    pub fingerprint: String,
    /// Cosmetic device class.
    #[serde(default)]
    pub device_type: DeviceType,
    /// Port the TLS listener is bound to.
    pub port: u16,
    /// Capability tokens.
    #[serde(default)]
    pub caps: Vec<String>,
    /// Whether a PIN is required for a new peer.
    #[serde(default)]
    pub pin_required: bool,
}

/// Machine-readable error code. UI text is derived from these, never from
/// `message`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// The request was malformed or failed validation.
    BadRequest,
    /// The peer is not authenticated for this session.
    Unauthorized,
    /// A PIN is required before the session may continue.
    PinRequired,
    /// The supplied PIN was wrong.
    PinIncorrect,
    /// The session or item does not exist.
    NotFound,
    /// The item collides with existing state.
    Conflict,
    /// The declared size exceeds a limit.
    TooLarge,
    /// The receiver ran out of space.
    DiskFull,
    /// The transfer was cancelled by one side.
    Cancelled,
    /// A required capability or version is missing.
    Unsupported,
    /// Something failed that the peer cannot act on.
    Internal,
}

impl ErrorCode {
    /// Stable wire string for this code.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BadRequest => "bad_request",
            Self::Unauthorized => "unauthorized",
            Self::PinRequired => "pin_required",
            Self::PinIncorrect => "pin_incorrect",
            Self::NotFound => "not_found",
            Self::Conflict => "conflict",
            Self::TooLarge => "too_large",
            Self::DiskFull => "disk_full",
            Self::Cancelled => "cancelled",
            Self::Unsupported => "unsupported",
            Self::Internal => "internal",
        }
    }
}

impl std::fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Uniform error body returned by every endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiErrorBody {
    /// Machine-readable code.
    pub code: ErrorCode,
    /// Developer-facing detail. Never shown to a user verbatim.
    pub message: String,
    /// Whether retrying the same request could succeed.
    #[serde(default)]
    pub retryable: bool,
}

impl ApiErrorBody {
    /// Build an error body.
    #[must_use]
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            retryable: false,
        }
    }

    /// Mark this error as retryable.
    #[must_use]
    pub fn retryable(mut self) -> Self {
        self.retryable = true;
        self
    }
}

/// One entry in a prepare manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestItem {
    /// Sender-chosen opaque identifier, unique within the session.
    pub id: String,
    /// Relative path as chosen by the sender. Untrusted: sanitize on arrival.
    pub name: String,
    /// Exact byte length for files; zero for text items.
    #[serde(default)]
    pub size: u64,
    /// Lowercase hex SHA-256 of the file, or of the UTF-8 text payload.
    #[serde(default)]
    pub sha256: String,
    /// Modification time, seconds since the Unix epoch.
    #[serde(default)]
    pub mtime: Option<i64>,
    /// File or inline text.
    #[serde(default)]
    pub kind: ItemKind,
    /// Inline payload for text items. Absent for files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// Body of `POST /api/v1/upload`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrepareRequest {
    /// Items the sender wants to deliver.
    pub items: Vec<ManifestItem>,
    /// Sender's own sum of item sizes. Advisory, used to fail fast on disk.
    #[serde(default)]
    pub total_bytes: u64,
}

/// Why a single item was rejected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RejectedItem {
    /// The `id` from the manifest.
    pub id: String,
    /// Why it was rejected.
    pub reason: ErrorCode,
}

/// Body of a successful `POST /api/v1/upload`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionResponse {
    /// Opaque session identifier, used in every later path segment.
    pub session_id: String,
    /// Items the receiver will accept.
    pub accepted: Vec<String>,
    /// Items the receiver refused, with a reason.
    #[serde(default)]
    pub rejected: Vec<RejectedItem>,
    /// Bytes already committed for a resumed session. Zero for a new one.
    #[serde(default)]
    pub offset: u64,
}

/// Body of `POST /api/v1/upload/{sid}/finish`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FinishRequest {
    /// Items the sender believes are fully uploaded.
    #[serde(default)]
    pub item_ids: Vec<String>,
}

/// Result of one item's verification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemResult {
    /// The manifest `id`.
    pub id: String,
    /// True when the receiver's hash matched the manifest.
    pub verified: bool,
    /// Final on-disk name after conflict resolution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saved_as: Option<String>,
}

/// Body of a successful `POST /api/v1/upload/{sid}/finish`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FinishResponse {
    /// Per-item verification results.
    pub items: Vec<ItemResult>,
}

/// Body of `POST /api/v1/text`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextRequest {
    /// The text or link itself.
    pub text: String,
    /// Optional clipboard annotation.
    #[serde(default)]
    pub label: Option<String>,
}

/// A single progress event on the SSE stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProgressEvent {
    /// Item identifier, or `None` for a session-level event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item: Option<String>,
    /// Bytes committed for that item.
    pub bytes: u64,
    /// Total expected bytes.
    pub total: u64,
    /// True when the session has finished, successfully or not.
    #[serde(default)]
    pub done: bool,
}

/// A validation failure in a manifest, before any I/O happens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestError {
    /// The manifest declared nothing.
    Empty,
    /// More items than the receiver will accept.
    TooManyItems {
        /// How many were declared.
        count: usize,
        /// The cap.
        max: usize,
    },
    /// Two items shared an identifier, or an identifier was empty.
    BadId(String),
    /// An item name was empty or otherwise unusable.
    BadName(String),
    /// A single item exceeded [`MAX_FILE_BYTES`].
    ItemTooLarge {
        /// Offending item id.
        id: String,
        /// Declared size.
        size: u64,
    },
    /// The declared total exceeded [`MAX_TOTAL_BYTES`].
    TotalTooLarge {
        /// Declared total.
        total: u64,
    },
    /// A text item carried no payload, or a file item carried one.
    BadKind(String),
}

impl std::fmt::Display for ManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => f.write_str("manifest is empty"),
            Self::TooManyItems { count, max } => {
                write!(f, "manifest declares {count} items, limit is {max}")
            }
            Self::BadId(id) => write!(f, "invalid or duplicate item id: {id:?}"),
            Self::BadName(name) => write!(f, "invalid item name: {name:?}"),
            Self::ItemTooLarge { id, size } => {
                write!(
                    f,
                    "item {id:?} declares {size} bytes, over the per-item limit"
                )
            }
            Self::TotalTooLarge { total } => {
                write!(f, "manifest declares {total} bytes, over the session limit")
            }
            Self::BadKind(id) => {
                write!(f, "item {id:?} has a payload that does not match its kind")
            }
        }
    }
}

impl std::error::Error for ManifestError {}

/// Validate a manifest against the limits in this crate.
///
/// This runs before any I/O: a hostile manifest must be rejected on its
/// declared shape, not discovered while writing files.
pub fn validate_manifest(items: &[ManifestItem]) -> Result<(), ManifestError> {
    if items.is_empty() {
        return Err(ManifestError::Empty);
    }
    if items.len() > MAX_ITEMS {
        return Err(ManifestError::TooManyItems {
            count: items.len(),
            max: MAX_ITEMS,
        });
    }

    let mut seen: Vec<&str> = Vec::with_capacity(items.len());
    let mut total: u64 = 0;
    for item in items {
        if item.id.is_empty() {
            return Err(ManifestError::BadId(String::new()));
        }
        if seen.contains(&item.id.as_str()) {
            return Err(ManifestError::BadId(item.id.clone()));
        }
        seen.push(&item.id);

        if item.name.is_empty() {
            return Err(ManifestError::BadName(item.name.clone()));
        }
        if item.size > MAX_FILE_BYTES {
            return Err(ManifestError::ItemTooLarge {
                id: item.id.clone(),
                size: item.size,
            });
        }
        match item.kind {
            ItemKind::File if item.text.is_some() => {
                return Err(ManifestError::BadKind(item.id.clone()));
            }
            ItemKind::Text if item.text.is_none() => {
                return Err(ManifestError::BadKind(item.id.clone()));
            }
            _ => {}
        }

        total = total.saturating_add(item.size);
        if total > MAX_TOTAL_BYTES {
            return Err(ManifestError::TotalTooLarge { total });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(id: &str, name: &str, size: u64) -> ManifestItem {
        ManifestItem {
            id: id.into(),
            name: name.into(),
            size,
            sha256: "00".repeat(32),
            mtime: Some(1_700_000_000),
            kind: ItemKind::File,
            text: None,
        }
    }

    #[test]
    fn info_round_trips_as_json() {
        let info = Info {
            proto: PROTO_VERSION,
            alias: "Desk".into(),
            device_id: "0123456789abcdef".into(),
            fingerprint: "ab".repeat(32),
            device_type: DeviceType::Laptop,
            port: TLS_PORT,
            version: "0.1.0".into(),
            caps: vec![caps::RESUME.into(), caps::PACK.into()],
            pin_required: false,
        };
        let json = serde_json::to_string(&info).unwrap();
        let back: Info = serde_json::from_str(&json).unwrap();
        assert_eq!(info, back);
    }

    #[test]
    fn unknown_fields_are_ignored_for_forward_compat() {
        let json = r#"{"proto":1,"alias":"Desk","device_id":"d","fingerprint":"f",
            "port":47821,"version":"0.1.0","something_new":{"a":1}}"#;
        let info: Info = serde_json::from_str(json).unwrap();
        assert_eq!(info.alias, "Desk");
        assert_eq!(info.device_type, DeviceType::Desktop);
        assert!(!info.pin_required);
    }

    #[test]
    fn accepts_only_matching_major_version() {
        let info = Info {
            proto: PROTO_VERSION,
            alias: String::new(),
            device_id: String::new(),
            fingerprint: String::new(),
            device_type: DeviceType::Desktop,
            port: 0,
            version: String::new(),
            caps: vec![caps::RESUME.into()],
            pin_required: false,
        };
        assert!(info.accepts_peer(1));
        assert!(!info.accepts_peer(2));
        assert!(info.has_caps(&[caps::RESUME]));
        assert!(!info.has_caps(&[caps::PACK]));
    }

    #[test]
    fn manifest_rejects_empty() {
        assert_eq!(validate_manifest(&[]), Err(ManifestError::Empty));
    }

    #[test]
    fn manifest_rejects_duplicate_ids() {
        let items = vec![file("a", "a.txt", 1), file("a", "b.txt", 1)];
        assert_eq!(
            validate_manifest(&items),
            Err(ManifestError::BadId("a".into()))
        );
    }

    #[test]
    fn manifest_rejects_oversized_item() {
        let items = vec![file("a", "huge.bin", MAX_FILE_BYTES + 1)];
        assert!(matches!(
            validate_manifest(&items),
            Err(ManifestError::ItemTooLarge { .. })
        ));
    }

    #[test]
    fn manifest_rejects_text_item_without_payload() {
        let mut item = file("a", "note.txt", 0);
        item.kind = ItemKind::Text;
        assert_eq!(
            validate_manifest(&[item]),
            Err(ManifestError::BadKind("a".into()))
        );
    }

    #[test]
    fn manifest_accepts_a_normal_batch() {
        let items = vec![file("a", "a.txt", 10), file("b", "dir/b.bin", 20)];
        assert_eq!(validate_manifest(&items), Ok(()));
    }

    #[test]
    fn error_codes_have_stable_strings() {
        assert_eq!(ErrorCode::PinIncorrect.as_str(), "pin_incorrect");
        assert_eq!(ErrorCode::PinIncorrect.to_string(), "pin_incorrect");
    }
}
