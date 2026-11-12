//! Reproducible benchmark harness.
//!
//! Every number published in `docs/benchmarks.md` comes from this binary, with
//! the hardware and link type recorded alongside it. Loopback results are
//! labelled as loopback and are never used as a product claim.
//!
//! Scenarios run a real server and a real pinned client in one process, so the
//! measured path is the shipping one: TLS, hashing, sessions, verification and
//! installation. What is *not* measured is connection setup, file generation
//! and cleanup.

#![deny(missing_docs)]

use std::path::PathBuf;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use flit_api::DeviceType;
use flit_client::{Client, FileOutcome, SendTarget};
use flit_crypto::Identity;
use flit_server::{Server, ServerConfig};
use tokio::sync::oneshot;

/// A single measured result, with enough context to reproduce it.
#[derive(Debug, Clone)]
pub struct Measurement {
    /// What was measured, e.g. `single-file-put`.
    pub name: &'static str,
    /// Transfer size in bytes.
    pub bytes: u64,
    /// Elapsed time in seconds.
    pub seconds: f64,
    /// Where the traffic ran, e.g. `loopback` or `wifi6-5ghz`.
    pub link: &'static str,
    /// How many files the scenario sent.
    pub files: usize,
}

impl Measurement {
    /// Throughput in mebibytes per second.
    #[must_use]
    pub fn mib_per_second(&self) -> f64 {
        if self.seconds <= 0.0 {
            return 0.0;
        }
        (self.bytes as f64 / (1024.0 * 1024.0)) / self.seconds
    }

    /// Files completed per second.
    #[must_use]
    pub fn files_per_second(&self) -> f64 {
        if self.seconds <= 0.0 {
            return 0.0;
        }
        self.files as f64 / self.seconds
    }
}

/// A running receiver plus the scratch directories around it.
struct Fixture {
    fingerprint: String,
    addr: std::net::SocketAddr,
    stop: Option<oneshot::Sender<()>>,
    handle: tokio::task::JoinHandle<()>,
    root: PathBuf,
    source: PathBuf,
}

impl Fixture {
    async fn start() -> Self {
        let identity = Identity::from_secret_bytes([9u8; 32]);
        let fingerprint = identity.fingerprint();
        let root = unique_dir("recv");
        let source = unique_dir("src");

        let config = ServerConfig::default()
            .with_alias("Bench")
            .with_device_type(DeviceType::Laptop)
            .with_bind(std::net::SocketAddr::from(([127, 0, 0, 1], 0)))
            .with_root(&root);
        let server = Server::bind(&identity, config).await.expect("bind");
        let addr = server.local_addr();

        let (stop, stopped) = oneshot::channel();
        let handle = tokio::spawn(async move {
            let _ = server
                .run_until(async move {
                    let _ = stopped.await;
                })
                .await;
        });

        Self {
            fingerprint,
            addr,
            stop: Some(stop),
            handle,
            root,
            source,
        }
    }

    async fn send(&self, targets: &[SendTarget]) -> Vec<FileOutcome> {
        let client = Client::new(self.addr, &self.fingerprint).expect("client");
        let mut connection = client.connect().await.expect("connect");
        let outcomes = connection.send_files(targets).await.expect("send");
        connection.close();
        outcomes
    }

    async fn shutdown(self) {
        if let Some(stop) = self.stop {
            let _ = stop.send(());
        }
        let _ = self.handle.await;
        std::fs::remove_dir_all(&self.root).ok();
        std::fs::remove_dir_all(&self.source).ok();
    }
}

