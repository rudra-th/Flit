//! The shell's own state: the engine, the sends in flight, and the history.
//!
//! The engine already owns the network. What it does not own is the *story*: a
//! send row needs the name of the device it went to, and a history row needs
//! the paths the user actually chose (a folder, not its 400 files) so it can
//! be sent again. That context lives here, in one map keyed by transfer id, and
//! is written to disk the moment a send ends.

use std::{
    collections::HashMap,
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard},
};

use flit_core::{Engine, EngineConfig, Event, PeerView};
use tauri::{AppHandle, Emitter};
use tokio::sync::broadcast::error::RecvError;

use crate::{
    dto::{HistoryEntry, Item},
    events::{CHANNEL, EngineEvent},
};

/// How many past sends are kept.
///
/// A year of heavy use is a few hundred; past that the file is a log, not a
/// history, and a list nobody scrolls is not worth a megabyte.
const MAX_HISTORY: usize = 200;

/// The id used for entries loaded from disk, which no live transfer can have.
const NO_ID: u64 = 0;

/// The engine and everything the window needs beside it.
pub struct Shell {
    /// Shared with the event pump, which outlives the setup closure.
    pub inner: Arc<Inner>,
}

impl Shell {
    /// Load the identity, start the receiver, and start listening for events.
    pub fn start(app: &AppHandle) -> Result<Self, String> {
        let store = flit_crypto::IdentityFile::default_path().map_err(|err| err.to_string())?;
        let identity = store.load_or_create().map_err(|err| err.to_string())?;
        let history_path = store.path().with_file_name("history.json");

        let config = EngineConfig {
            alias: hostname(),
            ..EngineConfig::default()
        };
        let engine = tauri::async_runtime::block_on(Engine::start(Arc::new(identity), config))
            .map_err(|err| err.to_string())?;

        let inner = Arc::new(Inner {
            engine,
            data: Mutex::new(Data::load(history_path)),
        });

        pump(Arc::clone(&inner), app.clone());
        Ok(Self { inner })
    }
}

/// A handle on the engine and the send bookkeeping.
pub struct Inner {
    /// The engine every command goes through.
    pub engine: Engine,
    /// Sends in flight and past sends.
    pub data: Mutex<Data>,
}

impl Inner {
    /// The data lock, which survives a poisoned mutex.
    ///
    /// A panic while holding this lock means a bug, not corrupt state: the maps
    /// and the history vector are only ever replaced, never half-mutated.
    pub fn data(&self) -> MutexGuard<'_, Data> {
        self.data.lock().unwrap_or_else(|err| err.into_inner())
    }

    /// Turn one engine event into a window event, recording history on the way.
    async fn observe(&self, event: Event) -> Option<EngineEvent> {
        match event {
            Event::PeerSeen(view) => Some(EngineEvent::PeerSeen {
                peer: crate::dto::Peer::from(&view),
            }),
            Event::PeerLost { device_id } => Some(EngineEvent::PeerLost { device_id }),
            Event::DiscoveryUnavailable { reason } => {
                Some(EngineEvent::DiscoveryUnavailable { reason })
            }
            Event::TransferStarted {
                id,
                label,
                files,
                bytes,
                peer,
            } => {
                // The event carries the address but not the name, and the name
                // is what a row shows. The peer table still has it.
                let known = self
                    .engine
                    .peers()
                    .await
                    .into_iter()
                    .find(|view| view.addr == peer);
                let id = id.as_u64();
                self.data().begin(id, files, peer, known.as_ref());
                Some(EngineEvent::TransferStarted {
                    id,
                    label,
                    files,
                    bytes,
                    peer: peer.to_string(),
                })
            }
            Event::TransferProgress {
                id,
                sent,
                total,
                files_done,
                files_total,
            } => Some(EngineEvent::TransferProgress {
                id: id.as_u64(),
                sent,
                total,
                files_done,
                files_total,
            }),
            Event::TransferFinished { id, items } => {
                let id = id.as_u64();
                let items: Vec<Item> = items.iter().map(Item::from).collect();
                self.data().close(
                    id,
                    Ending::Finished {
                        items: items.clone(),
                    },
                );
                Some(EngineEvent::TransferFinished { id, items })
            }
            Event::TransferFailed { id, reason } => {
                let id = id.as_u64();
                self.data().close(id, Ending::Failed(reason.clone()));
                Some(EngineEvent::TransferFailed { id, reason })
            }
            Event::TransferCancelled { id } => {
                let id = id.as_u64();
                self.data().close(id, Ending::Cancelled);
                Some(EngineEvent::TransferCancelled { id })
            }
        }
    }
}

/// How a send ended, for the history row it produces.
pub enum Ending {
    /// Every item has a result.
    Finished {
        /// One entry per file.
        items: Vec<Item>,
    },
    /// The session failed before any per-item result existed.
    Failed(String),
    /// The user stopped it.
    Cancelled,
}

/// One send, from the moment it starts until the history row is written.
struct Sending {
    /// When the send started, in milliseconds since the Unix epoch.
    at: u64,
    /// Alias of the device it went to.
    alias: String,
    /// Its badge, if the fingerprint parsed.
    emoji: Option<String>,
    /// The key that was pinned.
    fingerprint: String,
    /// Where it went.
    addr: SocketAddr,
    /// Files the send declared.
    files: usize,
    /// Paths the user chose, for "send that again".
    paths: Vec<String>,
}

