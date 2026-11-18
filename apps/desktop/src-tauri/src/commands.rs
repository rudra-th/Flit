//! What the window may ask the shell to do.
//!
//! These are the whole surface. Each one is a verb a person would recognise —
//! "what is my status", "who is nearby", "send these files" — and none of them
//! exposes a socket, a manifest or a session. The engine decides how; this
//! module only translates and records.

use std::{
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
    time::Duration,
};

use flit_api::{DISCOVERY_PORT, MULTICAST_GROUP};
use flit_core::{DiscoveryState, collect_targets};
use if_addrs::IfAddr;
use tauri::{AppHandle, State};
use tokio::{net::TcpStream, time::timeout};

use crate::{
    dto::{Check, Discovery, Doctor, HistoryEntry, NetInterface, Peer, Status, TrayEntry},
    state::Shell,
};

/// This device: the name, the fingerprint, and where it is listening.
#[tauri::command]
pub async fn status(shell: State<'_, Shell>) -> Result<Status, String> {
    let engine = &shell.inner.engine;
    let info = engine.info();
    let fingerprint = info.fingerprint.clone();

    Ok(Status {
        alias: info.alias.clone(),
        fingerprint,
        addr: reachable(engine.addr()),
        root: engine.root().to_string_lossy().into_owned(),
        proto: info.proto,
        version: info.version.clone(),
        discovery: Discovery::from(engine.discovery()),
    })
}

/// Change the name other devices see.
///
/// The announce carries whatever name the receiver had when it started, so the
/// new one is written to disk and the shell restarts: someone who renamed their
/// device expects the other screen to say the new name now, not after the next
/// reboot.
#[tauri::command]
pub async fn rename(app: AppHandle, name: String) -> Result<String, String> {
    let store = flit_crypto::IdentityFile::default_path().map_err(|err| err.to_string())?;
    let saved = flit_crypto::NameFile::beside(&store)
        .save(&name)
        .map_err(|err| err.to_string())?;

    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_millis(600)).await;
        app.restart();
    });

    Ok(saved)
}

/// Devices visible right now.
#[tauri::command]
pub async fn peers(shell: State<'_, Shell>) -> Result<Vec<Peer>, String> {
    Ok(shell
        .inner
        .engine
        .peers()
        .await
        .iter()
        .map(Peer::from)
        .collect())
}

/// Past sends, newest first.
#[tauri::command]
pub async fn history(shell: State<'_, Shell>) -> Result<Vec<HistoryEntry>, String> {
    Ok(shell.inner.data().history())
}

/// The checks a person can act on, plus the machine's addresses.
#[tauri::command]
pub async fn doctor(shell: State<'_, Shell>) -> Result<Doctor, String> {
    let engine = &shell.inner.engine;
    let addr = reachable(engine.addr());
    let root = engine.root().to_path_buf();
    // Asked before the checks are built: a receiver that does not answer on its
    // own address will not answer a phone either, and that is worth saying with
    // evidence rather than a shrug.
    let answers = answers_on(&addr).await;

    let checks = vec![
        Check::new(
            "receive",
            "Receiving",
            "ok",
            format!("Listening on {addr}."),
        )
        .sub("Other devices connect to this address.")
        .value(addr.clone()),
        match engine.discovery() {
            DiscoveryState::Running => Check::new(
                "nearby",
                "Nearby devices",
                "ok",
                "Announcing and listening.",
            )
            .sub("Devices on this network appear on their own.")
            .value(multicast()),
            DiscoveryState::Off => {
                Check::new("nearby", "Nearby devices", "unknown", "Discovery is off.")
                    .sub("Send to a device by its address instead.")
            }
            DiscoveryState::Unavailable(reason) => Check::new(
                "nearby",
                "Nearby devices",
                "bad",
                "Multicast is blocked on this network.",
            )
            .sub(format!("{reason} Send by address; receiving still works."))
            .value(multicast()),
        },
        match probe(&root) {
            Ok(()) => Check::new("folder", "Receive folder", "ok", "Writable.")
                .value(root.display().to_string()),
            Err(err) => Check::new(
                "folder",
                "Receive folder",
                "bad",
                "Files cannot be written here.",
            )
            .sub(err)
            .value(root.display().to_string()),
        },
        Check::new(
            "firewall",
            "Inbound port",
            if answers { "ok" } else { "bad" },
            if answers {
                format!("The receiver answers on {addr}.")
            } else {
                format!("Nothing answered on {addr}.")
            },
        )
        .sub(if answers {
            // Windows asks once, when a program first listens, and a "no" is
            // remembered silently. This is the one sentence a person needs when
            // the PC is found but the transfer never starts.
            if cfg!(windows) {
                "Windows may ask once to allow Flit through the firewall. Allow it on private networks."
            } else {
                "If another device cannot connect, look here before looking there."
            }
        } else {
            "The listener is bound elsewhere, or something on this machine is blocking it."
        })
        .value(addr),
    ];

    Ok(Doctor {
        checks,
        interfaces: interfaces(),
    })
}

