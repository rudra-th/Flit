//! The table of devices seen recently.
//!
//! Discovery has three sources with very different lifetimes, and the table
//! exists to keep them from overwriting each other:
//!
//! * **multicast** is a heartbeat every two seconds, so a peer that misses
//!   three announces is gone;
//! * **scan** is a point-in-time answer with no heartbeat, so it lives for
//!   minutes rather than seconds;
//! * **manual** is a QR code or a typed address, which the user confirmed, so
//!   it never expires on its own.
//!
//! Duplicates are expected: every announce is also answered unicast, and a
//! device can appear in the multicast table and in a scan at the same time.
//! The device id is the merge key, and a known peer is refreshed in place so
//! the UI list does not churn.

use std::{
    collections::HashMap,
    net::SocketAddr,
    time::{Duration, Instant},
};

use flit_api::Announce;

/// How long a peer stays listed after its last multicast announce.
///
/// Three announce intervals: one lost datagram is normal, three in a row is a
/// device that has gone away.
pub const MULTICAST_TTL: Duration = Duration::from_secs(6);

/// How long a peer stays listed after being found by a subnet scan.
///
/// A scan is evidence that the device existed, not proof that it still does;
/// five minutes is long enough to be useful and short enough to be honest.
pub const SCAN_TTL: Duration = Duration::from_secs(300);

/// Default maximum number of peers held at once.
pub const MAX_PEERS: usize = 256;

/// Where a peer was learned from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PeerSource {
    /// A multicast announce.
    Multicast,
    /// A subnet scan over TLS.
    Scan,
    /// A QR code or an address the user typed.
    Manual,
}

impl PeerSource {
    /// How long an observation from this source stays valid.
    #[must_use]
    pub const fn ttl(self) -> Option<Duration> {
        match self {
            Self::Multicast => Some(MULTICAST_TTL),
            Self::Scan => Some(SCAN_TTL),
            Self::Manual => None,
        }
    }
}

/// One known device.
#[derive(Debug, Clone)]
pub struct Peer {
    announce: Announce,
    addr: SocketAddr,
    source: PeerSource,
    first_seen: Instant,
    last_seen: Instant,
}

impl Peer {
    /// What the device says about itself.
    #[must_use]
    pub fn announce(&self) -> &Announce {
        &self.announce
    }

    /// Where its control plane can be reached.
    #[must_use]
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// How this peer was learned.
    #[must_use]
    pub fn source(&self) -> PeerSource {
        self.source
    }

    /// When this peer was first seen in this process.
    #[must_use]
    pub fn first_seen(&self) -> Instant {
        self.first_seen
    }

    /// When this peer was last seen.
    #[must_use]
    pub fn last_seen(&self) -> Instant {
        self.last_seen
    }

    /// When this peer stops being listed, or `None` for a manual entry.
    #[must_use]
    pub fn expires_at(&self) -> Option<Instant> {
        self.source.ttl().map(|ttl| self.last_seen + ttl)
    }

    /// Whether this peer is past its lifetime at `now`.
    #[must_use]
    pub fn is_expired(&self, now: Instant) -> bool {
        self.expires_at().is_some_and(|at| now >= at)
    }
}

/// What [`PeerTable::observe`] did with an announce.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Observed {
    /// A new device entered the table. `evicted` names the device dropped to
    /// make room, if any.
    Added {
        /// Device id that lost its slot, when the table was full.
        evicted: Option<String>,
    },
    /// A known device was refreshed in place.
    Refreshed,
    /// The announce was our own, looped back by the network.
    SelfDevice,
    /// A known device id arrived with a different fingerprint.
    ///
    /// The id is derived from the fingerprint, so this is a collision or an
    /// attempt to shadow an existing device. The existing entry wins.
    Conflict {
        /// The fingerprint already in the table.
        known: String,
        /// The fingerprint that claimed the same id.
        offered: String,
    },
    /// The table was full of addresses the user typed by hand, so nothing was
    /// dropped to make room.
    Rejected,
}

