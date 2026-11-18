//! Sessions: everything the receiver knows about one upload.
//!
//! A session is created by a streamed manifest, receives bytes one item at a
//! time, and only then commits. Nothing in here trusts the sender:
//!
//! * names go through `flit_storage` before a path is built;
//! * `.part` files are named by index, never by the sender's id, so a hostile
//!   id cannot influence a filesystem path;
//! * a commit renames only after the whole-file hash matches;
//! * a name collision becomes `report (2).pdf`, never an overwrite.

use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{ErrorKind, Write},
    net::IpAddr,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};

use flit_api::{
    ErrorCode, FinishResponse, ItemKind, ItemResult, MAX_FILE_BYTES, MAX_ITEMS, MAX_TOTAL_BYTES,
    ManifestItem, RejectedItem, SessionResponse,
};
use flit_storage::{conflict_name, safe_join, sanitize_relative};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

use crate::consent::{Offer, OfferContext, OfferDecision, OfferGate};

/// Directory under the destination root that holds in-flight data.
const WORK_DIR: &str = ".flit";

/// How long an abandoned session may keep its `.part` files.
const STALE_AFTER: Duration = Duration::from_secs(60 * 60);

/// How long a receiver stays busy after opening a session.
///
/// One transfer at a time keeps the consent question unambiguous: a second
/// sender arriving mid-transfer would otherwise have to wait behind the same
/// screen, and a person cannot meaningfully answer two offers at once.
pub const ACTIVE_WINDOW: Duration = Duration::from_secs(60);

/// Prepare attempts allowed from one address per [`ADMISSION_WINDOW`].
pub const PREPARE_ATTEMPTS: usize = 12;

/// The window the attempt counter covers.
pub const ADMISSION_WINDOW: Duration = Duration::from_secs(60);

/// How many distinct addresses the limiter remembers before it prunes.
///
/// A receiver that has been up for months meets a lot of addresses; past this
/// point it sweeps the expired ones instead of turning into a log of everyone
/// who ever knocked.
const ADDRESS_MEMORY: usize = 256;

/// One item's in-flight state.
pub struct Item {
    /// Sender's opaque id.
    pub id: String,
    /// Relative name as it will be saved.
    pub name: String,
    /// Declared length in bytes.
    pub size: u64,
    /// Declared lowercase hex SHA-256.
    pub sha256: String,
    /// File or inline text.
    pub kind: ItemKind,
    /// Inline payload for text items.
    pub text: Option<String>,
    /// Where bytes accumulate until commit.
    pub part: PathBuf,
    /// Where the item will be renamed on success.
    pub target: PathBuf,
    /// Bytes written so far.
    pub received: u64,
    /// Running hash of everything written so far.
    pub hasher: Sha256,
    /// True once committed.
    pub done: bool,
}

impl Item {
    /// True when every declared byte has arrived.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.received == self.size
    }

    /// Drop the partial bytes and start this item over.
    ///
    /// Used when a hash did not match and when a pack entry replaces a partial
    /// upload; the sender always resends from zero in both cases.
    pub(crate) fn reset(&mut self) -> std::io::Result<()> {
        match fs::remove_file(&self.part) {
            Ok(()) => {}
            Err(err) if err.kind() == ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
        self.received = 0;
        self.hasher = Sha256::new();
        Ok(())
    }
}

/// What happened when one item was committed.
#[derive(Debug)]
pub enum Commit {
    /// Hash matched and the file is in place.
    Verified {
        /// Path relative to the destination root.
        saved_as: String,
    },
    /// The bytes arrived but hashed differently from the manifest.
    Mismatch,
    /// Not every byte arrived yet.
    Incomplete,
    /// No such item, or it was already committed.
    NotFound,
}

/// One upload session.
pub struct Session {
    /// Opaque identifier used in request paths.
    pub id: String,
    /// Digest of the manifest that opened this session.
    ///
    /// A restarted sender presents the same manifest again; matching this key
    /// lets the receiver hand back the same session instead of starting over.
    pub key: String,
    /// Destination root chosen by the user.
    pub root: PathBuf,
    /// Where `.part` files live.
    pub dir: PathBuf,
    /// Items by id.
    pub items: HashMap<String, Item>,
    /// Fingerprint of the device that opened the session.
    ///
    /// `None` means the sender presented no client certificate. Every later
    /// request must present the same value, so a session id alone is not a
    /// capability another device can borrow.
    pub owner: Option<String>,
    /// Address the session was opened from.
    ///
    /// Only consulted when there is no fingerprint: two anonymous senders are
    /// otherwise indistinguishable, and the address is the single fact that
    /// says "the same machine came back" rather than "someone else did".
    pub peer: IpAddr,
    /// Set when either side cancels; in-flight uploads must stop.
    pub cancelled: bool,
    /// When this session was created, for sweeping.
    pub created: SystemTime,
}