/// The bookkeeping the engine does not do for us.
pub struct Data {
    /// Past sends, newest first.
    history: Vec<HistoryEntry>,
    /// Transfer id per history row, so a late registration can still find its
    /// row. [`NO_ID`] for rows loaded from disk.
    history_ids: Vec<u64>,
    /// Where the history file lives.
    history_path: PathBuf,
    /// Sends that have started and not yet ended.
    sends: HashMap<u64, Sending>,
}

impl Data {
    /// Read the history file, or start empty when there is none.
    pub fn load(path: PathBuf) -> Self {
        let mut history = match std::fs::read(&path) {
            Ok(bytes) => {
                serde_json::from_slice::<Vec<HistoryEntry>>(&bytes).unwrap_or_else(|err| {
                    // A history file is convenience data. Refusing to start the
                    // app over it would be worse than losing it.
                    log::warn!("history at {} could not be read: {err}", path.display());
                    Vec::new()
                })
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(err) => {
                log::warn!("history at {} could not be read: {err}", path.display());
                Vec::new()
            }
        };
        history.truncate(MAX_HISTORY);
        Self {
            history_ids: vec![NO_ID; history.len()],
            history,
            history_path: path,
            sends: HashMap::new(),
        }
    }

    /// Past sends, newest first.
    pub fn history(&self) -> Vec<HistoryEntry> {
        self.history.clone()
    }

    /// Remember the paths a send was built from.
    ///
    /// The command that starts a send learns its id after the engine has
    /// already emitted `TransferStarted`, and in rare cases after the send has
    /// finished. Writing this twice, once into the live send and once into the
    /// history row, keeps "send that again" working either way.
    pub fn note_paths(&mut self, id: u64, paths: Vec<String>) {
        if let Some(send) = self.sends.get_mut(&id) {
            send.paths = paths;
            return;
        }
        if let Some(index) = self.history_ids.iter().position(|known| *known == id) {
            let written = self
                .history
                .get_mut(index)
                .map(|entry| entry.paths = paths)
                .is_some();
            if written {
                self.save();
            }
        }
    }

    /// Record that a send started.
    fn begin(&mut self, id: u64, files: usize, addr: SocketAddr, peer: Option<&PeerView>) {
        self.sends.insert(
            id,
            Sending {
                at: now_ms(),
                alias: peer
                    .map(|view| view.alias.clone())
                    .unwrap_or_else(|| addr.ip().to_string()),
                emoji: peer.and_then(|view| view.badge.as_ref().map(|badge| badge.emoji.clone())),
                fingerprint: peer
                    .map(|view| view.fingerprint.clone())
                    .unwrap_or_default(),
                addr,
                files,
                paths: Vec::new(),
            },
        );
    }

    /// Turn a finished send into a history row and write the file.
    fn close(&mut self, id: u64, ending: Ending) {
        let Some(send) = self.sends.remove(&id) else {
            // A send the shell never saw start: nothing to write.
            return;
        };

        let (files, bytes, ok, error) = match ending {
            Ending::Finished { items } => (
                items.len(),
                items.iter().map(|item| item.bytes).sum(),
                items.iter().all(|item| item.error.is_none()),
                items.iter().find_map(|item| item.error.clone()),
            ),
            Ending::Failed(reason) => (send.files, 0, false, Some(reason)),
            Ending::Cancelled => (
                send.files,
                0,
                false,
                Some("Cancelled before it finished.".to_string()),
            ),
        };

        self.history.insert(
            0,
            HistoryEntry {
                at: send.at,
                peer: send.alias,
                emoji: send.emoji,
                fingerprint: send.fingerprint,
                addr: send.addr.to_string(),
                files,
                bytes,
                ms: now_ms().saturating_sub(send.at),
                ok,
                error,
                paths: send.paths,
            },
        );
        self.history_ids.insert(0, id);
        self.history.truncate(MAX_HISTORY);
        self.history_ids.truncate(MAX_HISTORY);
        self.save();
    }

    /// Write the history beside the identity, atomically.
    ///
    /// A crash mid-write leaves the previous file rather than half a JSON
    /// array: the temporary is renamed into place only once it is complete.
    fn save(&self) {
        let body = match serde_json::to_vec_pretty(&self.history) {
            Ok(body) => body,
            Err(err) => {
                log::warn!("history could not be serialised: {err}");
                return;
            }
        };
        if let Some(parent) = self.history_path.parent() {
            if let Err(err) = std::fs::create_dir_all(parent) {
                log::warn!(
                    "history folder {} could not be created: {err}",
                    parent.display()
                );
                return;
            }
        }
        let temporary = self.history_path.with_extension("json.tmp");
        if let Err(err) = std::fs::write(&temporary, &body) {
            log::warn!("history could not be written: {err}");
            return;
        }
        if let Err(err) = std::fs::rename(&temporary, &self.history_path) {
            log::warn!("history could not be replaced: {err}");
        }
    }
}

/// Forward engine events to the window until the engine is dropped.
fn pump(inner: Arc<Inner>, app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut events = inner.engine.subscribe();
        loop {
            match events.recv().await {
                Ok(event) => {
                    if let Some(ready) = inner.observe(event).await {
                        // A closed window is not an error: the engine keeps
                        // running in the tray until the user quits.
                        let _ = app.emit(CHANNEL, ready);
                    }
                }
                Err(RecvError::Lagged(missed)) => {
                    // Progress is the only thing that can pile up, and a
                    // window that fell behind wants the newest numbers, not
                    // every one it missed.
                    log::warn!("the window missed {missed} engine events");
                }
                Err(RecvError::Closed) => return,
            }
        }
    });
}

/// The machine's name, which is what the device list should say.
fn hostname() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| flit_api::DEFAULT_ALIAS.to_string())
}

/// Milliseconds since the Unix epoch, which is what the window's clock expects.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}
