//! The headless Flit binary.
//!
//! `flit doctor` is the same diagnostic the desktop and Android shells will
//! show: build and protocol facts first, then a set of self-checks that fail
//! loudly. It exists from M0 because "device not found" is the most common
//! support question in this category, and a project with no crash reporting
//! needs a diagnostic that a user can paste into an issue.

use clap::{Parser, Subcommand};

/// Files, straight across.
#[derive(Debug, Parser)]
#[command(name = "flit", version, about = "Files, straight across.", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

/// What to do.
#[derive(Debug, Subcommand)]
enum Command {
    /// Print build facts and run the built-in self-checks.
    Doctor,
}

fn main() {
    let cli = Cli::parse();
    let code = match cli.command {
        Some(Command::Doctor) | None => doctor(),
    };
    std::process::exit(code);
}

/// Print facts, run self-checks, and return a process exit code.
fn doctor() -> i32 {
    println!("flit {}", env!("CARGO_PKG_VERSION"));
    println!("  protocol      {}", flit_api::PROTO_VERSION);
    println!(
        "  platform      {} {}",
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    println!("  discovery     udp {}", flit_api::DISCOVERY_PORT);
    println!("  control       tcp {}", flit_api::TLS_PORT);
    println!("  browser       tcp {}", flit_api::BROWSER_PORT);
    println!();

    let mut failures = 0;

    // Identity: a keypair must produce a stable, well-formed fingerprint.
    let identity = flit_crypto::Identity::from_secret_bytes([42u8; 32]);
    failures += report(
        "identity fingerprint",
        identity.fingerprint().len() == 64 && identity.device_id().len() == 16,
        &format!(
            "{} / {} / {}",
            identity.device_id(),
            identity.badge_emoji(),
            identity.badge_words()
        ),
    );

    // Signatures must round-trip and must not verify under tampering.
    let signed = identity.sign(b"doctor");
    let signature_ok = flit_crypto::verify(&identity.public_key(), b"doctor", &signed);
    let tamper_ok = !flit_crypto::verify(&identity.public_key(), b"doctoR", &signed);
    failures += report(
        "signature round trip",
        signature_ok && tamper_ok,
        "sign, verify, reject tampered message",
    );

    // Path safety: the rules that stop a hostile manifest from escaping.
    let traversal_blocked = flit_storage::sanitize_relative("../etc/passwd").is_err();
    let absolute_blocked = flit_storage::sanitize_relative("/etc/passwd").is_err();
    let device_blocked = flit_storage::sanitize_segment("CON.txt").is_err();
    failures += report(
        "path safety",
        traversal_blocked && absolute_blocked && device_blocked,
        "traversal, absolute paths and reserved names rejected",
    );

    // Conflict naming must never silently overwrite.
    let renamed = flit_storage::conflict_name("report.pdf", 2);
    failures += report("conflict naming", renamed == "report (2).pdf", &renamed);

    println!();
    if failures == 0 {
        println!("all checks passed");
        0
    } else {
        println!("{failures} check(s) failed");
        1
    }
}

/// Print one self-check and return 0 or 1 so callers can accumulate failures.
fn report(name: &str, ok: bool, detail: &str) -> i32 {
    let mark = if ok { "ok  " } else { "FAIL" };
    println!("  {mark}  {name:<24} {detail}");
    i32::from(!ok)
}