impl Session {
    /// Create the working directory for a new session.
    fn create(
        root: &Path,
        id: String,
        key: String,
        owner: Option<String>,
        peer: IpAddr,
    ) -> std::io::Result<Self> {
        let dir = root.join(WORK_DIR).join("parts").join(&id);
        fs::create_dir_all(&dir)?;
        Ok(Self {
            id,
            key,
            root: root.to_path_buf(),
            dir,
            items: HashMap::new(),
            owner,
            peer,
            cancelled: false,
            created: SystemTime::now(),
        })
    }

    /// Whether `fingerprint` is allowed to keep using this session.
    ///
    /// Both sides being `None` matches: a receiver that accepts a sender
    /// without a certificate cannot later demand one for the same session.
    #[must_use]
    pub fn owned_by(&self, fingerprint: Option<&str>) -> bool {
        self.owner.as_deref() == fingerprint
    }

    /// Validate one manifest item and reserve its paths.
    ///
    /// Rejection is per item: one unusable name does not fail the session.
    fn accept(&mut self, item: ManifestItem, index: usize) -> Result<(), ErrorCode> {
        if item.id.is_empty() || self.items.contains_key(&item.id) {
            return Err(ErrorCode::BadRequest);
        }
        if sanitize_relative(&item.name).is_err() {
            return Err(ErrorCode::BadRequest);
        }
        if item.size > MAX_FILE_BYTES {
            return Err(ErrorCode::TooLarge);
        }
        if !is_sha256(&item.sha256) {
            return Err(ErrorCode::BadRequest);
        }

        let target = safe_join(&self.root, &item.name).map_err(|_| ErrorCode::BadRequest)?;

        let (size, text) = match item.kind {
            ItemKind::File => (item.size, None),
            ItemKind::Text => {
                let Some(text) = item.text.clone() else {
                    return Err(ErrorCode::BadRequest);
                };
                (text.len() as u64, Some(text))
            }
        };
        if size > MAX_FILE_BYTES {
            return Err(ErrorCode::TooLarge);
        }

        self.items.insert(
            item.id.clone(),
            Item {
                part: self.dir.join(format!("{index}.part")),
                id: item.id,
                name: item.name,
                size,
                sha256: item.sha256.to_ascii_lowercase(),
                kind: item.kind,
                text,
                target,
                received: 0,
                hasher: Sha256::new(),
                done: false,
            },
        );
        Ok(())
    }

    /// Verify and move one item into place.
    ///
    /// Text items are materialized here rather than through `PUT`, because
    /// their payload already arrived in the manifest.
    pub fn commit(&mut self, id: &str) -> Commit {
        let Some(mut item) = self.items.remove(id) else {
            return Commit::NotFound;
        };
        if item.done {
            self.items.insert(item.id.clone(), item);
            return Commit::NotFound;
        }

        // Text items get their bytes written now; files were streamed.
        if item.kind == ItemKind::Text {
            let Some(text) = item.text.as_deref() else {
                self.items.insert(item.id.clone(), item);
                return Commit::Mismatch;
            };
            let written = fs::File::create(&item.part).and_then(|mut file| {
                file.write_all(text.as_bytes())?;
                file.sync_all()
            });
            if let Err(err) = written {
                log::warn!("could not write {}: {err}", item.name);
                self.items.insert(item.id.clone(), item);
                return Commit::Incomplete;
            }
            item.hasher.update(text.as_bytes());
            item.received = text.len() as u64;
        }

        if !item.is_complete() {
            log::warn!(
                "{} is incomplete: {} of {} bytes",
                item.name,
                item.received,
                item.size
            );
            self.items.insert(item.id.clone(), item);
            return Commit::Incomplete;
        }

        let digest = hex::encode(item.hasher.clone().finalize());
        if digest != item.sha256 {
            log::warn!(
                "{} failed verification: got {digest}, expected {}",
                item.name,
                item.sha256
            );
            // The sender can re-send this item from zero; nothing is kept.
            let _ = item.reset();
            self.items.insert(item.id.clone(), item);
            return Commit::Mismatch;
        }

        match self.install(&item) {
            Ok(saved_as) => {
                item.done = true;
                self.items.insert(item.id.clone(), item);
                Commit::Verified { saved_as }
            }
            Err(err) => {
                log::warn!("could not commit {}: {err}", item.name);
                self.items.insert(item.id.clone(), item);
                Commit::Incomplete
            }
        }
    }

