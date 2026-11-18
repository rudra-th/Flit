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

use flit_api::ItemKind;
use flit_core::{Engine, EngineConfig, Event, OfferId, OfferOutcome, PeerView};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use tokio::sync::broadcast::error::RecvError;

use crate::{
    dto::{HistoryEntry, Item, Offered},
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
        let policy_path = store.path().with_file_name("consent.json");
        let name = flit_crypto::NameFile::beside(&store)
            .load_or_create()
            .map_err(|err| err.to_string())?;

        // The same two rules the phone follows: nothing is accepted without an
        // answer unless the person turned that on, and "remember this device"
        // is the only other way past the question. Installed in the config so
        // it is in force before the receiver binds and can be asked.
        let policy = Policy::load(&policy_path);
        let config = EngineConfig {
            alias: name,
            auto_accept: policy.auto_accept,
            trusted: policy.trusted.clone(),
            ..EngineConfig::default()
        };
        let engine = tauri::async_runtime::block_on(Engine::start(Arc::new(identity), config))
            .map_err(|err| err.to_string())?;

        let inner = Arc::new(Inner {
            engine,
            data: Mutex::new(Data::load(history_path)),
            policy: Mutex::new(policy),
            policy_path,
            offers: Mutex::new(HashMap::new()),
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
    /// What this computer does when nothing answers an offer.
    policy: Mutex<Policy>,
    /// Where the policy is written, beside the identity.
    policy_path: PathBuf,
    /// Offers waiting for this window, by id.
    ///
    /// The engine owns the waiting; this is only what an answer needs once the
    /// person gives one: which items, and which key may be remembered.
    offers: Mutex<HashMap<u64, Pending>>,
}

/// One question on screen, from the shell's side.
struct Pending {
    /// Proven key, when the sender presented one. Trust is remembered under it
    /// and never under a name.
    fingerprint: Option<String>,
    /// The sender's ids for everything it offered.
    ids: Vec<String>,
}

/// What this computer does when another device asks to send something.
///
/// Two rules, the same ones the phone follows: a global switch, off by default,
/// and a set of fingerprints the person chose to trust. A display name is never
/// stored, because a name is a claim and a key is proof.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Policy {
    /// Accept every offer without asking.
    auto_accept: bool,
    /// Fingerprints accepted on their own.
    trusted: Vec<String>,
}

impl Policy {
    /// Read the policy, or start with everything asked about.
    ///
    /// A file that cannot be read is not a reason to start in a state nobody
    /// chose: the safe default is the one that asks.
    fn load(path: &PathBuf) -> Self {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|err| {
                log::warn!("consent policy at {} could not be read: {err}", path.display());
                Self::default()
            }),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(err) => {
                log::warn!("consent policy at {} could not be read: {err}", path.display());
                Self::default()
            }
        }
    }
}

