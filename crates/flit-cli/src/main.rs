//! The headless Flit binary.
//!
//! `flit doctor` is the same diagnostic the desktop and Android shells will
//! show: build and protocol facts first, then a set of self-checks that fail
//! loudly. It exists from M0 because "device not found" is the most common
//! support question in this category, and a project with no crash reporting
//! needs a diagnostic that a user can paste into an issue.
//!
//! `flit serve` and `flit info` are the M1 transport test harness: they let two
//! machines prove, by hand, that the TLS listener works and that a wrong pin is
//! refused before a byte of file data moves.

use std::{
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use clap::{Parser, Subcommand};
use flit_api::{Announce, DEFAULT_ALIAS, DeviceType, PROTO_VERSION, TLS_PORT};
use flit_client::{Client, SendTarget};
use flit_crypto::{Identity, IdentityFile, StoreError, badge_of};
use flit_discovery::{Discovery, DiscoveryConfig, PeerSource, announce_for};
use flit_server::{Server, ServerConfig};

/// Files, straight across.
#[derive(Debug, Parser)]
#[command(name = "flit", version, about = "Files, straight across.", long_about = None)]
struct Cli {
    /// Identity file to use instead of the per-user default.
    #[arg(long, global = true, value_name = "PATH")]
    identity: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Command>,
}

/// What to do.
#[derive(Debug, Subcommand)]
enum Command {
    /// Print build facts and run the built-in self-checks.
    Doctor,
    /// Show this device's identity, fingerprint and safety badge.
    Identity,
    /// Find nearby devices on this network.
    Discover {
        /// How long to listen for multicast announces.
        #[arg(long, default_value_t = 3)]
        seconds: u64,
        /// Also scan this machine's private /24s over TLS.
        #[arg(long)]
        scan: bool,
        /// Print machine-readable JSON.
        #[arg(long)]
        json: bool,
        /// Control port to probe during a scan.
        #[arg(long, default_value_t = TLS_PORT)]
        scan_port: u16,
        /// Discovery port. Only useful when something else owns 47820.
        #[arg(long, default_value_t = flit_api::DISCOVERY_PORT, hide = true)]
        discovery_port: u16,
    },
    /// Receive files until interrupted.
    Serve {
        /// Name shown to other devices.
        #[arg(long)]
        alias: Option<String>,
        /// Address to bind.
        #[arg(long, default_value = "0.0.0.0")]
        bind: IpAddr,
        /// Control port. 0 asks the operating system for a free one.
        #[arg(long, default_value_t = flit_api::TLS_PORT)]
        port: u16,
        /// Directory to save into.
        #[arg(long, value_name = "DIR")]
        dir: Option<PathBuf>,
        /// Require this PIN from senders. 4-12 digits.
        #[arg(long, value_name = "PIN")]
        pin: Option<String>,
        /// Do not announce this device on the local network.
        #[arg(long)]
        no_announce: bool,
    },
    /// Send one file to a pinned device.
    Send {
        /// Files or folders to send.
        #[arg(required = true, num_args = 1.., value_name = "PATH")]
        files: Vec<PathBuf>,
        /// Peer address, e.g. 192.168.1.20:47821.
        #[arg(long, value_name = "ADDR")]
        to: SocketAddr,
        /// Fingerprint from discovery, a QR code, or `flit identity`.
        #[arg(long)]
        fingerprint: String,
        /// Name to save as, instead of the local file name.
        #[arg(long)]
        name: Option<String>,
        /// PIN for first contact.
        #[arg(long)]
        pin: Option<String>,
    },
    /// Ask a device who it is, refusing anything but the pinned key.
    Info {
        /// Peer address, e.g. 192.168.1.20:47821.
        addr: SocketAddr,
        /// Fingerprint from discovery, a QR code, or `flit identity`.
        #[arg(long)]
        fingerprint: String,
        /// PIN for first contact.
        #[arg(long)]
        pin: Option<String>,
    },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let code = match cli.command {
        Some(Command::Doctor) | None => doctor(),
        Some(Command::Identity) => identity(cli.identity.as_deref()),
        Some(Command::Discover {
            seconds,
            scan,
            json,
            scan_port,
            discovery_port,
        }) => {
            discover(
                cli.identity.as_deref(),
                seconds,
                scan,
                json,
                scan_port,
                discovery_port,
            )
            .await
        }
        Some(Command::Serve {
            alias,
            bind,
            port,
            dir,
            pin,
            no_announce,
        }) => {
            serve(
                cli.identity.as_deref(),
                alias,
                bind,
                port,
                dir,
                pin,
                !no_announce,
            )
            .await
        }
        Some(Command::Info {
            addr,
            fingerprint,
            pin,
        }) => info(addr, &fingerprint, pin.as_deref()).await,
        Some(Command::Send {
            files,
            to,
            fingerprint,
            name,
            pin,
        }) => send(&files, to, &fingerprint, name.as_deref(), pin.as_deref()).await,
    };
    std::process::exit(code);
}

/// Resolve the identity file, defaulting to the per-user location.
fn identity_file(path: Option<&Path>) -> Result<IdentityFile, StoreError> {
    match path {
        Some(path) => Ok(IdentityFile::new(path)),
        None => IdentityFile::default_path(),
    }
}

/// Load the device identity, or explain in one line why it could not be.
fn load_identity(path: Option<&Path>) -> Result<(Identity, IdentityFile), String> {
    let store = identity_file(path).map_err(|err| err.to_string())?;
    let identity = store.load_or_create().map_err(|err| err.to_string())?;
    Ok((identity, store))
}

/// Print the identity, fingerprint and badge the way a user would compare them.
fn print_identity(identity: &Identity, store: &IdentityFile) {
    println!("flit {}", env!("CARGO_PKG_VERSION"));
    println!("  device        {}", identity.device_id());
    println!("  fingerprint   {}", identity.fingerprint());
    println!(
        "  safety badge  {}  {}",
        identity.badge_emoji(),
        identity.badge_words()
    );
    println!("  identity file {}", store.path().display());
}

/// `flit identity`: show what a peer will pin.
fn identity(path: Option<&Path>) -> i32 {
    match load_identity(path) {
        Ok((identity, store)) => {
            print_identity(&identity, &store);
            0
        }
        Err(err) => {
            eprintln!("flit: {err}");
            1
        }
    }
}

/// One device, owned so the peer table lock is released before printing.
struct Discovered {
    addr: SocketAddr,
    alias: String,
    device_id: String,
    fingerprint: String,
    source: PeerSource,
    caps: Vec<String>,
    pin_required: bool,
}

/// `flit discover`: list devices on this network.
///
/// Listening is the default and scanning is opt-in, because listening costs
/// nothing and a scan touches 254 hosts. The plan's rule is that the scan is
/// the fallback, so it is offered on the command line as exactly that.
async fn discover(
    path: Option<&Path>,
    seconds: u64,
    do_scan: bool,
    json: bool,
    scan_port: u16,
    discovery_port: u16,
) -> i32 {
    let (identity, _store) = match load_identity(path) {
        Ok(loaded) => loaded,
        Err(err) => {
            eprintln!("flit: {err}");
            return 1;
        }
    };

    // Discover-only: the announce is not sent, it only describes who we are to
    // the peer table and supplies the port a scan should dial.
    let announce = Announce {
        proto: PROTO_VERSION,
        alias: DEFAULT_ALIAS.to_string(),
        device_id: identity.device_id(),
        fingerprint: identity.fingerprint(),
        device_type: DeviceType::Desktop,
        port: scan_port,
        caps: Vec::new(),
        pin_required: false,
    };

    let config = DiscoveryConfig::default()
        .with_port(discovery_port)
        .without_announcing();
    let discovery = match Discovery::start(announce, config).await {
        Ok(discovery) => discovery,
        Err(err) => {
            eprintln!("flit: {err}");
            return 1;
        }
    };

    if !json {
        println!("  self          {}", identity.device_id());
        if do_scan {
            println!("  listening     udp {discovery_port} for {seconds}s, then scanning");
        } else {
            println!("  listening     udp {discovery_port} for {seconds}s");
        }
    }

    let listening = tokio::time::sleep(Duration::from_secs(seconds));
    let scan_report = if do_scan {
        // The listener runs in its own task, so a scan and the announce feed
        // fill the same table at the same time.
        let ((), report) = tokio::join!(listening, discovery.scan());
        Some(report)
    } else {
        listening.await;
        None
    };

    let table = discovery.table();
    let peers: Vec<Discovered> = {
        let guard = table.read().await;
        guard
            .snapshot(Instant::now())
            .into_iter()
            .map(|peer| Discovered {
                addr: peer.addr(),
                alias: peer.announce().alias.clone(),
                device_id: peer.announce().device_id.clone(),
                fingerprint: peer.announce().fingerprint.clone(),
                source: peer.source(),
                caps: peer.announce().caps.clone(),
                pin_required: peer.announce().pin_required,
            })
            .collect()
    };

    if json {
        let rows: Vec<serde_json::Value> = peers
            .iter()
            .map(|peer| {
                serde_json::json!({
                    "alias": peer.alias,
                    "device_id": peer.device_id,
                    "fingerprint": peer.fingerprint,
                    "addr": peer.addr.to_string(),
                    "source": source_name(peer.source),
                    "caps": peer.caps,
                    "pin_required": peer.pin_required,
                })
            })
            .collect();
        match serde_json::to_string_pretty(&rows) {
            Ok(text) => println!("{text}"),
            Err(err) => {
                eprintln!("flit: {err}");
                return 1;
            }
        }
        return 0;
    }

    if let Some(report) = &scan_report {
        println!(
            "  scan          {} targets, {} probed, {} answered",
            report.targets,
            report.probed,
            report.found.len()
        );
        if report.incomplete() {
            println!("                (budget ran out before every host was tried)");
        }
    }

    if peers.is_empty() {
        println!();
        println!("No devices found.");
        println!("Run `flit discover --scan` if this network filters multicast.");
        return 0;
    }

    println!();
    for peer in &peers {
        let badge = badge_of(&peer.fingerprint);
        let emoji = badge.as_ref().map(|b| b.emoji.as_str()).unwrap_or("----");
        let words = badge.as_ref().map(|b| b.words.as_str()).unwrap_or("");
        println!("  {emoji}  {}", peer.alias);
        println!(
            "        {}  {}  {}",
            peer.addr,
            peer.device_id,
            source_name(peer.source)
        );
        if !peer.caps.is_empty() {
            println!("        caps: {}", peer.caps.join(", "));
        }
        if peer.pin_required {
            println!("        pin required");
        }
        println!("        {words}");
    }
    println!();
    println!(
        "Pin one with --fingerprint. Full value:\n{}",
        peers
            .iter()
            .map(|peer| format!("  {}  {}", peer.device_id, peer.fingerprint))
            .collect::<Vec<_>>()
            .join("\n")
    );

    if let Some(report) = scan_report {
        if report.incomplete() {
            println!();
            println!("The scan ran out of time; run it again on a quieter network.");
        }
    }
    0
}

fn source_name(source: PeerSource) -> &'static str {
    match source {
        PeerSource::Multicast => "multicast",
        PeerSource::Scan => "scan",
        PeerSource::Manual => "manual",
    }
}

