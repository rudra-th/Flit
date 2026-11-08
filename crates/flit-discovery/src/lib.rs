//! Discovery for Flit.
//!
//! Three paths, in the order the protocol tries them:
//!
//! 1. a JSON announce on `239.255.42.99:47820` every two seconds;
//! 2. a subnet scan of `GET /api/v1/info` when multicast is filtered, which is
//!    the common case on guest and corporate networks;
//! 3. a QR code or a typed address, which is also the only way across subnets.
//!
//! The crate is deliberately split along its trust boundaries:
//!
//! * `wire` is the hostile input parser. Nothing reaches a peer table without
//!   passing through it.
//! * `peers` is pure bookkeeping: three sources with three lifetimes, merged by
//!   device id, so a scan cannot make a heartbeat look stale.
//! * `iface` hides the one piece of platform code (`getifaddrs` and
//!   `GetAdaptersAddresses`) behind a value type.
//! * `multicast` and `scan` do the I/O.
//!
//! M2 status: all three paths are implemented. QR encoding is the remaining
//! item and lands with the desktop shell that displays it.

#![deny(missing_docs)]

mod iface;
mod multicast;
mod peers;
mod scan;
mod wire;

pub use flit_api::{Announce, DISCOVERY_PORT, MULTICAST_GROUP};
pub use flit_client::Probe;
pub use iface::{Interface, hosts_in, interfaces, is_private_space, scan_targets};
pub use multicast::{
    ANNOUNCE_INTERVAL, Discovery, DiscoveryConfig, DiscoveryError, REPLY_INTERVAL,
};
pub use peers::{MAX_PEERS, MULTICAST_TTL, Observed, Peer, PeerSource, PeerTable, SCAN_TTL};
pub use scan::{BUDGET, CONCURRENCY, MAX_TARGETS, PER_HOST, ScanOptions, ScanReport, scan};
pub use wire::{
    AnnounceError, MAX_ALIAS_CHARS, MAX_ANNOUNCE_BYTES, MAX_CAP_CHARS, MAX_CAPS, announce_for,
    decode, encode,
};
