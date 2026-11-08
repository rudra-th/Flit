//! Multicast discovery: the heartbeat, and the socket that carries it.
//!
//! One socket per process is enough. Android requires a multicast lock, and a
//! laptop needs a join per interface; both are expressed as socket options on
//! the same socket, so receive does not have to fan in from four sockets.
//!
//! Two rules keep this from becoming a broadcast storm:
//!
//! 1. An announce is answered **unicast**, not multicast.
//! 2. A reply is only sent when the sender was not already a fresh neighbour.
//!
//! Replies are ordinary announces with no distinguishing field, so rule 2 is
//! what makes the protocol converge: A announces, B replies, A sees B for the
//! first time and would reply, but A's table already knows B, so it does not.
//! Without the rule the two devices answer each other forever.

use std::{
    io,
    net::{Ipv4Addr, SocketAddr},
    sync::Arc,
    time::{Duration, Instant},
};

use flit_api::{Announce, DISCOVERY_PORT, MULTICAST_GROUP};
use socket2::{Domain, Protocol, SockRef, Socket, Type};
use tokio::{net::UdpSocket, sync::RwLock, task::JoinHandle};

use crate::{
    iface::{Interface, interfaces, scan_targets},
    peers::{MAX_PEERS, PeerSource, PeerTable},
    scan::{MAX_TARGETS, ScanOptions, ScanReport, scan},
    wire::{self, AnnounceError, MAX_ANNOUNCE_BYTES},
};

/// How often a device announces itself.
pub const ANNOUNCE_INTERVAL: Duration = Duration::from_secs(2);

/// How recently a device must have been seen for its announce not to be
/// answered.
pub const REPLY_INTERVAL: Duration = Duration::from_secs(10);

/// How long a receive waits before checking whether the task was cancelled.
const RECV_IDLE: Duration = Duration::from_secs(5);