/// `flit serve`: receive until Ctrl-C.
async fn serve(
    path: Option<&Path>,
    alias: Option<String>,
    bind: IpAddr,
    port: u16,
    dir: Option<PathBuf>,
    pin: Option<String>,
    announce_enabled: bool,
) -> i32 {
    let (identity, store) = match load_identity(path) {
        Ok(loaded) => loaded,
        Err(err) => {
            eprintln!("flit: {err}");
            return 1;
        }
    };

    let mut config = ServerConfig::default().with_bind(SocketAddr::new(bind, port));
    if let Some(alias) = alias {
        config = config.with_alias(alias);
    }
    if let Some(dir) = dir {
        config = config.with_root(dir);
    }
    if let Some(pin) = pin {
        config = config.with_pin(pin);
    }

    let server = match Server::bind(&identity, config).await {
        Ok(server) => server,
        Err(err) => {
            eprintln!("flit: {err}");
            return 1;
        }
    };

    // Announcing is what makes the device appear without anyone typing an
    // address. A bind failure here is not fatal: the server still works, the
    // user just has to reach it by address or QR.
    let discovery = if announce_enabled {
        match Discovery::start(announce_for(server.info()), DiscoveryConfig::default()).await {
            Ok(discovery) => Some(discovery),
            Err(err) => {
                eprintln!("flit: discovery unavailable: {err}");
                None
            }
        }
    } else {
        None
    };

    print_identity(&identity, &store);
    println!("  listening     {}", server.local_addr());
    println!("  saving to     {}", server.root().display());
    println!("  protocol      {}", server.info().proto);
    if discovery.is_some() {
        println!("  discovering   udp {}", flit_api::DISCOVERY_PORT);
    } else if announce_enabled {
        println!("  discovering   disabled");
    }
    if server.info().pin_required {
        println!("  pin           required for every request except /info");
    }
    println!();
    println!("Waiting for a peer. Press Ctrl-C to stop.");

    let stopped = server
        .run_until(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await;

    match stopped {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("flit: {err}");
            1
        }
    }
}

