//! End-to-end checks for the M1 transport: a real TLS listener, a real client,
//! and the pinning rules that decide whether the two may talk.

use std::{
    net::SocketAddr,
    time::{Duration, Instant},
};

use flit_client::{Client, ClientError};
use flit_crypto::Identity;
use flit_server::{Server, ServerConfig};

/// Start a server on loopback with an ephemeral port.
async fn start(identity: &Identity, alias: &str) -> (Server, SocketAddr) {
    let config = ServerConfig::default()
        .with_alias(alias)
        .with_device_type(flit_api::DeviceType::Laptop)
        .with_bind(SocketAddr::from(([127, 0, 0, 1], 0)));
    let server = Server::bind(identity, config).await.expect("bind");
    let addr = server.local_addr();
    (server, addr)
}

/// Run a server until `stop` is triggered.
fn spawn(
    server: Server,
) -> (
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
) {
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let handle = tokio::spawn(async move {
        server
            .run_until(async move {
                let _ = stopped.await;
            })
            .await
            .expect("server run");
    });
    (stop, handle)
}

#[tokio::test]
async fn a_pinned_client_reads_info_from_the_right_peer() {
    let identity = Identity::from_secret_bytes([11u8; 32]);
    let (server, addr) = start(&identity, "Desk").await;
    let (stop, handle) = spawn(server);

    let client = Client::new(addr, &identity.fingerprint()).expect("client");
    let mut connection = client.connect().await.expect("pinned dial");
    let info = connection.info().await.expect("info");

    assert_eq!(info.alias, "Desk");
    assert_eq!(info.fingerprint, identity.fingerprint());
    assert_eq!(info.device_id, identity.device_id());
    assert_eq!(info.port, addr.port());
    assert_eq!(info.proto, flit_api::PROTO_VERSION);
    connection.close();

    let _ = stop.send(());
    handle.await.expect("server task");
}

#[tokio::test]
async fn the_account_sent_key_works_too() {
    let identity = Identity::from_secret_bytes([12u8; 32]);
    let (server, addr) = start(&identity, "Desk").await;
    let (stop, handle) = spawn(server);

    let upper = identity.fingerprint().to_uppercase();
    let client = Client::new(addr, &upper).expect("client");
    let info = client
        .connect()
        .await
        .expect("pinned dial")
        .info()
        .await
        .expect("info");
    assert_eq!(info.fingerprint, identity.fingerprint());

    let _ = stop.send(());
    handle.await.expect("server task");
}

#[tokio::test]
async fn a_different_key_is_refused_at_the_handshake() {
    let identity = Identity::from_secret_bytes([13u8; 32]);
    let (server, addr) = start(&identity, "Desk").await;
    let (stop, handle) = spawn(server);

    // The same address, but the fingerprint of a device that is not answering.
    let other = Identity::from_secret_bytes([14u8; 32]);
    let client = Client::new(addr, &other.fingerprint()).expect("client");
    let err = client.connect().await.expect_err("must not connect");

    match err {
        ClientError::PinMismatch {
            expected,
            presented,
        } => {
            assert_eq!(expected, other.fingerprint());
            assert_eq!(presented, identity.fingerprint());
        }
        other => panic!("expected a pin mismatch, got {other:?}"),
    }

    let _ = stop.send(());
    handle.await.expect("server task");
}

#[tokio::test]
async fn an_unreachable_peer_fails_without_a_pin_mismatch() {
    // Port 1 on loopback is privileged, so no test in this suite can ever be
    // listening on it: the outcome is deterministic "refused", not a race with
    // a sibling test that just released an ephemeral port.
    let addr = SocketAddr::from(([127, 0, 0, 1], 1));

    let client = Client::new(addr, &"0".repeat(64)).expect("client");
    let err = client.connect().await.expect_err("must not connect");
    assert!(
        matches!(err, ClientError::Connect { .. } | ClientError::Handshake(_)),
        "{err:?}"
    );
}

#[tokio::test]
async fn a_peer_that_stops_answering_does_not_hang_the_accept_loop() {
    let identity = Identity::from_secret_bytes([15u8; 32]);
    let (server, addr) = start(&identity, "Desk").await;
    let (stop, handle) = spawn(server);

    // A client that opens the handshake and walks away.
    let _half_open = tokio::net::TcpStream::connect(addr).await.unwrap();
    drop(_half_open);

    // The listener is still healthy afterwards.
    let client = Client::new(addr, &identity.fingerprint()).expect("client");
    let info = tokio::time::timeout(Duration::from_secs(5), async {
        client.connect().await?.info().await
    })
    .await
    .expect("server must still answer")
    .expect("info");
    assert_eq!(info.alias, "Desk");

    let _ = stop.send(());
    handle.await.expect("server task");
}

#[tokio::test]
async fn graceful_shutdown_stops_accepting() {
    let identity = Identity::from_secret_bytes([16u8; 32]);
    let (server, addr) = start(&identity, "Desk").await;
    let (stop, handle) = spawn(server);

    let client = Client::new(addr, &identity.fingerprint()).expect("client");
    assert!(client.connect().await.is_ok());

    let started = Instant::now();
    let _ = stop.send(());
    tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("server must stop promptly")
        .expect("server task");
    assert!(started.elapsed() < Duration::from_secs(5));
}