    /// Link the finished `.part` into place under a non-colliding name.
    ///
    /// `hard_link` is used because it fails atomically when the target exists;
    /// `rename` would silently replace on Unix. Filesystems without hard links
    /// fall back to a checked rename, which is safe here because one session
    /// holds the lock while committing.
    fn install(&self, item: &Item) -> std::io::Result<String> {
        let attempts = 10_000u32;
        let parent = item.target.parent().unwrap_or(&self.root);
        let stem = item
            .target
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| std::io::Error::other("item has no usable file name"))?;

        // A folder send offers nested names such as `photos/day1/a.jpg`.
        // Create the directories it needs, but only inside the root: the path
        // was already sanitized and joined when the item was accepted.
        if parent != self.root {
            fs::create_dir_all(parent)?;
        }

        for attempt in 1..=attempts {
            let candidate = if attempt == 1 {
                item.target.clone()
            } else {
                parent.join(conflict_name(stem, attempt))
            };

            match fs::hard_link(&item.part, &candidate) {
                Ok(()) => {
                    fs::remove_file(&item.part).ok();
                    return Ok(self.relative(&candidate));
                }
                Err(err) if err.kind() == ErrorKind::AlreadyExists => continue,
                Err(err) if is_link_unsupported(&err) => {
                    if candidate.exists() {
                        continue;
                    }
                    fs::rename(&item.part, &candidate)?;
                    return Ok(self.relative(&candidate));
                }
                Err(err) => return Err(err),
            }
        }
        Err(std::io::Error::other("too many name collisions"))
    }

    /// Render a path the way a user sees it, relative to the destination root.
    fn relative(&self, path: &Path) -> String {
        path.strip_prefix(&self.root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    }

    /// Remove the working directory. Called after every terminal outcome.
    fn cleanup(&self) {
        if let Err(err) = fs::remove_dir_all(&self.dir) {
            if err.kind() != ErrorKind::NotFound {
                log::warn!("could not remove {}: {err}", self.dir.display());
            }
        }
    }
}

/// All live sessions of one server, plus the admission state that decides
/// whether a new offer may even be put to the person.
pub struct Sessions {
    root: PathBuf,
    map: Mutex<HashMap<String, Arc<Mutex<Session>>>>,
    admission: std::sync::Mutex<Admission>,
}

/// Attempts per address and the window one open session claims.
#[derive(Default)]
struct Admission {
    recent: HashMap<IpAddr, Vec<Instant>>,
    busy_until: Option<Instant>,
}

impl Sessions {
    /// Prepare the destination root and its work directory.
    pub fn new(root: impl Into<PathBuf>) -> std::io::Result<Self> {
        let root = root.into();
        fs::create_dir_all(root.join(WORK_DIR).join("parts"))?;
        Ok(Self {
            root,
            map: Mutex::new(HashMap::new()),
            admission: std::sync::Mutex::new(Admission::default()),
        })
    }

    /// Count one prepare attempt from `peer`, refusing the ones over the limit.
    ///
    /// Counted before the offer is shown, so a sender that spams prepare cannot
    /// turn the consent sheet into a denial-of-service against the person.
    fn record_attempt(&self, peer: IpAddr) -> Result<(), ErrorCode> {
        let mut admission = self.admission.lock().expect("poisoned admission lock");
        let now = Instant::now();
        if admission.recent.len() >= ADDRESS_MEMORY {
            admission.recent.retain(|_, times| {
                times.retain(|at| now.duration_since(*at) < ADMISSION_WINDOW);
                !times.is_empty()
            });
        }
        let recent = admission.recent.entry(peer).or_default();
        recent.retain(|at| now.duration_since(*at) < ADMISSION_WINDOW);
        if recent.len() >= PREPARE_ATTEMPTS {
            return Err(ErrorCode::RateLimited);
        }
        recent.push(now);
        Ok(())
    }

    /// Whether a recently opened session still owns the receiver.
    fn busy(&self) -> bool {
        let admission = self.admission.lock().expect("poisoned admission lock");
        admission
            .busy_until
            .is_some_and(|until| Instant::now() < until)
    }

    /// Claim the receiver for the next [`ACTIVE_WINDOW`].
    fn claim(&self) {
        let mut admission = self.admission.lock().expect("poisoned admission lock");
        admission.busy_until = Some(Instant::now() + ACTIVE_WINDOW);
    }