/// `flit info`: dial a pinned peer and print what it says about itself.
async fn info(addr: SocketAddr, fingerprint: &str, pin: Option<&str>) -> i32 {
    let mut client = match Client::new(addr, fingerprint) {
        Ok(client) => client,
        Err(err) => {
            eprintln!("flit: {err}");
            return 1;
        }
    };
    if let Some(pin) = pin {
        client = client.with_pin(pin);
    }

    let mut connection = match client.connect().await {
        Ok(connection) => connection,
        Err(err) => {
            eprintln!("flit: {err}");
            return 1;
        }
    };

    let info = match connection.info().await {
        Ok(info) => info,
        Err(err) => {
            eprintln!("flit: {err}");
            return 1;
        }
    };
    connection.close();

    match serde_json::to_string_pretty(&info) {
        Ok(json) => println!("{json}"),
        Err(err) => {
            eprintln!("flit: {err}");
            return 1;
        }
    }

    if let Some(badge) = badge_of(&info.fingerprint) {
        let short = info.fingerprint.get(..16).unwrap_or(&info.fingerprint);
        println!("\n{}  {}", badge.emoji, badge.words);
        println!("{short}  {}", info.alias);
    }
    0
}

/// `flit send`: send one file to a pinned peer.
/// Expand command-line paths into the files a session will carry.
///
/// A directory keeps its own name at the receiver, so `flit send photos`
/// produces `photos/…` rather than leaking the local absolute path. Symlinks
/// are skipped: following them can escape the directory the user named and can
/// loop.
fn collect_inputs(paths: &[PathBuf], name: Option<&str>) -> Result<Vec<SendTarget>, String> {
    if name.is_some() && paths.len() > 1 {
        return Err("--name only applies when sending one file".into());
    }

    let mut out = Vec::new();
    for path in paths {
        let metadata =
            std::fs::metadata(path).map_err(|err| format!("{}: {err}", path.display()))?;
        if metadata.is_dir() {
            if name.is_some() {
                return Err("--name only applies to a file, not a folder".into());
            }
            let base = path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| format!("{} has no folder name", path.display()))?
                .to_string();
            let root = path.clone();
            let mut stack = vec![root.clone()];
            while let Some(dir) = stack.pop() {
                let entries =
                    std::fs::read_dir(&dir).map_err(|err| format!("{}: {err}", dir.display()))?;
                for entry in entries {
                    let entry = entry.map_err(|err| format!("{}: {err}", dir.display()))?;
                    let kind = entry
                        .file_type()
                        .map_err(|err| format!("{}: {err}", entry.path().display()))?;
                    if kind.is_dir() {
                        stack.push(entry.path());
                    } else if kind.is_file() {
                        let full = entry.path();
                        let relative = full.strip_prefix(&root).unwrap_or(&full);
                        let remote =
                            format!("{base}/{}", relative.to_string_lossy().replace('\\', "/"));
                        out.push(SendTarget {
                            path: full,
                            name: Some(remote),
                        });
                    }
                }
            }
        } else if metadata.is_file() {
            out.push(SendTarget {
                path: path.clone(),
                name: name.map(str::to_owned),
            });
        } else {
            return Err(format!("{} is not a file or directory", path.display()));
        }
    }

    out.sort_by(|a, b| a.path.cmp(&b.path));
    if out.is_empty() {
        return Err("nothing to send".into());
    }
    Ok(out)
}