/// The set of devices this process currently believes are nearby.
#[derive(Debug)]
pub struct PeerTable {
    self_id: String,
    max_peers: usize,
    peers: HashMap<String, Peer>,
}

impl PeerTable {
    /// Build a table that ignores `self_id` and holds at most [`MAX_PEERS`].
    #[must_use]
    pub fn new(self_id: impl Into<String>) -> Self {
        Self::with_capacity(self_id, MAX_PEERS)
    }

    /// Build a table with an explicit capacity. The capacity is at least one.
    #[must_use]
    pub fn with_capacity(self_id: impl Into<String>, max_peers: usize) -> Self {
        Self {
            self_id: self_id.into(),
            max_peers: max_peers.max(1),
            peers: HashMap::new(),
        }
    }

    /// Fold an announce into the table.
    ///
    /// `now` is passed in rather than read from the clock so expiry and
    /// freshness are testable without sleeping.
    pub fn observe(
        &mut self,
        announce: Announce,
        addr: SocketAddr,
        source: PeerSource,
        now: Instant,
    ) -> Observed {
        if announce.device_id == self.self_id {
            return Observed::SelfDevice;
        }

        if let Some(known) = self.peers.get_mut(&announce.device_id) {
            if known.announce.fingerprint != announce.fingerprint {
                return Observed::Conflict {
                    known: known.announce.fingerprint.clone(),
                    offered: announce.fingerprint,
                };
            }

            // A manual entry is the one the user confirmed. A multicast
            // announce from the same device refreshes it but never moves it
            // to an address the user did not approve.
            if known.source != PeerSource::Manual || source == PeerSource::Manual {
                known.addr = addr;
                known.source = source;
            }
            known.last_seen = now;
            known.announce = announce;
            return Observed::Refreshed;
        }

        let evicted = if self.peers.len() < self.max_peers {
            None
        } else {
            let victim = self.victim();
            if let Some(id) = &victim {
                self.peers.remove(id);
            }
            victim
        };

        if self.peers.len() >= self.max_peers {
            return Observed::Rejected;
        }

        self.peers.insert(
            announce.device_id.clone(),
            Peer {
                announce,
                addr,
                source,
                first_seen: now,
                last_seen: now,
            },
        );
        Observed::Added { evicted }
    }

    /// The oldest non-manual peer, which is the one evicted when the table is
    /// full. Manual entries are never evicted.
    fn victim(&self) -> Option<String> {
        self.peers
            .values()
            .filter(|peer| peer.source != PeerSource::Manual)
            .min_by_key(|peer| peer.last_seen)
            .map(|peer| peer.announce.device_id.clone())
    }

    /// Drop expired peers and return their device ids.
    pub fn prune(&mut self, now: Instant) -> Vec<String> {
        let mut gone: Vec<String> = self
            .peers
            .values()
            .filter(|peer| peer.is_expired(now))
            .map(|peer| peer.announce.device_id.clone())
            .collect();
        for id in &gone {
            self.peers.remove(id);
        }
        gone.sort();
        gone
    }

    /// Look a device up by id.
    #[must_use]
    pub fn get(&self, device_id: &str) -> Option<&Peer> {
        self.peers.get(device_id)
    }

    /// The id of the device this table belongs to.
    #[must_use]
    pub fn self_id(&self) -> &str {
        &self.self_id
    }

    /// Whether `device_id` was seen within `within` and is still live.
    ///
    /// This is the reply-suppression rule: an announce is answered unicast
    /// only when the sender is not already a fresh neighbour, which keeps two
    /// devices from answering each other forever.
    #[must_use]
    pub fn seen_recently(&self, device_id: &str, now: Instant, within: Duration) -> bool {
        self.peers.get(device_id).is_some_and(|peer| {
            !peer.is_expired(now) && now.saturating_duration_since(peer.last_seen) < within
        })
    }