    /// Release the claim when the session is finished or cancelled.
    fn release(&self) {
        let mut admission = self.admission.lock().expect("poisoned admission lock");
        admission.busy_until = None;
    }

    /// The destination root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Validate a manifest and open a session for the acceptable items.
    ///
    /// Returns the response the sender gets, or a whole-request error when the
    /// manifest itself is unusable (empty, too many items, too many bytes).
    pub async fn begin(
        &self,
        mut manifest: Vec<ManifestItem>,
        context: OfferContext,
        gate: Option<&OfferGate>,
    ) -> Result<(Arc<Mutex<Session>>, SessionResponse), ErrorCode> {
        if manifest.is_empty() {
            return Err(ErrorCode::BadRequest);
        }
        if manifest.len() > MAX_ITEMS {
            return Err(ErrorCode::TooLarge);
        }
        let declared: u64 = manifest.iter().map(|item| item.size).sum();
        if declared > MAX_TOTAL_BYTES {
            return Err(ErrorCode::TooLarge);
        }

        self.record_attempt(context.peer)?;
        self.sweep().await;

        // A sender that was killed and re-ran the same command sends the same
        // manifest. Hand back the open session so its `.part` bytes are not
        // thrown away; the first `PUT` resumes from what the receiver has.
        // Adoption is checked before the busy gate: resuming the session that
        // owns the receiver is not a second transfer.
        let key = manifest_key(&manifest);
        if let Some(adopted) = self
            .adopt(
                &key,
                &manifest,
                context.fingerprint.as_deref(),
                context.peer,
            )
            .await
        {
            log::debug!("adopted an open session for the same manifest");
            // Resuming is still the one transfer the receiver is doing, so the
            // window restarts with it.
            self.claim();
            return Ok(adopted);
        }

        if self.busy() {
            return Err(ErrorCode::Busy);
        }

        // Claim the receiver before asking, not after: the question itself is
        // the transfer slot, so a second device cannot put a second sheet on
        // the same screen while this one is unanswered.
        self.claim();

        let bytes = manifest.iter().map(item_bytes).sum();
        let decision = match gate {
            Some(gate) => {
                gate(Offer {
                    peer: context.peer,
                    fingerprint: context.fingerprint.clone(),
                    device_id: context.device_id,
                    alias: context.alias,
                    items: manifest.clone(),
                    bytes,
                })
                .await
            }
            // A receiver without a shell to ask accepts everything. The engine
            // always installs a gate; this is the CLI's and tests' behaviour.
            None => OfferDecision::Accept(manifest.iter().map(|item| item.id.clone()).collect()),
        };
        manifest = match decision {
            OfferDecision::Accept(ids) => {
                let keep: HashSet<String> = ids.into_iter().collect();
                manifest
                    .into_iter()
                    .filter(|item| keep.contains(&item.id))
                    .collect()
            }
            OfferDecision::Decline => {
                self.release();
                return Err(ErrorCode::Declined);
            }
            OfferDecision::NoAnswer => {
                self.release();
                return Err(ErrorCode::NoAnswer);
            }
        };
        if manifest.is_empty() {
            self.release();
            return Err(ErrorCode::Declined);
        }

        let id = new_session_id();
        log::debug!("opening session {id} for {} items", manifest.len());
        let mut session = match Session::create(
            &self.root,
            id.clone(),
            key,
            context.fingerprint.clone(),
            context.peer,
        ) {
            Ok(session) => session,
            Err(_) => {
                self.release();
                return Err(ErrorCode::Internal);
            }
        };
        let mut accepted = Vec::new();
        let mut rejected = Vec::new();
        for (index, item) in manifest.into_iter().enumerate() {
            let id = item.id.clone();
            match session.accept(item, index) {
                Ok(()) => accepted.push(id),
                Err(reason) => rejected.push(RejectedItem { id, reason }),
            }
        }

        if accepted.is_empty() {
            session.cleanup();
            self.release();
            return Err(ErrorCode::BadRequest);
        }

        let response = SessionResponse {
            session_id: id.clone(),
            accepted,
            rejected,
            offset: 0,
            resume: HashMap::new(),
        };
        let session = Arc::new(Mutex::new(session));
        self.map.lock().await.insert(id, Arc::clone(&session));
        Ok((session, response))
    }

    /// Look up a session by id.
    pub async fn get(&self, id: &str) -> Option<Arc<Mutex<Session>>> {
        self.map.lock().await.get(id).cloned()
    }