/// Everything that can stop discovery from starting.
#[derive(Debug, thiserror::Error)]
pub enum DiscoveryError {
    /// The announce could not be encoded, usually because it is malformed.
    #[error(transparent)]
    Announce(#[from] AnnounceError),
    /// No socket could be opened or joined.
    #[error("could not open the discovery socket: {0}")]
    Socket(#[source] io::Error),
}

/// How discovery should behave. The defaults are the protocol.
#[derive(Debug, Clone)]
pub struct DiscoveryConfig {
    /// UDP port to bind. The default is the discovery port.
    pub port: u16,
    /// Interfaces to use. `None` asks the operating system.
    pub interfaces: Option<Vec<Interface>>,
    /// Whether to announce ourselves.
    ///
    /// A discover-only client leaves this false: advertising a control port
    /// that nothing is listening on would send peers to a closed door.
    pub announce: bool,
    /// How often to announce.
    pub announce_interval: Duration,
    /// How recently a peer must have been seen to suppress a reply.
    pub reply_interval: Duration,
    /// Whether outgoing multicasts are looped back to this host.
    ///
    /// On by default: it lets two instances on one machine see each other,
    /// which is how discovery is tested and how a desktop and a phone emulator
    /// coexist on a developer's laptop.
    pub loopback: bool,
    /// Maximum peers held in the table.
    pub max_peers: usize,
    /// Scan tuning, used by [`Discovery::scan`].
    pub scan: ScanOptions,
}

impl Default for DiscoveryConfig {
    fn default() -> Self {
        Self {
            port: DISCOVERY_PORT,
            interfaces: None,
            announce: true,
            announce_interval: ANNOUNCE_INTERVAL,
            reply_interval: REPLY_INTERVAL,
            loopback: true,
            max_peers: MAX_PEERS,
            scan: ScanOptions::default(),
        }
    }
}

impl DiscoveryConfig {
    /// Use exactly these interfaces instead of asking the operating system.
    #[must_use]
    pub fn with_interfaces(mut self, interfaces: Vec<Interface>) -> Self {
        self.interfaces = Some(interfaces);
        self
    }

    /// Bind a different UDP port, for tests and for a machine that already
    /// has something on 47820.
    #[must_use]
    pub fn with_port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }

    /// Listen and scan, but do not advertise this device.
    #[must_use]
    pub fn without_announcing(mut self) -> Self {
        self.announce = false;
        self
    }
}

/// A bound, joined, sending-and-receiving discovery socket.
struct Wire {
    socket: UdpSocket,
    interfaces: Vec<Ipv4Addr>,
    group: SocketAddr,
}

impl Wire {
    /// Open the socket and join the group on every interface.
    fn bind(config: &DiscoveryConfig) -> io::Result<Self> {
        let group = Ipv4Addr::from(MULTICAST_GROUP);
        let chosen = config.interfaces.clone().unwrap_or_else(interfaces);
        let joins: Vec<Ipv4Addr> = if chosen.is_empty() {
            // No interface information: let the kernel pick the default route
            // rather than refusing to announce at all.
            vec![Ipv4Addr::UNSPECIFIED]
        } else {
            chosen.iter().map(|interface| interface.addr()).collect()
        };

        let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
        // Two Flit instances on one host, and a desktop app next to the CLI,
        // must both be able to listen. This only shares the discovery port;
        // the control plane is a TCP connection and never uses this flag.
        socket.set_reuse_address(true)?;
        #[cfg(unix)]
        socket.set_reuse_port(true)?;
        socket.set_nonblocking(true)?;
        socket.bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, config.port)).into())?;
        socket.set_multicast_ttl_v4(1)?;
        socket.set_multicast_loop_v4(config.loopback)?;
        for addr in &joins {
            socket.join_multicast_v4(&group, addr)?;
        }

        let std_socket: std::net::UdpSocket = socket.into();
        Ok(Self {
            socket: UdpSocket::from_std(std_socket)?,
            interfaces: joins,
            group: SocketAddr::from((group, config.port)),
        })
    }

    /// Send one datagram on every joined interface.
    async fn send(&self, payload: &[u8]) -> io::Result<usize> {
        let mut sent = 0;
        for addr in &self.interfaces {
            // The multicast interface is a socket-wide option, so it is set
            // immediately before the send that needs it. Sends from this
            // process are serialised by the single announcer task.
            if let Err(err) = SockRef::from(&self.socket).set_multicast_if_v4(addr) {
                log::debug!("discovery: cannot select interface {addr}: {err}");
                continue;
            }
            match self.socket.send_to(payload, self.group).await {
                Ok(_) => sent += 1,
                Err(err) => log::debug!("discovery: send on {addr} failed: {err}"),
            }
        }
        if sent == 0 {
            return Err(io::Error::other(
                "no interface accepted the discovery announce",
            ));
        }
        Ok(sent)
    }

    /// Send one datagram to a single address, used for the unicast reply.
    async fn send_to(&self, payload: &[u8], to: SocketAddr) -> io::Result<usize> {
        self.socket.send_to(payload, to).await
    }

    /// Receive one datagram.
    async fn recv(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        self.socket.recv_from(buf).await
    }
}

/// A running discovery service.
///
/// Dropping this stops announcing and listening. That is the intended way to
/// shut down: there is no flag to forget to set.
pub struct Discovery {
    table: Arc<RwLock<PeerTable>>,
    wire: Arc<Wire>,
    payload: Arc<Vec<u8>>,
    interfaces: Vec<Interface>,
    control_port: u16,
    scan_options: ScanOptions,
    reply_interval: Duration,
    tasks: Vec<JoinHandle<()>>,
}