/// Expand dropped paths into what the tray shows, one row per path.
///
/// A path that cannot be read is reported only when nothing else worked: one
/// unreadable file in a drag of five should not throw away the other four.
#[tauri::command]
pub async fn inspect_paths(paths: Vec<PathBuf>) -> Result<Vec<TrayEntry>, String> {
    let mut entries = Vec::new();
    let mut first_error: Option<String> = None;

    for path in paths {
        match expand(&path) {
            Ok(entry) => entries.push(entry),
            Err(err) => {
                if first_error.is_none() {
                    first_error = Some(err);
                }
            }
        }
    }

    if entries.is_empty() {
        if let Some(err) = first_error {
            return Err(err);
        }
    }
    Ok(entries)
}

/// Send files to a device from the peer list.
#[tauri::command]
pub async fn send_to_peer(
    shell: State<'_, Shell>,
    device_id: String,
    paths: Vec<PathBuf>,
) -> Result<u64, String> {
    let collected = collect_targets(&paths, None).map_err(|err| err.to_string())?;
    let id = shell
        .inner
        .engine
        .send_to_peer(&device_id, collected.targets)
        .await
        .map_err(|err| err.to_string())?;

    // The engine emits `TransferStarted` before this returns, so the send is
    // already in the map; recording the paths here is what makes the history
    // row repeatable.
    shell.inner.data().note_paths(id.as_u64(), spell(&paths));
    Ok(id.as_u64())
}

/// Send the clipboard as one text item to a device from the peer list.
///
/// Text has no local path and nothing to stream: the engine hands it over inside
/// the manifest, and the receiver saves it as an ordinary file. No paths are
/// recorded, which is what makes "send that again" honestly unavailable for a
/// pasted link instead of silently empty.
#[tauri::command]
pub async fn send_text_to_peer(
    shell: State<'_, Shell>,
    device_id: String,
    text: String,
    name: String,
) -> Result<u64, String> {
    let id = shell
        .inner
        .engine
        .send_text_to_peer(&device_id, text, name)
        .await
        .map_err(|err| err.to_string())?;

    shell.inner.data().note_paths(id.as_u64(), Vec::new());
    Ok(id.as_u64())
}

/// Whatever is on the system clipboard, as text.
///
/// Read here rather than in the webview: the browser clipboard API needs a
/// permission this window never asks for, and a paste that fails silently is
/// worse than one that says why.
#[tauri::command]
pub async fn read_clipboard(app: AppHandle) -> Result<String, String> {
    use tauri_plugin_clipboard_manager::ClipboardExt;

    app.clipboard().read_text().map_err(|_| {
        "Flit could not read the clipboard. Copy the text again, then paste.".to_string()
    })
}

/// Send files to an address and a fingerprint, without discovery.
#[tauri::command]
pub async fn send_to_address(
    shell: State<'_, Shell>,
    addr: String,
    fingerprint: String,
    paths: Vec<PathBuf>,
) -> Result<u64, String> {
    let addr: SocketAddr = addr
        .trim()
        .parse()
        .map_err(|_| "That address needs a port, like 192.168.1.8:47821.".to_string())?;
    let collected = collect_targets(&paths, None).map_err(|err| err.to_string())?;
    let id = shell
        .inner
        .engine
        .send_to(addr, fingerprint.trim(), collected.targets)
        .await
        .map_err(|err| err.to_string())?;

    shell.inner.data().note_paths(id.as_u64(), spell(&paths));
    Ok(id.as_u64())
}

/// Stop a send. False when it had already ended.
#[tauri::command]
pub async fn cancel_transfer(shell: State<'_, Shell>, id: u64) -> Result<bool, String> {
    let engine = &shell.inner.engine;
    let running = engine
        .running()
        .await
        .into_iter()
        .find(|transfer| transfer.as_u64() == id);

    Ok(match running {
        Some(transfer) => engine.cancel(transfer).await,
        None => false,
    })
}

/// Answer the question a device is asking.
///
/// `accept` takes everything it offered or nothing at all: the desktop question
/// is about the transfer, not a file picker. `remember` is honoured only when
/// the sender proved a key, and only when the answer actually landed, so a tap
/// that arrived too late grants nothing.
#[tauri::command]
pub async fn answer_offer(
    shell: State<'_, Shell>,
    id: u64,
    accept: bool,
    remember: bool,
) -> Result<bool, String> {
    Ok(shell.inner.answer(id, accept, remember).await)
}

/// Whether this computer accepts offers without asking.
#[tauri::command]
pub fn auto_accept(shell: State<'_, Shell>) -> bool {
    shell.inner.policy().0
}