/// `flit send`: one session, one or many files.
async fn send(
    files: &[PathBuf],
    addr: SocketAddr,
    fingerprint: &str,
    name: Option<&str>,
    pin: Option<&str>,
) -> i32 {
    let targets = match collect_inputs(files, name) {
        Ok(targets) => targets,
        Err(err) => {
            eprintln!("flit: {err}");
            return 1;
        }
    };

    let mut client = match Client::new(addr, fingerprint) {
        Ok(client) => client,
        Err(err) => {
            eprintln!("flit: {err}");
            return 1;
        }
    };
    if let Some(pin) = pin {
        client = client.with_pin(pin);
    }

    let mut connection = match client.connect().await {
        Ok(connection) => connection,
        Err(err) => {
            eprintln!("flit: {err}");
            return 1;
        }
    };

    let started = Instant::now();
    let results = connection.send_files(&targets).await;
    let elapsed = started.elapsed();
    connection.close();

    let outcomes = match results {
        Ok(outcomes) => outcomes,
        Err(err) => {
            eprintln!("flit: {err}");
            return 1;
        }
    };

    let mut failures = 0usize;
    let mut bytes = 0u64;
    let verbose = outcomes.len() <= 20;
    for outcome in &outcomes {
        match &outcome.result {
            Ok(saved) => {
                bytes += saved.bytes;
                if verbose {
                    println!("sent {} -> {}", outcome.path.display(), saved.saved_as);
                }
            }
            Err(err) => {
                failures += 1;
                eprintln!("flit: {}: {err}", outcome.path.display());
            }
        }
    }

    let seconds = elapsed.as_secs_f64();
    let mib = bytes as f64 / (1024.0 * 1024.0);
    println!(
        "{} of {} files, {mib:.1} MiB in {seconds:.2}s ({:.1} MiB/s){}",
        outcomes.len() - failures,
        outcomes.len(),
        if seconds > 0.0 { mib / seconds } else { 0.0 },
        if failures == 0 { ", verified" } else { "" }
    );

    if failures == 0 { 0 } else { 1 }
}