impl Discovery {
    /// Start announcing `announce` and listening for peers.
    pub async fn start(
        announce: Announce,
        config: DiscoveryConfig,
    ) -> Result<Self, DiscoveryError> {
        let payload = Arc::new(wire::encode(&announce)?);
        let wire = Arc::new(Wire::bind(&config).map_err(DiscoveryError::Socket)?);
        let chosen = config.interfaces.clone().unwrap_or_else(interfaces);
        let table = Arc::new(RwLock::new(PeerTable::with_capacity(
            announce.device_id.clone(),
            config.max_peers,
        )));

        let mut tasks = Vec::new();
        if config.announce {
            let wire = Arc::clone(&wire);
            let payload = Arc::clone(&payload);
            let interval = config.announce_interval;
            tasks.push(tokio::spawn(async move {
                loop {
                    if let Err(err) = wire.send(&payload).await {
                        log::debug!("discovery: announce failed: {err}");
                    }
                    tokio::time::sleep(interval).await;
                }
            }));
        }

        let listener = {
            let wire = Arc::clone(&wire);
            let payload = Arc::clone(&payload);
            let table = Arc::clone(&table);
            let reply_interval = config.reply_interval;
            let own_id = announce.device_id.clone();
            let replies = config.announce;
            tokio::spawn(async move {
                let mut buf = vec![0u8; MAX_ANNOUNCE_BYTES];
                loop {
                    let received = match tokio::time::timeout(RECV_IDLE, wire.recv(&mut buf)).await
                    {
                        Ok(result) => result,
                        // Idle tick: loop back around so a dropped task ends.
                        Err(_) => continue,
                    };
                    let (len, from) = match received {
                        Ok(pair) => pair,
                        Err(err) => {
                            log::warn!("discovery: receive failed: {err}");
                            tokio::time::sleep(Duration::from_secs(1)).await;
                            continue;
                        }
                    };

                    let announce = match wire::decode(&buf[..len]) {
                        Ok(announce) => announce,
                        Err(err) => {
                            log::debug!("discovery: dropped a datagram from {from}: {err}");
                            continue;
                        }
                    };
                    if announce.device_id == own_id {
                        continue;
                    }

                    let now = Instant::now();
                    let fresh =
                        table
                            .read()
                            .await
                            .seen_recently(&announce.device_id, now, reply_interval);
                    // The control plane is on the advertised port, not on the
                    // port the datagram came from.
                    let control = SocketAddr::from((from.ip(), announce.port));
                    let observed =
                        table
                            .write()
                            .await
                            .observe(announce, control, PeerSource::Multicast, now);

                    if replies && !fresh {
                        match wire.send_to(&payload, from).await {
                            Ok(_) => {}
                            Err(err) => log::debug!("discovery: reply to {from} failed: {err}"),
                        }
                    }
                    log::trace!("discovery: {observed:?} from {from}");
                }
            })
        };
        tasks.push(listener);

        let discovery = Self {
            table,
            wire,
            payload,
            interfaces: chosen,
            control_port: announce.port,
            scan_options: config.scan,
            reply_interval: config.reply_interval,
            tasks,
        };

        // Announce immediately: waiting two seconds to be visible is exactly
        // the delay the product is trying to avoid.
        if config.announce {
            if let Err(err) = discovery.announce_once().await {
                log::debug!("discovery: first announce failed: {err}");
            }
        }

        Ok(discovery)
    }

    /// The peer table, for a UI or a CLI that wants to read it.
    #[must_use]
    pub fn table(&self) -> Arc<RwLock<PeerTable>> {
        Arc::clone(&self.table)
    }

    /// Send one announce now instead of waiting for the next interval.
    pub async fn announce_once(&self) -> io::Result<usize> {
        self.wire.send(&self.payload).await
    }

    /// The encoded announce, which is also what a unicast reply carries.
    #[must_use]
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    /// How often a repeat announce is suppressed.
    #[must_use]
    pub fn reply_interval(&self) -> Duration {
        self.reply_interval
    }

    /// Scan this machine's subnets and fold the results into the table.
    ///
    /// The scan runs while the announcer keeps ticking; both write to the same
    /// table, and the merge rules make the order irrelevant.
    pub async fn scan(&self) -> ScanReport {
        let local: Vec<Ipv4Addr> = self
            .interfaces
            .iter()
            .map(|interface| interface.addr())
            .collect();
        let targets = scan_targets(&self.interfaces, &local, MAX_TARGETS);
        let port = self.control_port;
        let own_id = self.table.read().await.self_id().to_string();
        let report = scan(targets, port, &own_id, &self.scan_options).await;

        let now = Instant::now();
        let mut table = self.table.write().await;
        for found in &report.found {
            let addr = SocketAddr::from((found.addr.ip(), found.info.port));
            table.observe(wire::announce_for(&found.info), addr, PeerSource::Scan, now);
        }
        report
    }
}

impl Drop for Discovery {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

impl std::fmt::Debug for Discovery {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Discovery")
            .field("interfaces", &self.interfaces)
            .field("port", &self.wire.group.port())
            .finish_non_exhaustive()
    }
}