/// Accept every offer without asking, or stop doing that. Persisted.
#[tauri::command]
pub fn set_auto_accept(shell: State<'_, Shell>, enabled: bool) {
    shell.inner.set_auto_accept(enabled);
}

/// The fingerprints this computer takes without asking, sorted.
#[tauri::command]
pub fn trusted_devices(shell: State<'_, Shell>) -> Vec<String> {
    shell.inner.policy().1
}

/// Forget a trusted key. Returns whether it was trusted.
#[tauri::command]
pub fn forget_trusted(shell: State<'_, Shell>, fingerprint: String) -> bool {
    shell.inner.forget(&fingerprint)
}

impl Check {
    fn new(
        id: &'static str,
        name: &'static str,
        state: &'static str,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            id,
            name,
            state,
            detail: detail.into(),
            sub: None,
            value: None,
        }
    }

    fn sub(mut self, sub: impl Into<String>) -> Self {
        self.sub = Some(sub.into());
        self
    }

    fn value(mut self, value: impl Into<String>) -> Self {
        self.value = Some(value.into());
        self
    }
}

/// The address another device should use, as `ip:port`.
///
/// The receiver binds `0.0.0.0`, which nothing can connect to. The first
/// private IPv4 address is the one a person would read off the other screen.
fn reachable(bound: SocketAddr) -> String {
    let ip = primary_ipv4().unwrap_or_else(|| bound.ip());
    SocketAddr::new(ip, bound.port()).to_string()
}

/// Ask the receiver to answer on the address another device would dial.
///
/// This is a loopback-shaped probe: it proves the listener is bound to an
/// interface others can reach and is accepting connections. It cannot see past
/// this machine's firewall, so the Doctor reports what was answered and lets
/// the firewall sentence carry the rest.
async fn answers_on(addr: &str) -> bool {
    let Ok(target) = addr.parse::<SocketAddr>() else {
        return false;
    };
    matches!(
        timeout(Duration::from_millis(500), TcpStream::connect(target)).await,
        Ok(Ok(_))
    )
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

/// Every address on this machine, loopback last.
fn interfaces() -> Vec<NetInterface> {
    let mut rows: Vec<NetInterface> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .map(|iface| {
            let prefix = match iface.addr {
                IfAddr::V4(ref v4) => v4.prefixlen,
                IfAddr::V6(ref v6) => v6.prefixlen,
            };
            let ip = iface.ip();
            let loopback = iface.is_loopback();
            NetInterface {
                name: iface.name,
                addr: format!("{ip}/{prefix}"),
                loopback,
                // if-addrs lists addresses, not link state, and the platforms
                // disagree about whether a down interface is listed at all.
                // Saying "up" is the honest default for a diagnostic whose job
                // is naming addresses.
                up: true,
            }
        })
        .collect();

    rows.sort_by(|left, right| {
        left.loopback
            .cmp(&right.loopback)
            .then_with(|| left.name.cmp(&right.name))
    });
    rows
}

/// Prove the receive folder can be written before promising it can.
fn probe(root: &Path) -> Result<(), String> {
    std::fs::create_dir_all(root).map_err(|err| err.to_string())?;
    let test = root.join(".flit-write-test");
    std::fs::write(&test, b"flit").map_err(|err| err.to_string())?;
    let _ = std::fs::remove_file(&test);
    Ok(())
}

/// The multicast group and port discovery uses.
fn multicast() -> String {
    let [a, b, c, d] = MULTICAST_GROUP;
    format!("{a}.{b}.{c}.{d}:{DISCOVERY_PORT}")
}

/// One dropped path, as the tray row for it.
fn expand(path: &Path) -> Result<TrayEntry, String> {
    let metadata = std::fs::metadata(path).map_err(|err| format!("{}: {err}", path.display()))?;

    if metadata.is_dir() {
        let owned = path.to_path_buf();
        let collected = collect_targets(std::slice::from_ref(&owned), None)
            .map_err(|err| format!("{}: {err}", path.display()))?;
        let bytes = collected
            .targets
            .iter()
            .filter_map(|target| std::fs::metadata(&target.path).ok())
            .map(|metadata| metadata.len())
            .sum();

        Ok(TrayEntry {
            path: path.display().to_string(),
            name: name_of(path),
            is_dir: true,
            files: collected.targets.len(),
            bytes,
        })
    } else if metadata.is_file() {
        Ok(TrayEntry {
            path: path.display().to_string(),
            name: name_of(path),
            is_dir: false,
            files: 1,
            bytes: metadata.len(),
        })
    } else {
        Err(format!("{} is not a file or a folder.", path.display()))
    }
}

/// The last segment of a path, which is what a row has room for.
fn name_of(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

/// Paths as strings, in the order the user chose them.
fn spell(paths: &[PathBuf]) -> Vec<String> {
    paths
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect()
}