impl Inner {
    /// The data lock, which survives a poisoned mutex.
    ///
    /// A panic while holding this lock means a bug, not corrupt state: the maps
    /// and the history vector are only ever replaced, never half-mutated.
    pub fn data(&self) -> MutexGuard<'_, Data> {
        self.data.lock().unwrap_or_else(|err| err.into_inner())
    }

    /// Whether offers are accepted without asking, and which keys are trusted.
    pub fn policy(&self) -> (bool, Vec<String>) {
        let policy = self.lock_policy();
        (policy.auto_accept, policy.trusted.clone())
    }

    /// Turn "accept without asking" on or off, and write it down.
    ///
    /// The engine is told after the file is written, so a crash in between
    /// leaves a policy on disk that the next start honours, not one the running
    /// receiver has already forgotten.
    pub fn set_auto_accept(&self, enabled: bool) {
        let (auto, trusted) = {
            let mut policy = self.lock_policy();
            policy.auto_accept = enabled;
            write_policy(&self.policy_path, &policy);
            (policy.auto_accept, policy.trusted.clone())
        };
        self.engine.set_auto_accept(auto, trusted);
    }

    /// Remember a key the person chose to trust, and write it down.
    pub fn trust(&self, fingerprint: String) {
        let fingerprint = fingerprint.trim().to_ascii_lowercase();
        if fingerprint.is_empty() {
            return;
        }
        let (auto, trusted) = {
            let mut policy = self.lock_policy();
            if !policy.trusted.contains(&fingerprint) {
                policy.trusted.push(fingerprint);
                policy.trusted.sort();
            }
            write_policy(&self.policy_path, &policy);
            (policy.auto_accept, policy.trusted.clone())
        };
        self.engine.set_auto_accept(auto, trusted);
    }

    /// Forget a key. Returns whether it was trusted to begin with.
    pub fn forget(&self, fingerprint: &str) -> bool {
        let mut policy = self.lock_policy();
        let before = policy.trusted.len();
        policy
            .trusted
            .retain(|known| !known.eq_ignore_ascii_case(fingerprint.trim()));
        if policy.trusted.len() == before {
            return false;
        }
        let auto = policy.auto_accept;
        let trusted = policy.trusted.clone();
        write_policy(&self.policy_path, &policy);
        drop(policy);
        self.engine.set_auto_accept(auto, trusted);
        true
    }

    /// Answer the question on screen. False when it had already closed.
    pub async fn answer(&self, id: u64, accept: bool, remember: bool) -> bool {
        let Some(pending) = self.take_offer(id) else {
            // A tap that lands after the sender gave up is not an answer, and
            // must not leave a device trusted behind it.
            return false;
        };
        let ids = if accept { pending.ids } else { Vec::new() };
        let trusting = accept && remember && pending.fingerprint.is_some();
        let answered = self
            .engine
            .answer_offer(OfferId::from_raw(id), ids, trusting)
            .await;
        if answered {
            if let Some(fingerprint) = pending.fingerprint.filter(|_| trusting) {
                self.trust(fingerprint);
            }
        }
        answered
    }

    fn take_offer(&self, id: u64) -> Option<Pending> {
        self.offers
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .remove(&id)
    }

    fn lock_policy(&self) -> MutexGuard<'_, Policy> {
        self.policy.lock().unwrap_or_else(|err| err.into_inner())
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
            Event::IncomingOffer {
                id,
                sender,
                items,
                bytes,
            } => {
                let id = id.as_u64();
                // Registered before the window hears about it: a fast tap must
                // find what it is answering.
                self.offers
                    .lock()
                    .unwrap_or_else(|err| err.into_inner())
                    .insert(
                        id,
                        Pending {
                            fingerprint: sender.fingerprint.clone(),
                            ids: items.iter().map(|item| item.id.clone()).collect(),
                        },
                    );
                Some(EngineEvent::IncomingOffer {
                    id,
                    sender: sender.alias.clone(),
                    fingerprint: sender.fingerprint.clone().unwrap_or_default(),
                    items: items
                        .iter()
                        .map(|item| Offered {
                            name: item.name.clone(),
                            bytes: item.size,
                            text: item.kind == ItemKind::Text,
                        })
                        .collect(),
                    bytes,
                })
            }
            Event::OfferClosed { id, outcome } => {
                // Dropped here too: an offer the policy answered never reached
                // this window, and one that timed out under a finger must not
                // leave a question that can still be answered.
                self.take_offer(id.as_u64());
                Some(EngineEvent::OfferClosed {
                    id: id.as_u64(),
                    outcome: outcome_name(outcome).to_string(),
                })
            }
        }
    }
}

/// How an outcome reads in the window, in the spelling TypeScript switches on.
fn outcome_name(outcome: OfferOutcome) -> &'static str {
    match outcome {
        OfferOutcome::Accepted => "accepted",
        OfferOutcome::Declined => "declined",
        OfferOutcome::TimedOut => "timedOut",
    }
}

/// Write the policy beside the identity.
///
/// A policy that cannot be written is logged rather than raised: the choice is
/// already in force for this run, and a dialog over it would not help the
/// transfer that is waiting.
fn write_policy(path: &PathBuf, policy: &Policy) {
    let Ok(bytes) = serde_json::to_vec_pretty(policy) else {
        return;
    };
    if let Err(err) = std::fs::write(path, bytes) {
        log::warn!(
            "consent policy at {} could not be written: {err}",
            path.display()
        );
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
                        // A question with no window in front is a question
                        // nobody answers, and an unanswered offer becomes a
                        // decline when it times out. Bring the window back for
                        // the one event that is waiting on a person; an offer
                        // the policy settled on its own never asks, so this
                        // does not interrupt anything.
                        if matches!(&ready, EngineEvent::IncomingOffer { .. }) {
                            crate::tray::reveal(&app);
                        }
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

/// Milliseconds since the Unix epoch, which is what the window's clock expects.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}
