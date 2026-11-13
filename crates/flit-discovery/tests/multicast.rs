//! Two discovery instances on one machine, which is the smallest possible
//! multicast network.
//!
//! The loopback flag is on by default precisely so this test can exist without
//! a second computer. If the test interface cannot carry multicast, the test
//! says so instead of pretending discovery works — with one exception: GitHub's
//! macOS runners drop multicast between two sockets on the same host, so there
//! the test skips loudly rather than failing a build over a sandbox rule.

use std::time::{Duration, Instant};

use flit_api::{Announce, DeviceType, PROTO_VERSION, TLS_PORT};
use flit_discovery::{Discovery, DiscoveryConfig, PeerSource, interfaces};

fn announce(seed: u8, alias: &str) -> Announce {
    let fingerprint = hex_seed(seed);
    Announce {
        proto: PROTO_VERSION,
        alias: alias.into(),
        device_id: fingerprint[..16].to_string(),
        fingerprint,
        device_type: DeviceType::Desktop,
        port: TLS_PORT,
        caps: vec!["resume".into()],
        pin_required: false,
    }
}

fn hex_seed(seed: u8) -> String {
    format!("{seed:02x}").repeat(32)
}

/// A UDP port that is free right now. The socket is closed before the port is
/// used, so this is a heuristic, not a reservation.
fn free_udp_port() -> u16 {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").expect("bind");
    socket.local_addr().expect("local addr").port()
}

async fn wait_for(both: impl Fn() -> bool, limit: Duration) -> bool {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        if both() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    both()
}

/// Whether this host is known to drop looped-back multicast between two sockets.
///
/// Only the CI runner is excused, and only for that reason: a developer machine
/// that filters multicast is a real discovery problem and must still fail.
fn multicast_is_filtered_here() -> bool {
    cfg!(target_os = "macos") && std::env::var_os("CI").is_some()
}

/// Report "this environment cannot show this" instead of a red build, but only
/// where the environment is the known cause.
fn skip_filtered_multicast(what: &str) -> bool {
    if multicast_is_filtered_here() {
        eprintln!("skipping: this CI host filters multicast ({what})");
        return true;
    }
    false
}

#[tokio::test(flavor = "multi_thread")]
async fn two_instances_on_one_machine_find_each_other() {
    let hardware = interfaces();
    if hardware.is_empty() {
        eprintln!("skipping: no interface to join");
        return;
    }

    let port = free_udp_port();
    let config = DiscoveryConfig::default()
        .with_port(port)
        .with_interfaces(hardware);
    let first_announce = announce(0x11, "First");
    let second_announce = announce(0x22, "Second");
    let first_id = first_announce.device_id.clone();
    let second_id = second_announce.device_id.clone();

    let first = Discovery::start(first_announce, config.clone())
        .await
        .expect("first discovery");
    let second = Discovery::start(second_announce, config)
        .await
        .expect("second discovery");

    let first_table = first.table();
    let second_table = second.table();

    let found = wait_for(
        || {
            let a = first_table.try_read().map(|t| t.get(&second_id).is_some());
            let b = second_table.try_read().map(|t| t.get(&first_id).is_some());
            a.unwrap_or(false) && b.unwrap_or(false)
        },
        Duration::from_secs(6),
    )
    .await;

    if !found {
        let a_len = first_table.read().await.len();
        let b_len = second_table.read().await.len();
        if skip_filtered_multicast("no delivery in 6s") {
            return;
        }
        panic!(
            "multicast did not deliver within 6s (tables: {a_len} and {b_len}); \
             this environment filters multicast"
        );
    }

    let peer = first_table.read().await;
    let peer = peer.get(&second_id).expect("second device");
    assert_eq!(peer.announce().alias, "Second");
    assert_eq!(peer.addr().port(), TLS_PORT);
    // The announce came in over the network, so it is a multicast peer.
    assert!(matches!(peer.source(), PeerSource::Multicast));
    assert_eq!(peer.announce().device_id.len(), 16);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_silent_instance_does_not_become_a_peer() {
    let hardware = interfaces();
    if hardware.is_empty() {
        eprintln!("skipping: no interface to join");
        return;
    }

    let port = free_udp_port();
    let talking = Discovery::start(
        announce(0x33, "Talking"),
        DiscoveryConfig::default()
            .with_port(port)
            .with_interfaces(hardware.clone()),
    )
    .await
    .expect("talking discovery");

    let silent_announce = announce(0x44, "Silent");
    let silent_id = silent_announce.device_id.clone();
    let silent = Discovery::start(
        silent_announce,
        DiscoveryConfig::default()
            .with_port(port)
            .with_interfaces(hardware)
            .without_announcing(),
    )
    .await
    .expect("silent discovery");

    let table = silent.table();
    let heard = wait_for(
        || table.try_read().map(|t| !t.is_empty()).unwrap_or(false),
        Duration::from_secs(6),
    )
    .await;
    if !heard && skip_filtered_multicast("the silent instance heard nothing") {
        return;
    }
    assert!(heard, "the silent instance never heard the announcing one");

    let talking_table = talking.table();
    // Give the announcing side a few announce intervals to prove it never
    // learned about the silent one.
    tokio::time::sleep(Duration::from_secs(3)).await;
    let learned = talking_table.read().await.get(&silent_id).is_some();
    assert!(!learned, "a silent instance was still discovered");
}
