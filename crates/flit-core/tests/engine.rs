//! End-to-end tests for the engine facade.
//!
//! These drive the same code a UI drives: no protocol calls, no sockets built
//! by hand. A send is started by device id or by address, and the result is
//! read from the event stream.

use std::{
    net::SocketAddr,
    path::PathBuf,
    sync::Arc,
    time::{Duration, SystemTime},
};

use flit_core::Identity;
use flit_core::{
    Engine, EngineConfig, EngineError, Event, ItemOutcome, SendTarget, collect_targets,
};

/// A scratch directory that does not need a temp-dir crate.
fn scratch(name: &str) -> PathBuf {
    let unique = format!(
        "flit-engine-{}-{}-{name}",
        std::process::id(),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let path = std::env::temp_dir().join(unique);
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn config(root: PathBuf, bind: SocketAddr) -> EngineConfig {
    EngineConfig {
        alias: "Desk".to_string(),
        bind,
        root,
        // Tests run two engines on one machine: discovery would have both
        // announcing on the same port and neither needs it.
        discovery: None,
        ..EngineConfig::default()
    }
}

async fn start_engine(secret: u8, name: &str) -> (Engine, PathBuf, Arc<Identity>) {
    let identity = Arc::new(Identity::from_secret_bytes([secret; 32]));
    let root = scratch(name);
    let engine = Engine::start(
        Arc::clone(&identity),
        config(root.clone(), SocketAddr::from(([127, 0, 0, 1], 0))),
    )
    .await
    .expect("engine start");
    (engine, root, identity)
}

/// Wait for a terminal event, ignoring peer and discovery noise.
async fn terminal(events: &mut tokio::sync::broadcast::Receiver<Event>) -> Event {
    loop {
        let event = tokio::time::timeout(Duration::from_secs(20), events.recv())
            .await
            .expect("a terminal event within 20 s")
            .expect("event stream stayed open");
        match event {
            Event::TransferFinished { .. }
            | Event::TransferFailed { .. }
            | Event::TransferCancelled { .. } => return event,
            _ => {}
        }
    }
}

#[tokio::test]
async fn a_send_between_two_engines_verifies_and_installs_the_file() {
    let (receiver, receiver_root, receiver_identity) = start_engine(31, "receiver").await;
    let (sender, sender_root, _) = start_engine(32, "sender").await;

    let source = sender_root.join("hello.txt");
    std::fs::write(&source, b"hello from the engine").unwrap();
    let targets = vec![SendTarget {
        path: source,
        name: Some("greeting.txt".to_string()),
    }];

    let mut events = sender.subscribe();
    let id = sender
        .send_to(receiver.addr(), &receiver_identity.fingerprint(), targets)
        .await
        .expect("send starts");

    assert!(matches!(
        terminal(&mut events).await,
        Event::TransferFinished { id: finished, ref items }
            if finished == id
                && items.len() == 1
                && items[0].saved_as.as_deref() == Some("greeting.txt")
                && items[0].error.is_none()
    ));
    assert_eq!(
        std::fs::read(receiver_root.join("greeting.txt")).unwrap(),
        b"hello from the engine"
    );
    assert_eq!(sender.running().await, Vec::new());

    sender.stop().await;
    receiver.stop().await;
    std::fs::remove_dir_all(&receiver_root).ok();
    std::fs::remove_dir_all(&sender_root).ok();
}

#[tokio::test]
async fn a_text_send_arrives_as_a_verified_file() {
    let (receiver, receiver_root, receiver_identity) = start_engine(35, "text-receiver").await;
    let (sender, sender_root, _) = start_engine(36, "text-sender").await;

    let mut events = sender.subscribe();
    let id = sender
        .send_text_to(
            receiver.addr(),
            &receiver_identity.fingerprint(),
            "https://example.com/q3".to_string(),
            "clipboard.txt".to_string(),
        )
        .await
        .expect("text send starts");

    assert!(matches!(
        terminal(&mut events).await,
        Event::TransferFinished { id: finished, ref items }
            if finished == id
                && items.len() == 1
                && items[0].name == "clipboard.txt"
                && items[0].path.as_os_str().is_empty()
                && items[0].saved_as.as_deref() == Some("clipboard.txt")
                && items[0].error.is_none()
    ));
    assert_eq!(
        std::fs::read_to_string(receiver_root.join("clipboard.txt")).unwrap(),
        "https://example.com/q3"
    );
    assert_eq!(sender.running().await, Vec::new());

    sender.stop().await;
    receiver.stop().await;
    std::fs::remove_dir_all(&receiver_root).ok();
    std::fs::remove_dir_all(&sender_root).ok();
}

#[tokio::test]
async fn a_wrong_fingerprint_fails_the_send_instead_of_sending() {
    let (receiver, receiver_root, _) = start_engine(33, "wrong-pin").await;
    let (sender, sender_root, _) = start_engine(34, "wrong-pin-src").await;

    let source = sender_root.join("secret.txt");
    std::fs::write(&source, b"not for you").unwrap();

    // A fingerprint from a different identity: exactly what a stale peer row
    // or a QR code for the wrong device looks like.
    let impostor = Identity::from_secret_bytes([35; 32]).fingerprint();

    let mut events = sender.subscribe();
    let id = sender
        .send_to(
            receiver.addr(),
            &impostor,
            vec![SendTarget {
                path: source,
                name: None,
            }],
        )
        .await
        .expect("the send is attempted");

    match terminal(&mut events).await {
        Event::TransferFailed { id: failed, reason } => {
            assert_eq!(failed, id);
            assert!(!reason.is_empty(), "a failure needs a reason");
        }
        other => panic!("expected a failed transfer, got {other:?}"),
    }
    assert!(
        !receiver_root.join("secret.txt").exists(),
        "a refused connection writes nothing"
    );

    sender.stop().await;
    receiver.stop().await;
    std::fs::remove_dir_all(&receiver_root).ok();
    std::fs::remove_dir_all(&sender_root).ok();
}

#[tokio::test]
async fn a_batch_reports_every_item_even_when_one_is_refused() {
    let (receiver, receiver_root, receiver_identity) = start_engine(36, "batch").await;
    let (sender, sender_root, _) = start_engine(37, "batch-src").await;

    let one = sender_root.join("one.txt");
    let two = sender_root.join("two.txt");
    std::fs::write(&one, b"first").unwrap();
    std::fs::write(&two, b"second").unwrap();

    let targets = vec![
        SendTarget {
            path: one,
            name: Some("bundle/one.txt".to_string()),
        },
        SendTarget {
            path: two,
            name: Some("../escape.txt".to_string()),
        },
    ];

    let mut events = sender.subscribe();
    sender
        .send_to(receiver.addr(), &receiver_identity.fingerprint(), targets)
        .await
        .expect("send starts");

    let items = match terminal(&mut events).await {
        Event::TransferFinished { items, .. } => items,
        other => panic!("expected a finished transfer, got {other:?}"),
    };
    assert_eq!(items.len(), 2);

    let saved: Vec<&ItemOutcome> = items.iter().filter(|item| item.error.is_none()).collect();
    assert_eq!(saved.len(), 1, "one item is accepted: {items:?}");
    assert_eq!(saved[0].saved_as.as_deref(), Some("bundle/one.txt"));

    let refused = items.iter().find(|item| item.error.is_some()).unwrap();
    assert_eq!(refused.name, "../escape.txt");
    assert!(!receiver_root.join("escape.txt").exists());

    sender.stop().await;
    receiver.stop().await;
    std::fs::remove_dir_all(&receiver_root).ok();
    std::fs::remove_dir_all(&sender_root).ok();
}

#[tokio::test]
async fn an_engine_without_discovery_sees_no_peers_and_still_receives() {
    let (engine, root, identity) = start_engine(38, "quiet").await;

    assert!(
        engine.peers().await.is_empty(),
        "no discovery means no peer table"
    );
    assert_eq!(engine.info().alias, "Desk");
    assert_eq!(engine.info().fingerprint, identity.fingerprint());

    // The listening address is real even though nothing advertises it: this is
    // the manual path a QR code or a typed address uses.
    let mut stream = tokio::net::TcpStream::connect(engine.addr())
        .await
        .expect("the receiver listens");
    use tokio::io::AsyncWriteExt;
    stream
        .write_all(&[0x16, 0x03, 0x01])
        .await
        .expect("a TLS handshake byte is accepted");
    drop(stream);

    engine.stop().await;
    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn a_folder_dropped_on_the_engine_becomes_targets_in_one_call() {
    let (engine, root, _) = start_engine(39, "drop").await;

    let folder = root.join("photos");
    std::fs::create_dir_all(folder.join("day one")).unwrap();
    std::fs::write(folder.join("a.jpg"), b"a").unwrap();
    std::fs::write(folder.join("day one/b.jpg"), b"b").unwrap();

    let collected = collect_targets(&[folder], None).expect("walk");
    assert_eq!(collected.skipped, 0);
    let mut names: Vec<String> = collected
        .targets
        .iter()
        .map(|target| target.name.clone().unwrap())
        .collect();
    names.sort();
    assert_eq!(
        names,
        vec![
            "photos/a.jpg".to_string(),
            "photos/day one/b.jpg".to_string()
        ]
    );

    engine.stop().await;
    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn sending_nothing_or_to_an_unknown_device_is_refused_before_any_work() {
    let (engine, root, _) = start_engine(40, "refuse").await;

    match engine
        .send_to(SocketAddr::from(([127, 0, 0, 1], 9)), "nope", Vec::new())
        .await
    {
        Err(EngineError::NothingToSend) => {}
        other => panic!("expected NothingToSend, got {other:?}"),
    }

    match engine
        .send_text_to(
            SocketAddr::from(([127, 0, 0, 1], 9)),
            "nope",
            String::new(),
            "clipboard.txt".to_string(),
        )
        .await
    {
        Err(EngineError::NothingToSend) => {}
        other => panic!("expected NothingToSend for empty text, got {other:?}"),
    }

    let source = root.join("file.txt");
    std::fs::write(&source, b"x").unwrap();
    match engine
        .send_to_peer(
            "no-such-device",
            vec![SendTarget {
                path: source,
                name: None,
            }],
        )
        .await
    {
        Err(EngineError::UnknownPeer(id)) => assert_eq!(id, "no-such-device"),
        other => panic!("expected UnknownPeer, got {other:?}"),
    }

    engine.stop().await;
    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn a_send_reports_progress_between_started_and_finished() {
    let (receiver, receiver_root, receiver_identity) = start_engine(44, "progress-receiver").await;
    let (sender, sender_root, _) = start_engine(45, "progress-sender").await;

    // Large enough that the 200 ms ticker fires while bytes are moving; a send
    // that finishes sooner legitimately has no intermediate state to show.
    let payload = vec![0u8; 256 * 1024 * 1024];
    let source = sender_root.join("bulk.bin");
    std::fs::write(&source, &payload).unwrap();

    let mut events = sender.subscribe();
    let id = sender
        .send_to(
            receiver.addr(),
            &receiver_identity.fingerprint(),
            vec![SendTarget {
                path: source,
                name: Some("bulk.bin".to_string()),
            }],
        )
        .await
        .expect("send starts");

    let mut saw_bytes = false;
    let mut declared = 0u64;
    loop {
        let event = tokio::time::timeout(Duration::from_secs(60), events.recv())
            .await
            .expect("an event within 60 s")
            .expect("event stream stayed open");
        match event {
            Event::TransferProgress {
                id: progress_id,
                sent,
                total,
                ..
            } => {
                assert_eq!(progress_id, id);
                assert!(
                    sent <= total,
                    "progress never overshoots: {sent} of {total}"
                );
                declared = total;
                saw_bytes |= sent > 0;
            }
            Event::TransferFinished {
                id: finished,
                items,
            } => {
                assert_eq!(finished, id);
                assert_eq!(items[0].bytes, payload.len() as u64);
                break;
            }
            Event::TransferFailed { reason, .. } => panic!("send failed: {reason}"),
            Event::TransferCancelled { .. } => panic!("send was cancelled unexpectedly"),
            _ => {}
        }
    }
    assert!(saw_bytes, "a 256 MiB send reports bytes while it runs");
    assert_eq!(declared, payload.len() as u64);

    sender.stop().await;
    receiver.stop().await;
    std::fs::remove_dir_all(&receiver_root).ok();
    std::fs::remove_dir_all(&sender_root).ok();
}

#[tokio::test]
async fn cancelling_a_send_reports_it_and_writes_nothing() {
    let (receiver, receiver_root, receiver_identity) = start_engine(46, "cancel-receiver").await;
    let (sender, sender_root, _) = start_engine(47, "cancel-sender").await;

    let payload = vec![0u8; 256 * 1024 * 1024];
    let source = sender_root.join("bulk.bin");
    std::fs::write(&source, &payload).unwrap();

    let mut events = sender.subscribe();
    let id = sender
        .send_to(
            receiver.addr(),
            &receiver_identity.fingerprint(),
            vec![SendTarget {
                path: source,
                name: Some("bulk.bin".to_string()),
            }],
        )
        .await
        .expect("send starts");

    // Cancel once the wire is actually moving, so this exercises the upload
    // path rather than cancelling during hashing.
    loop {
        let event = tokio::time::timeout(Duration::from_secs(30), events.recv())
            .await
            .expect("an event within 30 s")
            .expect("event stream stayed open");
        if let Event::TransferProgress { sent, .. } = event {
            if sent > 0 {
                break;
            }
        }
    }
    assert!(sender.cancel(id).await, "a running send can be cancelled");

    match terminal(&mut events).await {
        Event::TransferCancelled { id: cancelled } => assert_eq!(cancelled, id),
        other => panic!("expected a cancelled transfer, got {other:?}"),
    }
    assert!(
        !receiver_root.join("bulk.bin").exists(),
        "nothing installed"
    );
    let parts = receiver_root.join(".flit").join("parts");
    assert!(
        !parts.is_dir() || std::fs::read_dir(&parts).unwrap().next().is_none(),
        "the receiver dropped the session instead of keeping partial bytes"
    );

    sender.stop().await;
    receiver.stop().await;
    std::fs::remove_dir_all(&receiver_root).ok();
    std::fs::remove_dir_all(&sender_root).ok();
}