fn unique_dir(tag: &str) -> PathBuf {
    let unique = format!(
        "flit-bench-{tag}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    );
    let path = std::env::temp_dir().join(unique);
    std::fs::create_dir_all(&path).expect("scratch dir");
    path
}

/// One large file through `PUT`, the path every big transfer takes.
async fn single_file(fixture: &Fixture, size: u64) -> Measurement {
    let path = fixture.source.join("large.bin");
    let file = std::fs::File::create(&path).expect("create");
    file.set_len(size).expect("sparse file");
    drop(file);

    let targets = vec![SendTarget {
        path: path.clone(),
        name: Some("large.bin".to_string()),
    }];
    let started = Instant::now();
    let outcomes = fixture.send(&targets).await;
    let seconds = started.elapsed().as_secs_f64();
    assert_ok(&outcomes);
    assert_eq!(
        std::fs::metadata(fixture.root.join("large.bin"))
            .expect("received")
            .len(),
        size
    );

    Measurement {
        name: "single-file-put",
        bytes: size,
        seconds,
        link: "loopback",
        files: 1,
    }
}

/// Many small files through `pack`, which is what the 10k target exercises.
async fn pack_files(fixture: &Fixture, count: usize, size: u32) -> Measurement {
    let dir = fixture.source.join("many");
    std::fs::create_dir_all(&dir).expect("many dir");
    let block: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();

    let mut targets = Vec::with_capacity(count);
    for index in 0..count {
        let name = format!("file-{index:05}.bin");
        let path = dir.join(&name);
        std::fs::write(&path, &block).expect("write");
        targets.push(SendTarget {
            path,
            name: Some(name),
        });
    }

    let started = Instant::now();
    let outcomes = fixture.send(&targets).await;
    let seconds = started.elapsed().as_secs_f64();
    assert_ok(&outcomes);

    Measurement {
        name: "pack-small-files",
        bytes: u64::from(size) * count as u64,
        seconds,
        link: "loopback",
        files: count,
    }
}

fn assert_ok(outcomes: &[FileOutcome]) {
    let failed: Vec<String> = outcomes
        .iter()
        .filter_map(|outcome| match &outcome.result {
            Ok(_) => None,
            Err(err) => Some(format!("{}: {err}", outcome.path.display())),
        })
        .collect();
    assert!(
        failed.is_empty(),
        "{} file(s) failed: {failed:?}",
        failed.len()
    );
}

fn usage() -> ! {
    eprintln!(
        "usage: flit-bench [--files N] [--small-bytes N] [--large-bytes N] [--only single|pack]"
    );
    std::process::exit(2);
}

#[tokio::main]
async fn main() {
    let mut files = 10_000usize;
    let mut small_bytes = 4096u32;
    let mut large_bytes = 256u64 * 1024 * 1024;
    let mut only: Option<String> = None;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--files" => files = parse(&mut args, "--files"),
            "--small-bytes" => small_bytes = parse::<u32>(&mut args, "--small-bytes"),
            "--large-bytes" => large_bytes = parse::<u64>(&mut args, "--large-bytes"),
            "--only" => {
                let value = args.next().unwrap_or_else(|| usage());
                only = Some(value);
            }
            "-h" | "--help" => usage(),
            other => {
                eprintln!("unknown argument: {other}");
                usage();
            }
        }
    }

    println!(
        "flit-bench {} (protocol {})",
        env!("CARGO_PKG_VERSION"),
        flit_api::PROTO_VERSION
    );
    println!("link: loopback (127.0.0.1), one process, one machine");
    println!();
    println!(
        "{:<18} {:>8} {:>12} {:>10} {:>9} {:>10}",
        "scenario", "files", "bytes", "seconds", "MiB/s", "files/s"
    );

    let fixture = Fixture::start().await;
    let mut measurements = Vec::new();

    if only.as_deref().is_none_or(|name| name == "single") {
        measurements.push(single_file(&fixture, large_bytes).await);
    }
    if only.as_deref().is_none_or(|name| name == "pack") {
        measurements.push(pack_files(&fixture, files, small_bytes).await);
    }

    for measurement in &measurements {
        println!(
            "{:<18} {:>8} {:>12} {:>10.2} {:>9.1} {:>10.1}",
            measurement.name,
            measurement.files,
            measurement.bytes,
            measurement.seconds,
            measurement.mib_per_second(),
            measurement.files_per_second()
        );
    }

    fixture.shutdown().await;
}

fn parse<T: std::str::FromStr>(args: &mut impl Iterator<Item = String>, flag: &str) -> T {
    match args.next().and_then(|value| value.parse().ok()) {
        Some(value) => value,
        None => {
            eprintln!("{flag} needs a number");
            usage();
        }
    }
}
