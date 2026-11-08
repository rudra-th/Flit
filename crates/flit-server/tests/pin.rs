//! PIN enforcement, end to end: a real listener, a real client, and the rule
//! that a PIN gates every endpoint except `/info`.

use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
};

use flit_api::DeviceType;
use flit_client::{Client, ClientError};
use flit_crypto::Identity;
use flit_server::{MAX_ATTEMPTS, Server, ServerConfig, ServerError};

const PIN: &str = "482913";

/// A scratch directory that does not need a temp-dir crate.
fn scratch(name: &str) -> PathBuf {
    let unique = format!(
        "flit-pin-{}-{}-{name}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let path = std::env::temp_dir().join(unique);
    std::fs::create_dir_all(&path).unwrap();
    path
}

/// Start a receiver with `pin`, or without one when `pin` is `None`.
async fn start(
    identity: &Identity,
    root: &Path,
    pin: Option<&str>,
) -> Result<(Server, SocketAddr), ServerError> {
    let mut config = ServerConfig::default()
        .with_alias("Desk")
        .with_device_type(DeviceType::Laptop)
        .with_bind(SocketAddr::from(([127, 0, 0, 1], 0)))
        .with_root(root);
    if let Some(pin) = pin {
        config = config.with_pin(pin);
    }
    let server = Server::bind(identity, config).await?;
    let addr = server.local_addr();
    Ok((server, addr))
}

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

/// The HTTP status and body of a failure, as the client reports it.
fn status_of(err: &ClientError) -> Option<(u16, &str)> {
    match err {
        ClientError::Status { status, body } => Some((*status, body.as_str())),
        _ => None,
    }
}

#[tokio::test]
async fn a_receiver_with_a_pin_refuses_an_unpinned_sender() {
    let identity = Identity::from_secret_bytes([31u8; 32]);
    let root = scratch("unpinned");
    let source_dir = scratch("unpinned-src");
    let source = source_dir.join("secret.txt");
    std::fs::write(&source, b"not for you").unwrap();

    let (server, addr) = start(&identity, &root, Some(PIN)).await.unwrap();
    let (stop, handle) = spawn(server);

    let client = Client::new(addr, &identity.fingerprint()).unwrap();
    let mut connection = client.connect().await.unwrap();
    let err = connection
        .send_file(&source, None)
        .await
        .expect_err("a PIN is required");
    connection.close();

    let (status, body) = status_of(&err).expect("an HTTP error, not a transport one");
    assert_eq!(status, 403, "{body}");
    assert!(body.contains("pin_required"), "{body}");
    assert!(!root.join("secret.txt").exists(), "nothing may be written");

    let _ = stop.send(());
    handle.await.unwrap();
    std::fs::remove_dir_all(&root).ok();
    std::fs::remove_dir_all(&source_dir).ok();
}

#[tokio::test]
async fn the_right_pin_lets_a_file_through() {
    let identity = Identity::from_secret_bytes([32u8; 32]);
    let root = scratch("right");
    let source_dir = scratch("right-src");
    let payload = b"with the pin, thank you";
    let source = source_dir.join("note.txt");
    std::fs::write(&source, payload).unwrap();

    let (server, addr) = start(&identity, &root, Some(PIN)).await.unwrap();
    let (stop, handle) = spawn(server);

    let client = Client::new(addr, &identity.fingerprint())
        .unwrap()
        .with_pin(PIN);
    let mut connection = client.connect().await.unwrap();
    connection.send_file(&source, None).await.expect("send");
    connection.close();

    assert_eq!(std::fs::read(root.join("note.txt")).unwrap(), payload);

    let _ = stop.send(());
    handle.await.unwrap();
    std::fs::remove_dir_all(&root).ok();
    std::fs::remove_dir_all(&source_dir).ok();
}

#[tokio::test]
async fn a_wrong_pin_is_refused_and_writes_nothing() {
    let identity = Identity::from_secret_bytes([33u8; 32]);
    let root = scratch("wrong");
    let source_dir = scratch("wrong-src");
    let source = source_dir.join("note.txt");
    std::fs::write(&source, b"nope").unwrap();

    let (server, addr) = start(&identity, &root, Some(PIN)).await.unwrap();
    let (stop, handle) = spawn(server);

    let client = Client::new(addr, &identity.fingerprint())
        .unwrap()
        .with_pin("000000");
    let mut connection = client.connect().await.unwrap();
    let err = connection
        .send_file(&source, None)
        .await
        .expect_err("a wrong PIN must not open a session");
    connection.close();

    let (status, body) = status_of(&err).expect("an HTTP error");
    assert_eq!(status, 403, "{body}");
    assert!(body.contains("pin_incorrect"), "{body}");
    assert!(!root.join("note.txt").exists(), "nothing may be written");

    let _ = stop.send(());
    handle.await.unwrap();
    std::fs::remove_dir_all(&root).ok();
    std::fs::remove_dir_all(&source_dir).ok();
}

#[tokio::test]
async fn three_wrong_guesses_lock_the_address_out() {
    let identity = Identity::from_secret_bytes([34u8; 32]);
    let root = scratch("lockout");
    let source_dir = scratch("lockout-src");
    let source = source_dir.join("note.txt");
    std::fs::write(&source, b"a real file, so the request reaches the server").unwrap();

    let (server, addr) = start(&identity, &root, Some(PIN)).await.unwrap();
    let (stop, handle) = spawn(server);

    for attempt in 1..=MAX_ATTEMPTS {
        let client = Client::new(addr, &identity.fingerprint())
            .unwrap()
            .with_pin("000000");
        let mut connection = client.connect().await.unwrap();
        let err = connection
            .send_file(&source, None)
            .await
            .expect_err("wrong PIN");
        assert_eq!(
            status_of(&err).map(|(status, _)| status),
            Some(403),
            "attempt {attempt}: {err:?}"
        );
        connection.close();
    }

    // The right PIN now waits out the window, or the limit would be useless.
    let client = Client::new(addr, &identity.fingerprint())
        .unwrap()
        .with_pin(PIN);
    let mut connection = client.connect().await.unwrap();
    let err = connection
        .send_file(&source, None)
        .await
        .expect_err("locked out");
    assert_eq!(status_of(&err).map(|(status, _)| status), Some(403));
    connection.close();

    assert!(!root.join("note.txt").exists(), "nothing may be written");

    let _ = stop.send(());
    handle.await.unwrap();
    std::fs::remove_dir_all(&root).ok();
    std::fs::remove_dir_all(&source_dir).ok();
}

#[tokio::test]
async fn info_stays_open_when_a_pin_is_required() {
    // Discovery and pinning run before anyone knows there is a PIN, so `/info`
    // must never require one. It also carries `pin_required` so a sender knows
    // to ask the user.
    let identity = Identity::from_secret_bytes([35u8; 32]);
    let root = scratch("open-info");
    let (server, addr) = start(&identity, &root, Some(PIN)).await.unwrap();
    let (stop, handle) = spawn(server);

    let client = Client::new(addr, &identity.fingerprint()).unwrap();
    let mut connection = client.connect().await.unwrap();
    let info = connection.info().await.expect("info must not need a PIN");
    assert!(info.pin_required);
    assert_eq!(info.alias, "Desk");
    connection.close();

    let _ = stop.send(());
    handle.await.unwrap();
    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn an_unusable_pin_fails_at_startup_rather_than_silently_opening() {
    let identity = Identity::from_secret_bytes([36u8; 32]);
    let root = scratch("bad-pin");

    for bad in ["abc", "12", "1234567890123"] {
        let err = match start(&identity, &root, Some(bad)).await {
            Err(err) => err,
            Ok(_) => panic!("startup must refuse {bad:?}"),
        };
        assert!(matches!(err, ServerError::Pin(_)), "{bad:?} gave {err:?}");
    }

    std::fs::remove_dir_all(&root).ok();
}
