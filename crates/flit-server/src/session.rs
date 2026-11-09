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
    collections::HashMap,
    fs,
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime},
};

use flit_api::{
    ErrorCode, FinishResponse, ItemKind, ItemResult, MAX_FILE_BYTES, MAX_ITEMS, MAX_TOTAL_BYTES,
    ManifestItem, RejectedItem, SessionResponse,
};
use flit_storage::{conflict_name, safe_join, sanitize_relative};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

/// Directory under the destination root that holds in-flight data.
const WORK_DIR: &str = ".flit";

/// How long an abandoned session may keep its `.part` files.
const STALE_AFTER: Duration = Duration::from_secs(60 * 60);

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

    /// Forget everything received, so the sender can start over.
    fn reset(&mut self) -> std::io::Result<()> {
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
    /// Destination root chosen by the user.
    pub root: PathBuf,
    /// Where `.part` files live.
    pub dir: PathBuf,
    /// Items by id.
    pub items: HashMap<String, Item>,
    /// Set when either side cancels; in-flight uploads must stop.
    pub cancelled: bool,
    /// When this session was created, for sweeping.
    pub created: SystemTime,
}

impl Session {
    /// Create the working directory for a new session.
    fn create(root: &Path, id: String) -> std::io::Result<Self> {
        let dir = root.join(WORK_DIR).join("parts").join(&id);
        fs::create_dir_all(&dir)?;
        Ok(Self {
            id,
            root: root.to_path_buf(),
            dir,
            items: HashMap::new(),
            cancelled: false,
            created: SystemTime::now(),
        })
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
            if written.is_err() {
                self.items.insert(item.id.clone(), item);
                return Commit::Incomplete;
            }
            item.hasher.update(text.as_bytes());
            item.received = text.len() as u64;
        }

        if !item.is_complete() {
            self.items.insert(item.id.clone(), item);
            return Commit::Incomplete;
        }

        let digest = hex::encode(item.hasher.clone().finalize());
        if digest != item.sha256 {
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

/// All live sessions of one server.
pub struct Sessions {
    root: PathBuf,
    map: Mutex<HashMap<String, Arc<Mutex<Session>>>>,
}

impl Sessions {
    /// Prepare the destination root and its work directory.
    pub fn new(root: impl Into<PathBuf>) -> std::io::Result<Self> {
        let root = root.into();
        fs::create_dir_all(root.join(WORK_DIR).join("parts"))?;
        Ok(Self {
            root,
            map: Mutex::new(HashMap::new()),
        })
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
        manifest: Vec<ManifestItem>,
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

        self.sweep().await;

        let id = new_session_id();
        let mut session =
            Session::create(&self.root, id.clone()).map_err(|_| ErrorCode::Internal)?;
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
            return Err(ErrorCode::BadRequest);
        }

        let response = SessionResponse {
            session_id: id.clone(),
            accepted,
            rejected,
            offset: 0,
        };
        let session = Arc::new(Mutex::new(session));
        self.map.lock().await.insert(id, Arc::clone(&session));
        Ok((session, response))
    }

    /// Look up a session by id.
    pub async fn get(&self, id: &str) -> Option<Arc<Mutex<Session>>> {
        self.map.lock().await.get(id).cloned()
    }

    /// Forget a session. In-flight uploads notice via `cancelled`.
    pub async fn take(&self, id: &str) -> Option<Arc<Mutex<Session>>> {
        let session = self.map.lock().await.remove(id);
        if let Some(session) = &session {
            let mut guard = session.lock().await;
            guard.cancelled = true;
            guard.cleanup();
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

/// Whether a link failure means "this filesystem has no hard links".
fn is_link_unsupported(err: &std::io::Error) -> bool {
    err.kind() == ErrorKind::Unsupported || err.raw_os_error() == Some(1) // ERROR_INVALID_FUNCTION
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

    async fn open_session(root: &Path, items: Vec<ManifestItem>) -> Arc<Mutex<Session>> {
        let sessions = Sessions::new(root).unwrap();
        let (session, response) = sessions.begin(items).await.expect("session");
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

        let (session, response) = sessions.begin(items).await.unwrap();
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

    #[test]
    fn sha256_detection_is_exact() {
        assert!(is_sha256(&"a".repeat(64)));
        assert!(!is_sha256(&"a".repeat(63)));
        assert!(!is_sha256(&"g".repeat(64)));
        assert!(!is_sha256(""));
    }
}