    /// Number of peers currently listed.
    #[must_use]
    pub fn len(&self) -> usize {
        self.peers.len()
    }

    /// Whether the table is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.peers.is_empty()
    }

    /// Every live peer, newest first, with a stable tie-break.
    ///
    /// Ordering is by `last_seen` descending so a device that is answering
    /// right now sorts above one that is about to expire, then by alias and id
    /// so two peers seen in the same instant never swap places between frames.
    #[must_use]
    pub fn snapshot(&self, now: Instant) -> Vec<&Peer> {
        let mut live: Vec<&Peer> = self
            .peers
            .values()
            .filter(|peer| !peer.is_expired(now))
            .collect();
        live.sort_by(|a, b| {
            b.last_seen
                .cmp(&a.last_seen)
                .then_with(|| a.announce.alias.cmp(&b.announce.alias))
                .then_with(|| a.announce.device_id.cmp(&b.announce.device_id))
        });
        live
    }
}

impl Default for PeerTable {
    fn default() -> Self {
        Self::new("")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flit_api::{DeviceType, PROTO_VERSION, TLS_PORT};

    fn announce(id_byte: u8, alias: &str) -> Announce {
        let fingerprint = format!("{:02x}", id_byte).repeat(32);
        Announce {
            proto: PROTO_VERSION,
            alias: alias.into(),
            device_id: fingerprint[..16].to_string(),
            fingerprint,
            device_type: DeviceType::Laptop,
            port: TLS_PORT,
            caps: vec!["resume".into()],
            pin_required: false,
        }
    }

    fn addr(last: u8) -> SocketAddr {
        SocketAddr::from(([192, 168, 1, last], TLS_PORT))
    }

    #[test]
    fn a_new_device_is_added_once_and_then_refreshed() {
        let now = Instant::now();
        let mut table = PeerTable::new("self");
        let a = announce(1, "Desk");

        assert_eq!(
            table.observe(a.clone(), addr(10), PeerSource::Multicast, now),
            Observed::Added { evicted: None }
        );
        assert_eq!(
            table.observe(
                a.clone(),
                addr(11),
                PeerSource::Multicast,
                now + Duration::from_secs(2)
            ),
            Observed::Refreshed
        );
        assert_eq!(table.len(), 1);
        assert_eq!(table.get(&a.device_id).unwrap().addr(), addr(11));
    }

    #[test]
    fn our_own_announce_is_ignored() {
        let now = Instant::now();
        let a = announce(1, "Self");
        let mut table = PeerTable::new(a.device_id.clone());
        assert_eq!(
            table.observe(a, addr(10), PeerSource::Multicast, now),
            Observed::SelfDevice
        );
        assert!(table.is_empty());
    }

    #[test]
    fn a_second_key_claiming_a_known_id_does_not_replace_it() {
        let now = Instant::now();
        let mut table = PeerTable::new("self");
        let first = announce(1, "Desk");
        table.observe(first.clone(), addr(10), PeerSource::Multicast, now);

        let mut impostor = first.clone();
        impostor.fingerprint = "ff".repeat(32);
        impostor.alias = "Evil".into();
        let outcome = table.observe(impostor, addr(66), PeerSource::Multicast, now);
        assert!(matches!(outcome, Observed::Conflict { .. }));
        assert_eq!(
            table.get(&first.device_id).unwrap().announce().alias,
            "Desk"
        );
        assert_eq!(table.get(&first.device_id).unwrap().addr(), addr(10));
    }

    #[test]
    fn multicast_peers_expire_after_three_missed_announces() {
        let now = Instant::now();
        let mut table = PeerTable::new("self");
        let a = announce(1, "Desk");
        table.observe(a.clone(), addr(10), PeerSource::Multicast, now);

        assert!(
            table
                .snapshot(now + MULTICAST_TTL - Duration::from_millis(1))
                .len()
                == 1
        );
        assert!(table.snapshot(now + MULTICAST_TTL).is_empty());
        assert_eq!(table.prune(now + MULTICAST_TTL), vec![a.device_id]);
        assert!(table.is_empty());
    }

    #[test]
    fn a_scan_result_lives_longer_than_an_announce() {
        let now = Instant::now();
        let mut table = PeerTable::new("self");
        table.observe(announce(1, "Desk"), addr(10), PeerSource::Scan, now);
        assert_eq!(table.snapshot(now + MULTICAST_TTL * 2).len(), 1);
        assert!(table.snapshot(now + SCAN_TTL).is_empty());
    }

    #[test]
    fn a_manual_entry_never_expires_and_never_moves() {
        let now = Instant::now();
        let mut table = PeerTable::new("self");
        let a = announce(1, "Desk");
        table.observe(a.clone(), addr(10), PeerSource::Manual, now);

        table.observe(
            a.clone(),
            addr(99),
            PeerSource::Multicast,
            now + Duration::from_secs(1),
        );
        let peer = table.get(&a.device_id).unwrap();
        assert_eq!(peer.addr(), addr(10));
        assert_eq!(peer.source(), PeerSource::Manual);

        assert_eq!(
            table.prune(now + Duration::from_secs(86_400)),
            Vec::<String>::new()
        );
        assert_eq!(table.snapshot(now + Duration::from_secs(86_400)).len(), 1);
    }

    #[test]
    fn a_full_table_evicts_the_oldest_multicast_peer() {
        let now = Instant::now();
        let mut table = PeerTable::with_capacity("self", 2);
        let old = announce(1, "Old");
        let new = announce(2, "New");
        let newest = announce(3, "Newest");

        table.observe(old.clone(), addr(10), PeerSource::Multicast, now);
        table.observe(
            new.clone(),
            addr(11),
            PeerSource::Multicast,
            now + Duration::from_secs(1),
        );
        let outcome = table.observe(
            newest.clone(),
            addr(12),
            PeerSource::Multicast,
            now + Duration::from_secs(2),
        );
        assert_eq!(
            outcome,
            Observed::Added {
                evicted: Some(old.device_id.clone())
            }
        );
        assert!(table.get(&old.device_id).is_none());
        assert!(table.get(&new.device_id).is_some());
    }

    #[test]
    fn a_full_table_of_manual_entries_rejects_newcomers() {
        let now = Instant::now();
        let mut table = PeerTable::with_capacity("self", 1);
        table.observe(announce(1, "Typed"), addr(10), PeerSource::Manual, now);
        let outcome = table.observe(announce(2, "Beacon"), addr(11), PeerSource::Multicast, now);
        assert_eq!(outcome, Observed::Rejected);
        assert_eq!(table.len(), 1);
    }

    #[test]
    fn reply_suppression_only_treats_fresh_neighbours_as_fresh() {
        let now = Instant::now();
        let mut table = PeerTable::new("self");
        let a = announce(1, "Desk");
        rule_check(&mut table, &a, now);
    }

    fn rule_check(table: &mut PeerTable, a: &Announce, now: Instant) {
        table.observe(a.clone(), addr(10), PeerSource::Multicast, now);
        assert!(table.seen_recently(&a.device_id, now, Duration::from_secs(10)));
        assert!(!table.seen_recently(
            &a.device_id,
            now + Duration::from_secs(11),
            Duration::from_secs(10)
        ));
        assert!(!table.seen_recently("someone-else", now, Duration::from_secs(10)));
    }

    #[test]
    fn snapshots_are_stable_for_equal_timestamps() {
        let now = Instant::now();
        let mut table = PeerTable::new("self");
        table.observe(announce(2, "Beta"), addr(10), PeerSource::Multicast, now);
        table.observe(announce(1, "Alpha"), addr(11), PeerSource::Multicast, now);
        let aliases: Vec<&str> = table
            .snapshot(now)
            .iter()
            .map(|p| p.announce().alias.as_str())
            .collect();
        assert_eq!(aliases, vec!["Alpha", "Beta"]);
    }
}
