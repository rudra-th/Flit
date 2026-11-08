//! The fallback path end to end: a real TLS server, a real probe.
//!
//! These tests are the reason `flit-client::probe` exists. If a scan cannot
//! find a Flit server that is listening on a known address, the feature that
//! makes blocked multicast a non-event does not work at all.

use std::{
    net::{Ipv4Addr, SocketAddr},
    time::{Duration, Instant},
};

use flit_crypto::Identity;
use flit_discovery::{ScanOptions, scan};
use flit_server::{Server, ServerConfig};

/// Start a receiver on an ephemeral port and keep it accepting in the
/// background.
async fn start_server() -> (String, String, SocketAddr, tokio::task::JoinHandle<()>) {
    let identity = Identity::from_secret_bytes([9u8; 32]);
    let config = ServerConfig::default().with_bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)));
    let server = Server::bind(&identity, config).await.expect("bind");
    let addr = server.local_addr();
    let task = tokio::spawn(async move {
        let _ = server.run_until(std::future::pending()).await;
    });
    (identity.device_id(), identity.fingerprint(), addr, task)
}

#[tokio::test]
async fn a_scan_finds_a_server_on_a_known_address() {
    let (device_id, fingerprint, addr, server) = start_server().await;

    let started = Instant::now();
    let report = scan(
        vec![Ipv4Addr::LOCALHOST],
        addr.port(),
        "not-this-device",
        &ScanOptions::default(),
    )
    .await;
    let elapsed = started.elapsed();
    server.abort();

    assert_eq!(report.targets, 1);
    assert_eq!(report.probed, 1);
    assert_eq!(report.found.len(), 1, "{report:?}");
    let found = &report.found[0];
    assert_eq!(found.info.device_id, device_id);
    assert_eq!(found.fingerprint, fingerprint);
    assert_eq!(found.info.fingerprint, fingerprint);
    assert!(!found.info.alias.is_empty());
    // One host with an answer must not wait for the per-host timeout.
    assert!(elapsed < Duration::from_secs(2), "scan took {elapsed:?}");
}

#[tokio::test]
async fn a_scan_does_not_report_the_scanning_device_to_itself() {
    let (device_id, _fingerprint, addr, server) = start_server().await;

    let report = scan(
        vec![Ipv4Addr::LOCALHOST],
        addr.port(),
        &device_id,
        &ScanOptions::default(),
    )
    .await;
    server.abort();

    assert_eq!(report.probed, 1);
    assert!(report.found.is_empty());
}

#[tokio::test]
async fn a_closed_port_is_a_miss_not_a_hang() {
    // Bind, learn the port, and drop the socket: nothing is listening.
    let probe_socket = std::net::UdpSocket::bind("127.0.0.1:0").expect("bind");
    let closed = probe_socket.local_addr().expect("local addr").port();
    drop(probe_socket);

    let started = Instant::now();
    let report = scan(
        vec![Ipv4Addr::LOCALHOST],
        closed,
        "self",
        &ScanOptions {
            per_host: Duration::from_millis(300),
            ..ScanOptions::default()
        },
    )
    .await;
    let elapsed = started.elapsed();

    assert!(report.found.is_empty());
    assert_eq!(report.probed, 1);
    assert!(elapsed < Duration::from_secs(2), "miss took {elapsed:?}");
}