/// Print facts, run self-checks, and return a process exit code.
fn doctor() -> i32 {
    println!("flit {}", env!("CARGO_PKG_VERSION"));
    println!("  protocol      {}", flit_api::PROTO_VERSION);
    println!(
        "  platform      {} {}",
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    println!("  discovery     udp {}", flit_api::DISCOVERY_PORT);
    println!("  control       tcp {}", flit_api::TLS_PORT);
    println!("  browser       tcp {}", flit_api::BROWSER_PORT);
    println!();

    let mut failures = 0;

    // Identity: a keypair must produce a stable, well-formed fingerprint.
    let identity = Identity::from_secret_bytes([42u8; 32]);
    failures += report(
        "identity fingerprint",
        identity.fingerprint().len() == 64 && identity.device_id().len() == 16,
        &format!(
            "{} / {} / {}",
            identity.device_id(),
            identity.badge_emoji(),
            identity.badge_words()
        ),
    );

    // Signatures must round-trip and must not verify under tampering.
    let signed = identity.sign(b"doctor");
    let signature_ok = flit_crypto::verify(&identity.public_key(), b"doctor", &signed);
    let tamper_ok = !flit_crypto::verify(&identity.public_key(), b"doctoR", &signed);
    failures += report(
        "signature round trip",
        signature_ok && tamper_ok,
        "sign, verify, reject tampered message",
    );

    // The TLS certificate must carry the identity key, or pinning is theatre.
    let cert_ok = identity
        .tls_identity(&["localhost".to_string()])
        .ok()
        .and_then(|tls| flit_crypto::ed25519_public_key(tls.cert_chain[0].as_ref()).ok())
        .is_some_and(|key| key == identity.public_key());
    failures += report(
        "certificate identity",
        cert_ok,
        "certificate key equals the pinned key",
    );

    // Path safety: the rules that stop a hostile manifest from escaping.
    let traversal_blocked = flit_storage::sanitize_relative("../etc/passwd").is_err();
    let absolute_blocked = flit_storage::sanitize_relative("/etc/passwd").is_err();
    let device_blocked = flit_storage::sanitize_segment("CON.txt").is_err();
    failures += report(
        "path safety",
        traversal_blocked && absolute_blocked && device_blocked,
        "traversal, absolute paths and reserved names rejected",
    );

    // Conflict naming must never silently overwrite.
    let renamed = flit_storage::conflict_name("report.pdf", 2);
    failures += report("conflict naming", renamed == "report (2).pdf", &renamed);

    println!();
    if failures == 0 {
        println!("all checks passed");
        0
    } else {
        println!("{failures} check(s) failed");
        1
    }
}

/// Print one self-check and return 0 or 1 so callers can accumulate failures.
fn report(name: &str, ok: bool, detail: &str) -> i32 {
    let mark = if ok { "ok  " } else { "FAIL" };
    println!("  {mark}  {name:<24} {detail}");
    i32::from(!ok)
}