    /// Find an open session with the same manifest and hand it back.
    ///
    /// Only sessions with nothing committed are eligible: a session that has
    /// installed files is either finishing or finished, and reusing it would
    /// report those items as failures to the new sender. Anything still open
    /// is reused as-is, including its partial bytes.
    async fn adopt(
        &self,
        key: &str,
        manifest: &[ManifestItem],
        fingerprint: Option<&str>,
        peer: IpAddr,
    ) -> Option<(Arc<Mutex<Session>>, SessionResponse)> {
        let map = self.map.lock().await;
        for session in map.values() {
            let Ok(guard) = session.try_lock() else {
                continue;
            };
            // A session id is a capability, but only for the device that opened
            // it: a second sender with the same manifest must not inherit
            // someone else's half-written bytes.
            if !same_sender(guard.owner.as_deref(), guard.peer, fingerprint, peer) {
                continue;
            }
            if guard.cancelled || guard.key != key || guard.items.values().any(|item| item.done) {
                continue;
            }
            let accepted: Vec<String> = manifest
                .iter()
                .filter(|item| guard.items.contains_key(&item.id))
                .map(|item| item.id.clone())
                .collect();
            let rejected: Vec<RejectedItem> = manifest
                .iter()
                .filter(|item| !guard.items.contains_key(&item.id))
                .map(|item| RejectedItem {
                    id: item.id.clone(),
                    reason: ErrorCode::BadRequest,
                })
                .collect();
            if accepted.is_empty() {
                continue;
            }
            let resume = guard
                .items
                .iter()
                .filter(|(_, item)| item.received > 0)
                .map(|(id, item)| (id.clone(), item.received))
                .collect();
            let response = SessionResponse {
                session_id: guard.id.clone(),
                accepted,
                rejected,
                offset: 0,
                resume,
            };
            let session = Arc::clone(session);
            return Some((session, response));
        }
        None
    }

    /// Forget a session. In-flight uploads notice via `cancelled`.
    pub async fn take(&self, id: &str) -> Option<Arc<Mutex<Session>>> {
        let session = self.map.lock().await.remove(id);
        if let Some(session) = &session {
            let mut guard = session.lock().await;
            guard.cancelled = true;
            guard.cleanup();
            // The receiver becomes free the moment the transfer ends, so
            // sending a second batch does not have to wait out the window.
            self.release();
        }
        session
    }

    /// Build the finish response for `ids`, committing what verifies.
    pub fn finish(&self, session: &mut Session, ids: &[String]) -> FinishResponse {
        let ids: Vec<String> = if ids.is_empty() {
            session.items.keys().cloned().collect()
        } else {
            ids.to_vec()
        };

        let items = ids
            .into_iter()
            .map(|id| {
                let id_for_response = id.clone();
                match session.commit(&id) {
                    Commit::Verified { saved_as } => ItemResult {
                        id: id_for_response,
                        verified: true,
                        saved_as: Some(saved_as),
                    },
                    _ => ItemResult {
                        id: id_for_response,
                        verified: false,
                        saved_as: None,
                    },
                }
            })
            .collect();

        FinishResponse { items }
    }

    /// Delete working directories of sessions that were never finished.
    async fn sweep(&self) {
        let now = SystemTime::now();
        let mut map = self.map.lock().await;
        map.retain(|_, session| match session.try_lock() {
            Ok(guard) => now
                .duration_since(guard.created)
                .map(|age| age < STALE_AFTER)
                .unwrap_or(true),
            Err(_) => true,
        });
        // The map only tracks sessions in this process; leftovers from a
        // previous run are removed by directory age.
        let parts = self.root.join(WORK_DIR).join("parts");
        if let Ok(entries) = fs::read_dir(&parts) {
            for entry in entries.flatten() {
                let stale = entry
                    .metadata()
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|modified| now.duration_since(modified).ok())
                    .is_some_and(|age| age > STALE_AFTER);
                if stale {
                    log::debug!("removing stale session {}", entry.path().display());
                    fs::remove_dir_all(entry.path()).ok();
                }
            }
        }
    }
}

/// True when `value` is exactly 64 hex characters.
#[must_use]
pub fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Payload bytes an item represents.
///
/// Text travels inline, so its manifest `size` is zero while its bytes are the
/// UTF-8 payload. The free-space number a person sees must count those too.
fn item_bytes(item: &ManifestItem) -> u64 {
    match item.kind {
        ItemKind::Text => item.text.as_deref().map_or(0, |text| text.len() as u64),
        ItemKind::File => item.size,
    }
}

