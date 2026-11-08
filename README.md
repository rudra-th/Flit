# Flit

**Files, straight across.** Send a folder from one device to another over your
own network. No account, no cloud, no configuration.

> **Status: pre-alpha.** The protocol is specified, and identity, path safety and
> the M1 transport work: a pinned TLS 1.3 connection that sends one file,
> verifies it end to end, and installs it under a non-colliding name. Discovery,
> the UIs and the browser page are still ahead. Nothing is usable yet.

---

## What it will be

- **Fast.** Built for wired and Wi-Fi 6 LAN speeds, with a benchmark harness in
  the repo so the numbers can be checked. The loopback numbers measured so far
  are in [`docs/benchmarks.md`](docs/benchmarks.md), including the parts that are
  slower than they should be.
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
  flit-crypto/     Ed25519 identity, fingerprints, safety badges, key file
  flit-storage/    hostile-path sanitizing, conflict naming
  flit-discovery/  multicast announce, subnet scan, QR       (M2)
  flit-client/     dial, pin, streaming send, resume         (M1)
  flit-server/     TLS listener, sessions, verify, commit    (M1)
  flit-core/       engine facade for every UI
  flit-cli/        headless binary: serve, send, doctor, identity
  flit-bench/      reproducible benchmarks
docs/
  protocol.md      normative wire specification
  benchmarks.md    measured numbers, method and known gaps
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

## Trying the transport

Two terminals on one machine are enough to see the M1 pipeline work:

```sh
# terminal 1
cargo run -p flit-cli -- identity        # prints the fingerprint to pin
cargo run -p flit-cli -- serve --bind 127.0.0.1 --dir ./recv

# terminal 2
cargo run -p flit-cli -- send ./some-file.bin \
    --to 127.0.0.1:47821 --fingerprint <fingerprint from terminal 1>
```

A wrong fingerprint fails during the handshake, before any request is sent: that
is the whole point of pinning the key inside the certificate.

## Licence

MIT OR Apache-2.0, at your option.
