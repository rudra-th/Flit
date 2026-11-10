//! End-to-end upload tests: prepare, stream, verify, commit.

use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
};

use flit_api::{DeviceType, ErrorCode};
use flit_client::{Client, ClientError, SendOutcome, SendTarget};
use flit_crypto::Identity;
use flit_server::{Server, ServerConfig};

/// A scratch directory that does not need a temp-dir crate.
fn scratch(name: &str) -> PathBuf {
    let unique = format!(
        "flit-upload-{}-{}-{name}",
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

async fn start(identity: &Identity, root: &Path) -> (Server, SocketAddr) {
    let config = ServerConfig::default()
        .with_alias("Desk")
        .with_device_type(DeviceType::Laptop)
        .with_bind(SocketAddr::from(([127, 0, 0, 1], 0)))
        .with_root(root);
    let server = Server::bind(identity, config).await.expect("bind");
    let addr = server.local_addr();
    (server, addr)
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

#[tokio::test]
async fn a_file_arrives_verified_and_whole() {
    let identity = Identity::from_secret_bytes([21u8; 32]);
    let root = scratch("roundtrip");
    let sender_dir = scratch("roundtrip-src");
    let payload = b"flit sends files, not promises\n".repeat(1024);
    let source = sender_dir.join("note.txt");
    std::fs::write(&source, &payload).unwrap();

    let (server, addr) = start(&identity, &root).await;
    let (stop, handle) = spawn(server);

    let client = Client::new(addr, &identity.fingerprint()).unwrap();
    let mut connection = client.connect().await.unwrap();
    let outcome = connection.send_file(&source, None).await.expect("send");
    connection.close();

    match outcome {
        SendOutcome::Verified(saved) => {
            assert_eq!(saved.saved_as, "note.txt");
            assert_eq!(saved.bytes, payload.len() as u64);
        }
    }

    assert_eq!(std::fs::read(root.join("note.txt")).unwrap(), payload);
    assert!(
        !root
            .join(".flit")
            .join("parts")
            .read_dir()
            .unwrap()
            .any(|_| true)
    );

    let _ = stop.send(());
    handle.await.unwrap();
    std::fs::remove_dir_all(&root).ok();
    std::fs::remove_dir_all(&sender_dir).ok();
}

#[tokio::test]
async fn a_second_file_with_the_same_name_does_not_overwrite() {
    let identity = Identity::from_secret_bytes([22u8; 32]);
    let root = scratch("conflict");
    let sender_dir = scratch("conflict-src");
    let first = sender_dir.join("report.txt");
    let second = sender_dir.join("other.txt");
    std::fs::write(&first, b"first").unwrap();
    std::fs::write(&second, b"second").unwrap();

    let (server, addr) = start(&identity, &root).await;
    let (stop, handle) = spawn(server);

    let client = Client::new(addr, &identity.fingerprint()).unwrap();
    let mut connection = client.connect().await.unwrap();

    let one = connection
        .send_file(&first, Some("report.txt"))
        .await
        .unwrap();
    let two = connection
        .send_file(&second, Some("report.txt"))
        .await
        .unwrap();
    connection.close();

    match (one, two) {
        (SendOutcome::Verified(a), SendOutcome::Verified(b)) => {
            assert_eq!(a.saved_as, "report.txt");
            assert_eq!(b.saved_as, "report (2).txt");
        }
    }
    assert_eq!(std::fs::read(root.join("report.txt")).unwrap(), b"first");
    assert_eq!(
        std::fs::read(root.join("report (2).txt")).unwrap(),
        b"second"
    );

    let _ = stop.send(());
    handle.await.unwrap();
    std::fs::remove_dir_all(&root).ok();
    std::fs::remove_dir_all(&sender_dir).ok();
}

#[tokio::test]
async fn traversal_in_the_announced_name_is_refused() {
    let identity = Identity::from_secret_bytes([23u8; 32]);
    let root = scratch("traversal");
    let sender_dir = scratch("traversal-src");
    let source = sender_dir.join("evil.txt");
    std::fs::write(&source, b"escape attempt").unwrap();

    let (server, addr) = start(&identity, &root).await;
    let (stop, handle) = spawn(server);

    let client = Client::new(addr, &identity.fingerprint()).unwrap();
    let mut connection = client.connect().await.unwrap();
    let err = connection
        .send_file(&source, Some("../escape.txt"))
        .await
        .expect_err("must be refused");
    connection.close();

    assert!(
        matches!(
            err,
            ClientError::Rejected(ErrorCode::BadRequest) | ClientError::Status { status: 400, .. }
        ),
        "{err:?}"
    );
    let escaped = root.parent().unwrap().join("escape.txt");
    assert!(!escaped.exists(), "nothing may be written outside the root");

    let _ = stop.send(());
    handle.await.unwrap();
    std::fs::remove_dir_all(&root).ok();
    std::fs::remove_dir_all(&sender_dir).ok();
    std::fs::remove_file(escaped).ok();
}

#[tokio::test]
async fn an_empty_file_is_a_file() {
    let identity = Identity::from_secret_bytes([24u8; 32]);
    let root = scratch("empty");
    let sender_dir = scratch("empty-src");
    let source = sender_dir.join("empty.bin");
    std::fs::write(&source, b"").unwrap();

    let (server, addr) = start(&identity, &root).await;
    let (stop, handle) = spawn(server);

    let client = Client::new(addr, &identity.fingerprint()).unwrap();
    let mut connection = client.connect().await.unwrap();
    connection.send_file(&source, None).await.expect("send");
    connection.close();

    assert_eq!(std::fs::read(root.join("empty.bin")).unwrap(), b"");

    let _ = stop.send(());
    handle.await.unwrap();
    std::fs::remove_dir_all(&root).ok();
    std::fs::remove_dir_all(&sender_dir).ok();
}

#[tokio::test]
async fn a_batch_arrives_in_one_session_with_nested_names() {
    let identity = Identity::from_secret_bytes([25u8; 32]);
    let root = scratch("batch");
    let sender_dir = scratch("batch-src");
    let first = sender_dir.join("one.txt");
    let second = sender_dir.join("two.bin");
    let third = sender_dir.join("three.txt");
    std::fs::write(&first, b"first payload").unwrap();
    std::fs::write(&second, vec![7u8; 300 * 1024]).unwrap();
    std::fs::write(&third, b"").unwrap();

    let (server, addr) = start(&identity, &root).await;
    let (stop, handle) = spawn(server);

    let client = Client::new(addr, &identity.fingerprint()).unwrap();
    let mut connection = client.connect().await.unwrap();
    let targets = vec![
        SendTarget { path: first.clone(), name: Some("one.txt".into()) },
        SendTarget { path: second.clone(), name: Some("folder/two.bin".into()) },
        SendTarget { path: third.clone(), name: None },
    ];
    let outcomes = connection.send_files(&targets).await.expect("send");
    connection.close();

    assert_eq!(outcomes.len(), 3, "{outcomes:?}");
    let saved: Vec<String> = outcomes
        .iter()
        .map(|outcome| match &outcome.result {
            Ok(saved) => saved.saved_as.clone(),
            Err(err) => panic!("{} failed: {err}", outcome.path.display()),
        })
        .collect();
    assert_eq!(saved, vec!["one.txt", "folder/two.bin", "three.txt"]);

    assert_eq!(std::fs::read(root.join("one.txt")).unwrap(), b"first payload");
    assert_eq!(std::fs::read(root.join("folder/two.bin")).unwrap(), vec![7u8; 300 * 1024]);
    assert_eq!(std::fs::read(root.join("three.txt")).unwrap(), b"");
    assert!(
        !root
            .join(".flit")
            .join("parts")
            .read_dir()
            .unwrap()
            .any(|_| true),
        "the session directory must be gone once the batch is committed"
    );

    let _ = stop.send(());
    handle.await.unwrap();
    std::fs::remove_dir_all(&root).ok();
    std::fs::remove_dir_all(&sender_dir).ok();
}

#[tokio::test]
async fn many_small_files_ride_pack_requests() {
    let identity = Identity::from_secret_bytes([26u8; 32]);
    let root = scratch("pack");
    let sender_dir = scratch("pack-src");

    let mut targets = Vec::new();
    for index in 0u32..200 {
        let path = sender_dir.join(format!("file-{index:03}.txt"));
        let payload = vec![b'a' + (index % 26) as u8; (index as usize * 37) % 4096];
        std::fs::write(&path, &payload).unwrap();
        targets.push(SendTarget {
            path,
            name: Some(format!("bundle/file-{index:03}.txt")),
        });
    }

    let (server, addr) = start(&identity, &root).await;
    let (stop, handle) = spawn(server);

    let client = Client::new(addr, &identity.fingerprint()).unwrap();
    let mut connection = client.connect().await.unwrap();
    let outcomes = connection.send_files(&targets).await.expect("send");
    connection.close();

    assert_eq!(outcomes.len(), 200);
    for (index, outcome) in outcomes.iter().enumerate() {
        match &outcome.result {
            Ok(saved) => assert_eq!(saved.saved_as, format!("bundle/file-{index:03}.txt")),
            Err(err) => panic!("file {index} failed: {err}"),
        }
    }
    assert!(std::fs::read(root.join("bundle/file-000.txt")).unwrap().is_empty());
    assert_eq!(
        std::fs::read(root.join("bundle/file-199.txt")).unwrap().len(),
        (199 * 37) % 4096
    );
    assert!(
        !root
            .join(".flit")
            .join("parts")
            .read_dir()
            .unwrap()
            .any(|_| true),
        "packed items must not leave parts behind"
    );

    let _ = stop.send(());
    handle.await.unwrap();
    std::fs::remove_dir_all(&root).ok();
    std::fs::remove_dir_all(&sender_dir).ok();
}
