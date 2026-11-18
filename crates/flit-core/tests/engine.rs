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
    Engine, EngineConfig, EngineError, Event, ItemOutcome, OfferId, SendTarget, collect_targets,
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
    start_engine_with(secret, name, true).await
}

/// Start an engine whose consent policy is `auto_accept`.
///
/// Most tests are about transfers, not consent, so they start with the receiver
/// accepting every offer the way the desktop does. The consent tests start with
/// it off and answer the sheet themselves.
async fn start_engine_with(
    secret: u8,
    name: &str,
    auto_accept: bool,
) -> (Engine, PathBuf, Arc<Identity>) {
    let identity = Arc::new(Identity::from_secret_bytes([secret; 32]));
    let root = scratch(name);
    let engine = Engine::start(
        Arc::clone(&identity),
        config(root.clone(), SocketAddr::from(([127, 0, 0, 1], 0))),
    )
    .await
    .expect("engine start");
    engine.set_auto_accept(auto_accept, Vec::new());
    (engine, root, identity)
}

/// A receiver that must be answered before anything is saved.
async fn start_asking(secret: u8, name: &str) -> (Engine, PathBuf, Arc<Identity>) {
    start_engine_with(secret, name, false).await
}

/// Wait for the question a person would see: the offer, its item ids, and who
/// proved it.
async fn offered(
    events: &mut tokio::sync::broadcast::Receiver<Event>,
) -> (OfferId, Vec<String>, Option<String>) {
    loop {
        let event = tokio::time::timeout(Duration::from_secs(20), events.recv())
            .await
            .expect("an offer within 20 s")
            .expect("event stream stayed open");
        if let Event::IncomingOffer {
            id, sender, items, ..
        } = event
        {
            return (
                id,
                items.into_iter().map(|item| item.id).collect(),
                sender.fingerprint,
            );
        }
    }
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

#[tokio::test]
async fn an_answered_offer_saves_what_the_person_accepted() {
    let (receiver, receiver_root, receiver_identity) = start_asking(41, "consent-receiver").await;
    let (sender, sender_root, sender_identity) = start_engine(42, "consent-sender").await;

    let source = sender_root.join("note.txt");
    std::fs::write(&source, b"the person said yes").unwrap();

    let mut sender_events = sender.subscribe();
    let mut receiver_events = receiver.subscribe();
    sender
        .send_to(
            receiver.addr(),
            &receiver_identity.fingerprint(),
            vec![SendTarget {
                path: source,
                name: Some("note.txt".to_string()),
            }],
        )
        .await
        .expect("send starts");

    let (offer, ids, fingerprint) = offered(&mut receiver_events).await;
    assert_eq!(
        fingerprint,
        Some(sender_identity.fingerprint()),
        "the offer carries the fingerprint proven by the certificate"
    );
    assert_eq!(ids.len(), 1, "the sheet lists the items it can accept");
    assert!(
        receiver.answer_offer(offer, ids, true).await,
        "an open offer takes an answer"
    );

    assert!(matches!(
        terminal(&mut sender_events).await,
        Event::TransferFinished { ref items, .. } if items.iter().all(|item| item.error.is_none())
    ));
    assert_eq!(
        std::fs::read(receiver_root.join("note.txt")).unwrap(),
        b"the person said yes"
    );
    assert_eq!(
        receiver.trusted_devices(),
        vec![sender_identity.fingerprint()],
        "ticking remember adds the proven fingerprint"
    );

    // The remembered device skips the sheet: this second send finishes without
    // anybody answering. If trust had not stuck, it would wait out the offer
    // timeout instead.
    let second = sender_root.join("second.txt");
    std::fs::write(&second, b"no sheet this time").unwrap();
    sender
        .send_to(
            receiver.addr(),
            &receiver_identity.fingerprint(),
            vec![SendTarget {
                path: second,
                name: Some("second.txt".to_string()),
            }],
        )
        .await
        .expect("send starts");
    assert!(matches!(
        terminal(&mut sender_events).await,
        Event::TransferFinished { ref items, .. } if items.iter().all(|item| item.error.is_none())
    ));
    assert_eq!(
        std::fs::read(receiver_root.join("second.txt")).unwrap(),
        b"no sheet this time"
    );

    sender.stop().await;
    receiver.stop().await;
    std::fs::remove_dir_all(&receiver_root).ok();
    std::fs::remove_dir_all(&sender_root).ok();
}

#[tokio::test]
async fn a_declined_offer_writes_nothing_and_remembers_nothing() {
    let (receiver, receiver_root, receiver_identity) = start_asking(43, "declined-receiver").await;
    let (sender, sender_root, _) = start_engine(44, "declined-sender").await;

    let source = sender_root.join("secret.txt");
    std::fs::write(&source, b"not for this phone").unwrap();

    let mut sender_events = sender.subscribe();
    let mut receiver_events = receiver.subscribe();
    sender
        .send_to(
            receiver.addr(),
            &receiver_identity.fingerprint(),
            vec![SendTarget {
                path: source,
                name: Some("secret.txt".to_string()),
            }],
        )
        .await
        .expect("send starts");

    let (offer, ids, _) = offered(&mut receiver_events).await;
    assert!(!receiver.auto_accept(), "nothing is accepted silently");
    // Remembering is ticked on purpose: a decline must not grant trust.
    assert!(receiver.answer_offer(offer, Vec::new(), true).await);

    match terminal(&mut sender_events).await {
        Event::TransferFailed { reason, .. } => {
            assert!(reason.contains("declined"), "the reason names it: {reason}");
        }
        other => panic!("expected a failed transfer, got {other:?}"),
    }
    assert!(
        !receiver_root.join("secret.txt").exists(),
        "a declined offer writes nothing"
    );
    assert!(
        receiver.trusted_devices().is_empty(),
        "declining never adds a trusted device"
    );
    assert!(
        !receiver.answer_offer(offer, ids, false).await,
        "a closed offer cannot be answered twice"
    );

    sender.stop().await;
    receiver.stop().await;
    std::fs::remove_dir_all(&receiver_root).ok();
    std::fs::remove_dir_all(&sender_root).ok();
}

/// The phone keeps the trusted list in its own settings and hands it to the
/// engine on every start, so trust has to survive that handoff: a new engine
/// told what was saved must not ask again, without the person touching
/// anything.
#[tokio::test]
async fn trust_saved_on_this_phone_still_skips_the_sheet() {
    let receiver_identity = Arc::new(Identity::from_secret_bytes([45; 32]));
    let (sender, sender_root, _) = start_engine(46, "restart-sender").await;
    let mut sender_events = sender.subscribe();

    let first_root = scratch("restart-first");
    let first = Engine::start(
        Arc::clone(&receiver_identity),
        config(first_root.clone(), SocketAddr::from(([127, 0, 0, 1], 0))),
    )
    .await
    .expect("first start");
    first.set_auto_accept(false, Vec::new());

    let before = sender_root.join("before.txt");
    std::fs::write(&before, b"trusted before the restart").unwrap();
    let mut first_events = first.subscribe();
    sender
        .send_to(
            first.addr(),
            &receiver_identity.fingerprint(),
            vec![SendTarget {
                path: before,
                name: Some("before.txt".to_string()),
            }],
        )
        .await
        .expect("send starts");
    let (offer, ids, _) = offered(&mut first_events).await;
    assert!(first.answer_offer(offer, ids, true).await);
    assert!(matches!(
        terminal(&mut sender_events).await,
        Event::TransferFinished { .. }
    ));
    let saved = first.trusted_devices();
    assert_eq!(saved.len(), 1, "the answer granted exactly one device");
    first.stop().await;

    let second_root = scratch("restart-second");
    let second = Engine::start(
        Arc::clone(&receiver_identity),
        config(second_root.clone(), SocketAddr::from(([127, 0, 0, 1], 0))),
    )
    .await
    .expect("second start");
    second.set_auto_accept(false, saved);

    let after = sender_root.join("after.txt");
    std::fs::write(&after, b"still trusted after the restart").unwrap();
    sender
        .send_to(
            second.addr(),
            &receiver_identity.fingerprint(),
            vec![SendTarget {
                path: after,
                name: Some("after.txt".to_string()),
            }],
        )
        .await
        .expect("send starts");
    // No sheet and no answer: the transfer only finishes inside the offer
    // window if the saved list was honoured.
    assert!(matches!(
        terminal(&mut sender_events).await,
        Event::TransferFinished { ref items, .. } if items.iter().all(|item| item.error.is_none())
    ));
    assert_eq!(
        std::fs::read(second_root.join("after.txt")).unwrap(),
        b"still trusted after the restart"
    );

    sender.stop().await;
    second.stop().await;
    std::fs::remove_dir_all(&first_root).ok();
    std::fs::remove_dir_all(&second_root).ok();
    std::fs::remove_dir_all(&sender_root).ok();
}

/// A browser share is a capability written on the screen. It must exist while
/// the screen says it does, survive being read twice, and stop being an address
/// the moment the person stops it.
#[tokio::test]
async fn a_browser_share_lives_until_it_is_stopped() {
    let (engine, root, _) = start_engine(61, "share").await;

    assert!(matches!(
        engine.open_share(Vec::new()).await,
        Err(EngineError::NothingToSend)
    ));
    assert!(engine.share().await.is_none(), "nothing opened yet");

    let file = root.join("notes.txt");
    std::fs::write(&file, b"hello from the phone").unwrap();
    let target = engine.open_share(vec![file]).await.expect("a share");

    assert_eq!(target.files, 1);
    assert!(
        target.url.starts_with("http://"),
        "{} is not an address a browser takes",
        target.url
    );
    assert!(
        target.url.contains("/r/"),
        "{} carries no token",
        target.url
    );
    assert!(
        !target.expires_in.is_zero(),
        "a share that lives for no time"
    );

    let read_again = engine.share().await.expect("still open");
    assert_eq!(
        read_again.url, target.url,
        "reading the address must not change it"
    );

    engine.close_share().await;
    assert!(
        engine.share().await.is_none(),
        "a stopped share must not still hand out its address"
    );

    engine.stop().await;
    std::fs::remove_dir_all(&root).ok();
}
