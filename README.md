# Flit

**Files, straight across.** Send a folder from one device to another over your
own network. No account, no cloud, no configuration.

> **Status: pre-alpha.** The protocol is specified, the identity and path-safety
> layers are written, and the transport is next. Nothing is usable yet.

---

## What it will be

- **Fast.** Built for wired and Wi-Fi 6 LAN speeds, with a benchmark harness in
  the repo so the numbers can be checked.
- **Correct.** Every file is hashed and verified before it gets its real name.
  Interrupted transfers resume from the last committed byte.
- **Quiet.** No account, no telemetry, no analytics, no server in the middle.
- **Free.** MIT OR Apache-2.0, no paid tier, no ads.

Windows and Linux desktop, Android, and a browser page for receiving without
installing anything.

## How it works

Two devices on the same network talk directly to each other over
HTTP/1.1 inside TLS 1.3:

1. Devices announce themselves over UDP multicast, and fall back to scanning the
   local subnet when multicast is blocked.
2. The sender pins the receiver's Ed25519 fingerprint from that announce. A
   certificate that does not match is a hard failure.
3. Files are uploaded one `PUT` per file, with `Content-Range` for resume. Small
   files are batched into a single pack request, which is what makes ten thousand
   files finish in under a minute instead of ten.

The full specification is in [`docs/protocol.md`](docs/protocol.md).

## Repository layout

```
crates/
  flit-api/        wire types: info, manifest, errors, limits
  flit-crypto/     Ed25519 identity, fingerprints, safety badges
  flit-storage/    hostile-path sanitizing, conflict naming
  flit-discovery/  multicast announce, subnet scan, QR       (M2)
  flit-client/     dial, pin, send                            (M1)
  flit-server/     TLS control and data plane                 (M1)
  flit-core/       engine facade for every UI
  flit-cli/        headless binary, doctor, benchmark harness
  flit-bench/      reproducible benchmarks
docs/
  protocol.md      normative wire specification
```

## Building

Requires Rust 1.85 or newer; the pinned toolchain is in `rust-toolchain.toml`.

```sh
cargo test --workspace
cargo run -p flit-cli -- doctor
cargo run -p flit-cli -- --version
```

`flit doctor` prints build and protocol facts and runs the self-checks that back
the safety claims above — no network involved.

## Licence

MIT OR Apache-2.0, at your option.