/// Digest of everything that makes a manifest mean the same transfer.
///
/// Item ids are included because the client derives them from position in the
/// batch, which is stable for a re-run of the same command. The text payload
/// is included because text items carry their bytes in the manifest.
/// Whether a prepare from `fingerprint` at `peer` may reopen a session that
/// `owner` opened from `opened_from`.
///
/// A fingerprint names exactly one device, so that is the whole test when both
/// sides present one. Without certificates every sender is the same empty
/// value, and treating "no owner" as a match would let a stranger on the same
/// Wi-Fi reopen, finish or cancel a transfer the person approved for someone
/// else. The address is the narrowest fact left in that case: it cannot name a
/// device, but it does distinguish "the same machine came back" from "another
/// one arrived". Mixing the two forms never matches, because a session the
/// person approved for an anonymous sender must not become the property of a
/// proven identity, or the other way round.
fn same_sender(
    owner: Option<&str>,
    opened_from: IpAddr,
    fingerprint: Option<&str>,
    peer: IpAddr,
) -> bool {
    match (owner, fingerprint) {
        (Some(owner), Some(claim)) => owner == claim,
        (None, None) => opened_from == peer,
        _ => false,
    }
}

fn manifest_key(manifest: &[ManifestItem]) -> String {
    let mut hasher = Sha256::new();
    for item in manifest {
        hasher.update(item.id.as_bytes());
        hasher.update([0]);
        hasher.update(item.name.as_bytes());
        hasher.update([0]);
        hasher.update(item.size.to_le_bytes());
        hasher.update(item.sha256.to_ascii_lowercase().as_bytes());
        hasher.update(match item.kind {
            ItemKind::File => [0u8],
            ItemKind::Text => [1u8],
        });
        if let Some(text) = &item.text {
            hasher.update(text.as_bytes());
        }
    }
    hex::encode(hasher.finalize())
}

/// 64 random bits, rendered as lowercase hex.
fn new_session_id() -> String {
    let mut bytes = [0u8; 8];
    if getrandom::getrandom(&mut bytes).is_err() {
        // Randomness is unavailable: fall back to time. Session ids are not
        // secrets and live only inside TLS.
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        bytes = nanos.to_le_bytes();
    }
    hex::encode(bytes)
}

