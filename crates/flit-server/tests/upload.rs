//! End-to-end upload tests: prepare, stream, verify, commit.

use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use flit_api::{
    API_PREFIX, DeviceType, ErrorCode, PROTO_HEADER, PROTO_VERSION_STR, SessionResponse,
};
use flit_client::{Client, ClientError, SendOutcome, SendProgress, SendTarget};
use flit_crypto::Identity;
use flit_server::{Server, ServerConfig};
use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper::{Request, StatusCode, header};
use hyper_util::rt::TokioIo;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use sha2::{Digest, Sha256};

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
        SendTarget {
            path: first.clone(),
            name: Some("one.txt".into()),
        },
        SendTarget {
            path: second.clone(),
            name: Some("folder/two.bin".into()),
        },
        SendTarget {
            path: third.clone(),
            name: None,
        },
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

    assert_eq!(
        std::fs::read(root.join("one.txt")).unwrap(),
        b"first payload"
    );
    assert_eq!(
        std::fs::read(root.join("folder/two.bin")).unwrap(),
        vec![7u8; 300 * 1024]
    );
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
    assert!(
        std::fs::read(root.join("bundle/file-000.txt"))
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        std::fs::read(root.join("bundle/file-199.txt"))
            .unwrap()
            .len(),
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

/// A deliberately unpinned TLS client for tests that need raw HTTP.
///
/// The real client pins the receiver's fingerprint. Here the server was just
/// started on loopback by this test, so a pin would only restate the fixture.
struct RawClient {
    sender: hyper::client::conn::http1::SendRequest<Full<Bytes>>,
}

/// Accepts any certificate. Only ever used against a loopback test server.
#[derive(Debug)]
struct AcceptAnyCertificate;

impl ServerCertVerifier for AcceptAnyCertificate {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

impl RawClient {
    async fn connect(addr: SocketAddr) -> Self {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let config = rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .expect("tls versions")
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AcceptAnyCertificate))
            .with_no_client_auth();
        let connector = tokio_rustls::TlsConnector::from(Arc::new(config));
        let stream = tokio::net::TcpStream::connect(addr).await.expect("tcp");
        let name = ServerName::try_from("flit.test".to_string()).expect("server name");
        let tls = connector.connect(name, stream).await.expect("tls");
        let (sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(tls))
            .await
            .expect("handshake");
        tokio::spawn(async move {
            let _ = connection.await;
        });
        Self { sender }
    }

    async fn call(&mut self, request: Request<Full<Bytes>>) -> (StatusCode, Bytes) {
        let response = self.sender.send_request(request).await.expect("response");
        let status = response.status();
        let body = response
            .into_body()
            .collect()
            .await
            .expect("response body")
            .to_bytes();
        (status, body)
    }

    /// Announce one item and return the session id.
    async fn prepare(&mut self, manifest: &str) -> String {
        let request = Request::builder()
            .method("POST")
            .uri(format!("{API_PREFIX}/upload"))
            .header(header::HOST, "flit.test")
            .header(PROTO_HEADER, PROTO_VERSION_STR)
            .body(Full::new(Bytes::from(manifest.to_string())))
            .expect("prepare request");
        let (status, body) = self.call(request).await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "prepare failed: {}",
            String::from_utf8_lossy(&body)
        );
        let response: SessionResponse = serde_json::from_slice(&body).expect("prepare response");
        assert_eq!(response.accepted, vec!["i1".to_string()]);
        response.session_id
    }

    /// Abandon a session. The receiver must drop its partial bytes.
    async fn delete(&mut self, session_id: &str) -> StatusCode {
        let request = Request::builder()
            .method("DELETE")
            .uri(format!("{API_PREFIX}/upload/{session_id}"))
            .header(header::HOST, "flit.test")
            .header(PROTO_HEADER, PROTO_VERSION_STR)
            .body(Full::new(Bytes::new()))
            .expect("delete request");
        self.call(request).await.0
    }

    /// Send bytes for one item, optionally declaring where they start.
    async fn put(
        &mut self,
        session_id: &str,
        item_id: &str,
        body: &[u8],
        offset: Option<u64>,
    ) -> (StatusCode, Bytes) {
        let mut builder = Request::builder()
            .method("PUT")
            .uri(format!("{API_PREFIX}/upload/{session_id}/{item_id}"))
            .header(header::HOST, "flit.test")
            .header(PROTO_HEADER, PROTO_VERSION_STR);
        if let Some(offset) = offset {
            builder = builder.header(header::CONTENT_RANGE, format!("bytes {offset}-"));
        }
        let request = builder
            .body(Full::new(Bytes::copy_from_slice(body)))
            .expect("put request");
        self.call(request).await
    }
}

/// A sender that dies mid-file leaves a partial `.part` and an open session.
/// Re-running the same command must adopt that session and resume, not start
/// over: this is the `kill -9` promise, exercised without the signal.
#[tokio::test]
async fn an_interrupted_send_resumes_in_the_same_session() {
    let identity = Identity::from_secret_bytes([27u8; 32]);
    let root = scratch("resume");
    let sender_dir = scratch("resume-src");
    let payload: Vec<u8> = (0..1024 * 1024).map(|index| (index % 251) as u8).collect();
    let source = sender_dir.join("big.bin");
    std::fs::write(&source, &payload).unwrap();

    let (server, addr) = start(&identity, &root).await;
    let (stop, handle) = spawn(server);

    let digest = hex::encode(Sha256::digest(&payload));
    let manifest = format!(
        "{{\"id\":\"i1\",\"name\":\"big.bin\",\"size\":{},\"sha256\":\"{digest}\",\"kind\":\"file\"}}\n",
        payload.len()
    );

    let mut raw = RawClient::connect(addr).await;
    let session_id = raw.prepare(&manifest).await;

    // The connection dies after 64 KiB of a 1 MiB file. The receiver answers
    // `400` because the body was short, but keeps what it already wrote.
    let partial = 64 * 1024;
    let (status, _) = raw.put(&session_id, "i1", &payload[..partial], None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "a short body is refused");

    let part = root
        .join(".flit")
        .join("parts")
        .join(&session_id)
        .join("0.part");
    assert_eq!(
        std::fs::metadata(&part).expect("part file").len(),
        partial as u64,
        "the receiver keeps the bytes it already has"
    );

    // The same command again. Its manifest matches the interrupted one, so the
    // receiver hands back the open session; the first `PUT` gets a `308` at
    // 64 KiB and the rest of the file follows from there.
    let client = Client::new(addr, &identity.fingerprint()).unwrap();
    let mut connection = client.connect().await.unwrap();
    let outcome = connection.send_file(&source, None).await.expect("resume");
    connection.close();
    assert!(matches!(outcome, SendOutcome::Verified(_)));

    assert_eq!(std::fs::read(root.join("big.bin")).unwrap(), payload);
    assert!(
        !root.join(".flit").join("parts").join(&session_id).exists(),
        "the interrupted session was resumed and cleaned up, not abandoned"
    );

    let _ = stop.send(());
    handle.await.unwrap();
    std::fs::remove_dir_all(&root).ok();
    std::fs::remove_dir_all(&sender_dir).ok();
}