/// Whether a link failure means "hard links are not available here".
///
/// `Unsupported` is the POSIX answer. Android is the other one: its sandbox
/// denies `link` on app data files outright and the kernel reports `EACCES`,
/// even for two paths the same app owns. A genuinely unwritable destination
/// still fails at the rename, so treating a denied link as "try the fallback"
/// hides nothing.
fn is_link_unsupported(err: &std::io::Error) -> bool {
    matches!(
        err.kind(),
        ErrorKind::Unsupported | ErrorKind::PermissionDenied
    ) || err.raw_os_error() == Some(1) // ERROR_INVALID_FUNCTION
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let unique = format!(
            "flit-session-{}-{}-{name}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let path = std::env::temp_dir().join(unique);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn sha(data: &[u8]) -> String {
        hex::encode(Sha256::digest(data))
    }

    fn file_item(id: &str, name: &str, data: &[u8]) -> ManifestItem {
        ManifestItem {
            id: id.to_string(),
            name: name.to_string(),
            size: data.len() as u64,
            sha256: sha(data),
            mtime: None,
            kind: ItemKind::File,
            text: None,
        }
    }

    /// Write bytes the way the HTTP handler does: append, hash, count.
    fn feed(item: &mut Item, data: &[u8]) {
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&item.part)
            .unwrap();
        file.write_all(data).unwrap();
        file.flush().unwrap();
        item.hasher.update(data);
        item.received += data.len() as u64;
    }

    fn context() -> OfferContext {
        OfferContext {
            peer: "127.0.0.1".parse().unwrap(),
            fingerprint: None,
            device_id: None,
            alias: None,
        }
    }

    async fn open_session(root: &Path, items: Vec<ManifestItem>) -> Arc<Mutex<Session>> {
        let sessions = Sessions::new(root).unwrap();
        let (session, response) = sessions
            .begin(items, context(), None)
            .await
            .expect("session");
        assert_eq!(response.accepted.len(), 1);
        session
    }

    #[tokio::test]
    async fn a_matching_hash_commits_the_file() {
        let root = scratch("commit");
        let data = b"verified bytes";
        let session = open_session(&root, vec![file_item("i1", "out.txt", data)]).await;
        let mut session = session.lock().await;

        let item = session.items.get_mut("i1").unwrap();
        feed(item, data);

        let commit = session.commit("i1");
        assert!(matches!(commit, Commit::Verified { saved_as } if saved_as == "out.txt"));
        assert_eq!(fs::read(root.join("out.txt")).unwrap(), data);

        fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn a_wrong_hash_is_not_committed_and_resets_for_a_retry() {
        let root = scratch("mismatch");
        let expected = b"what the sender promised";
        let session = open_session(&root, vec![file_item("i1", "out.txt", expected)]).await;
        let mut session = session.lock().await;

        let item = session.items.get_mut("i1").unwrap();
        // Same length as the promise, different bytes: this must fail on the
        // hash, not on the byte count.
        feed(item, b"not what was promised!!!");
        assert!(matches!(session.commit("i1"), Commit::Mismatch));

        assert!(!root.join("out.txt").exists(), "nothing may be committed");
        let item = session.items.get("i1").unwrap();
        assert_eq!(item.received, 0, "the sender must start over");
        assert!(!item.part.exists(), "the bad bytes are discarded");

        fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn an_interrupted_upload_resumes() {
        let root = scratch("resume");
        let data = b"abcdefghij";
        let session = open_session(&root, vec![file_item("i1", "out.bin", data)]).await;
        let mut session = session.lock().await;

        let item = session.items.get_mut("i1").unwrap();
        feed(item, &data[..4]);
        assert!(matches!(session.commit("i1"), Commit::Incomplete));
        assert_eq!(session.items.get("i1").unwrap().received, 4);

        let item = session.items.get_mut("i1").unwrap();
        feed(item, &data[4..]);
        assert!(matches!(session.commit("i1"), Commit::Verified { .. }));
        assert_eq!(fs::read(root.join("out.bin")).unwrap(), data);

        fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn a_name_collision_becomes_a_numbered_sibling() {
        let root = scratch("collide");
        fs::write(root.join("out.txt"), b"already here").unwrap();

        let data = b"second copy";
        let session = open_session(&root, vec![file_item("i1", "out.txt", data)]).await;
        let mut session = session.lock().await;
        feed(session.items.get_mut("i1").unwrap(), data);

        let commit = session.commit("i1");
        assert!(
            matches!(commit, Commit::Verified { ref saved_as } if saved_as == "out (2).txt"),
            "{commit:?}"
        );
        assert_eq!(fs::read(root.join("out.txt")).unwrap(), b"already here");
        assert_eq!(fs::read(root.join("out (2).txt")).unwrap(), data);

        fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn text_items_are_written_at_commit() {
        let root = scratch("text");
        let text = "https://example.com";
        let item = ManifestItem {
            id: "t1".into(),
            name: "clipboard.txt".into(),
            size: 0,
            sha256: sha(text.as_bytes()),
            mtime: None,
            kind: ItemKind::Text,
            text: Some(text.into()),
        };

        let session = open_session(&root, vec![item]).await;
        let mut session = session.lock().await;
        let commit = session.commit("t1");
        assert!(matches!(commit, Commit::Verified { .. }));
        assert_eq!(
            fs::read_to_string(root.join("clipboard.txt")).unwrap(),
            text
        );

        fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn a_hostile_manifest_item_is_refused_without_a_path() {
        let root = scratch("hostile");
        let sessions = Sessions::new(&root).unwrap();
        let mut items = vec![file_item("ok", "fine.txt", b"fine")];
        items.push(file_item("bad", "../../etc/passwd", b"nope"));
        items.push(file_item("abs", "/etc/passwd", b"nope"));
        items.push(file_item("dev", "CON.txt", b"nope"));

        let (session, response) = sessions.begin(items, context(), None).await.unwrap();
        assert_eq!(response.accepted, vec!["ok".to_string()]);
        assert_eq!(response.rejected.len(), 3);
        assert!(
            response
                .rejected
                .iter()
                .all(|rejected| rejected.reason == ErrorCode::BadRequest)
        );

        let session = session.lock().await;
        assert_eq!(session.items.len(), 1);
        assert!(session.items.contains_key("ok"));
        drop(session);

        fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn an_identical_manifest_reuses_the_open_session() {
        let root = scratch("adopt");
        let sessions = Sessions::new(&root).unwrap();
        let manifest = vec![
            file_item("i1", "one.bin", b"first item bytes"),
            file_item("i2", "two.bin", b"second item bytes"),
        ];

        let (session, first) = sessions
            .begin(manifest.clone(), context(), None)
            .await
            .unwrap();
        assert_eq!(first.accepted, vec!["i1".to_string(), "i2".to_string()]);

        // The sender dies mid-item; the receiver keeps what it already wrote.
        {
            let mut guard = session.lock().await;
            let item = guard.items.get_mut("i1").unwrap();
            feed(item, b"first ");
            assert_eq!(item.received, 6);
        }

        // The same command again must not throw those bytes away.
        let (resumed, second) = sessions
            .begin(manifest.clone(), context(), None)
            .await
            .unwrap();
        assert_eq!(second.session_id, first.session_id);
        assert_eq!(second.accepted, first.accepted);
        assert!(second.rejected.is_empty());
        {
            let guard = resumed.lock().await;
            assert_eq!(guard.items["i1"].received, 6);
        }
        assert!(Arc::ptr_eq(&session, &resumed));

        // A different transfer gets a different session, even on the same root.
        // Finish the open session first: the receiver admits one at a time.
        sessions.take(&first.session_id).await;
        let (fresh, third) = sessions
            .begin(
                vec![file_item("i1", "other.bin", b"different")],
                context(),
                None,
            )
            .await
            .unwrap();
        assert_ne!(third.session_id, first.session_id);
        assert!(!Arc::ptr_eq(&session, &fresh));

        drop(session);
        drop(resumed);
        drop(fresh);
        fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn a_session_with_committed_items_is_never_adopted() {
        let root = scratch("adopt-done");
        let sessions = Sessions::new(&root).unwrap();
        let data = b"committed";
        let manifest = vec![file_item("i1", "done.bin", data)];

        let (session, first) = sessions
            .begin(manifest.clone(), context(), None)
            .await
            .unwrap();
        {
            let mut guard = session.lock().await;
            feed(guard.items.get_mut("i1").unwrap(), data);
            assert!(matches!(guard.commit("i1"), Commit::Verified { .. }));
        }

        // Re-sending the identical manifest starts over rather than reporting
        // an item the sender still has as already finished. The finished
        // session is gone by then, so the receiver is free to admit another.
        sessions.take(&first.session_id).await;
        let (second_session, second) = sessions.begin(manifest, context(), None).await.unwrap();
        assert_ne!(second.session_id, first.session_id);
        assert_eq!(second.accepted, vec!["i1".to_string()]);

        drop(session);
        drop(second_session);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn sha256_detection_is_exact() {
        assert!(is_sha256(&"a".repeat(64)));
        assert!(!is_sha256(&"a".repeat(63)));
        assert!(!is_sha256(&"g".repeat(64)));
        assert!(!is_sha256(""));
    }

    /// Android denies `link` on app data with `EACCES`, so that has to count as
    /// "no hard links here" or nothing can be installed on a phone.
    #[test]
    fn a_denied_link_falls_back_to_rename() {
        assert!(is_link_unsupported(&std::io::Error::from(
            ErrorKind::PermissionDenied
        )));
        assert!(is_link_unsupported(&std::io::Error::from(
            ErrorKind::Unsupported
        )));
        assert!(!is_link_unsupported(&std::io::Error::from(
            ErrorKind::AlreadyExists
        )));
    }

    fn ip(text: &str) -> IpAddr {
        text.parse().expect("a literal address")
    }

    /// The proven owner is the whole answer when both sides present one, and
    /// the address never overrides it: the same device on a new address resumes,
    /// a different device from the same address does not.
    #[test]
    fn a_fingerprint_names_the_only_sender_that_may_resume() {
        let owner = Some("a65cd186");
        assert!(same_sender(owner, ip("10.0.0.2"), owner, ip("10.0.0.2")));
        assert!(
            same_sender(owner, ip("10.0.0.2"), owner, ip("10.0.0.9")),
            "a device that proved its identity is not tied to one address"
        );
        assert!(!same_sender(
            owner,
            ip("10.0.0.2"),
            Some("d186a65c"),
            ip("10.0.0.2")
        ));
    }

    /// Without certificates there is no owner to compare, so the address has to
    /// stand in. Matching `None` to `None` would hand one person's approved
    /// session to any anonymous device that guessed the manifest.
    #[test]
    fn an_anonymous_session_reopens_only_from_its_own_address() {
        assert!(same_sender(None, ip("10.0.0.2"), None, ip("10.0.0.2")));
        assert!(!same_sender(None, ip("10.0.0.2"), None, ip("10.0.0.3")));
    }

    /// Consent is given to a form of identity, so it cannot change form: neither
    /// an anonymous client inheriting a proven session, nor a proven one
    /// inheriting an anonymous session.
    #[test]
    fn identity_forms_do_not_mix() {
        assert!(!same_sender(
            Some("a65cd186"),
            ip("10.0.0.2"),
            None,
            ip("10.0.0.2")
        ));
        assert!(!same_sender(
            None,
            ip("10.0.0.2"),
            Some("a65cd186"),
            ip("10.0.0.2")
        ));
    }
}