/// Cancelling a session must remove its partial bytes and forget it. A
/// cancelled session is not a resume candidate either.
#[tokio::test]
async fn cancelling_a_session_drops_its_partial_bytes() {
    let identity = Identity::from_secret_bytes([28u8; 32]);
    let root = scratch("cancel");

    let (server, addr) = start(&identity, &root).await;
    let (stop, handle) = spawn(server);

    let payload = vec![3u8; 8 * 1024];
    let digest = hex::encode(Sha256::digest(&payload));
    let manifest = format!(
        "{{\"id\":\"i1\",\"name\":\"x.bin\",\"size\":{},\"sha256\":\"{digest}\",\"kind\":\"file\"}}\n",
        payload.len()
    );

    let mut raw = RawClient::connect(addr).await;
    let session_id = raw.prepare(&manifest).await;
    let (status, _) = raw.put(&session_id, "i1", &payload[..1024], None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let dir = root.join(".flit").join("parts").join(&session_id);
    assert!(dir.join("0.part").exists());

    assert_eq!(raw.delete(&session_id).await, StatusCode::NO_CONTENT);
    assert!(!dir.exists(), "cancelling removes the partial bytes");
    assert!(!root.join("x.bin").exists());

    // Forgotten, not merely marked: the id is gone and a second cancel 404s.
    let (status, _) = raw.put(&session_id, "i1", &payload[..1024], Some(0)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(raw.delete(&session_id).await, StatusCode::NOT_FOUND);

    // The same manifest after a cancel starts a new session rather than
    // adopting the dead one.
    assert_ne!(raw.prepare(&manifest).await, session_id);

    let _ = stop.send(());
    handle.await.unwrap();
    std::fs::remove_dir_all(&root).ok();
}

/// A UI reads these counters while the send is still in flight, so they must be
/// complete by the time it returns and must count pack and PUT bytes alike.
#[tokio::test]
async fn progress_reaches_the_declared_total_for_every_item() {
    let identity = Identity::from_secret_bytes([42u8; 32]);
    let root = scratch("progress");
    let sender_dir = scratch("progress-src");

    let big = sender_dir.join("big.bin");
    let big_bytes: Vec<u8> = (0..512 * 1024).map(|index| (index % 251) as u8).collect();
    std::fs::write(&big, &big_bytes).unwrap();
    let small = sender_dir.join("small.txt");
    let small_bytes = b"a small file that rides a pack";
    std::fs::write(&small, small_bytes).unwrap();
    let empty = sender_dir.join("empty.txt");
    std::fs::write(&empty, b"").unwrap();

    let targets = vec![
        SendTarget {
            path: big,
            name: Some("big.bin".to_string()),
        },
        SendTarget {
            path: small,
            name: Some("small.txt".to_string()),
        },
        SendTarget {
            path: empty,
            name: Some("empty.txt".to_string()),
        },
    ];

    let (server, addr) = start(&identity, &root).await;
    let (stop, handle) = spawn(server);

    let client = Client::new(addr, &identity.fingerprint()).unwrap();
    let mut connection = client.connect().await.unwrap();
    let progress = SendProgress::new();
    let outcomes = connection
        .send_files_with(&targets, Some(&progress))
        .await
        .expect("send");
    connection.close();

    assert_eq!(outcomes.len(), 3);
    assert_eq!(progress.total_files(), 3);
    assert_eq!(progress.files_done(), 3, "every item got a result");
    assert_eq!(
        progress.total_bytes(),
        big_bytes.len() as u64 + small_bytes.len() as u64,
        "total is the declared size of every item"
    );
    assert_eq!(
        progress.sent_bytes(),
        progress.total_bytes(),
        "a finished send has sent exactly what it declared"
    );
    assert_eq!(progress.fraction(), 1.0);

    let _ = stop.send(());
    handle.await.unwrap();
    std::fs::remove_dir_all(&root).ok();
    std::fs::remove_dir_all(&sender_dir).ok();
}

/// Cancel is cooperative but prompt: it must stop inside a large file, ask the
/// receiver to drop the session, and leave no partial file or part bytes.
#[tokio::test]
async fn cancelling_a_send_drops_the_session_and_writes_nothing() {
    let identity = Identity::from_secret_bytes([43u8; 32]);
    let root = scratch("cancel-progress");
    let sender_dir = scratch("cancel-progress-src");

    let source = sender_dir.join("stubborn.bin");
    let payload = vec![7u8; 64 * 1024 * 1024];
    std::fs::write(&source, &payload).unwrap();

    let (server, addr) = start(&identity, &root).await;
    let (stop, handle) = spawn(server);

    let client = Client::new(addr, &identity.fingerprint()).unwrap();
    let mut connection = client.connect().await.unwrap();
    let progress = SendProgress::new();

    let watcher = progress.clone();
    let cancelling = tokio::spawn(async move {
        while watcher.sent_bytes() < 1024 * 1024 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        watcher.cancel();
    });

    let outcome = connection
        .send_files_with(
            &[SendTarget {
                path: source,
                name: None,
            }],
            Some(&progress),
        )
        .await;
    cancelling.await.unwrap();
    connection.close();

    assert!(
        matches!(outcome, Err(ClientError::Cancelled)),
        "a cancelled send reports a cancel, not a network error: {outcome:?}"
    );
    assert!(!root.join("stubborn.bin").exists(), "nothing is installed");
    let parts = root.join(".flit").join("parts");
    assert!(
        !parts.is_dir() || std::fs::read_dir(&parts).unwrap().next().is_none(),
        "the session was dropped, not left for the sweeper"
    );

    let _ = stop.send(());
    handle.await.unwrap();
    std::fs::remove_dir_all(&root).ok();
    std::fs::remove_dir_all(&sender_dir).ok();
}
