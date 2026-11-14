# Flit

**Files, straight across.** Send a folder from one device to another over your
own network. No account, no cloud, no configuration.

> **Status: pre-alpha.** The protocol is specified, and the engine works:
> identity, path safety, the pinned TLS 1.3 transport, discovery, multi-file
> sessions, inline text items, and resume across a sender restart. A folder with
> ten thousand files goes over in one session, every item is verified end to end
> before it gets its real name, and re-running an interrupted send streams only
> the bytes the receiver still needs. The desktop window is built on the engine
> facade (`flit-core`): nearby devices, drag and drop, live progress with a
> cancel, past sends, and a network doctor. The Android app and the browser page
> are still ahead.

**[getflit.surge.sh](https://getflit.surge.sh)** is the page that explains it: the
three moves, a real size-to-time figure, and five promises you can switch off.
One static page, no framework, no third-party request; the source and design
notes are in [`site/`](site/README.md).

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
  flit-discovery/  multicast announce, subnet scan, peer table
  flit-client/     dial, pin, streaming send, resume, probe
  flit-server/     TLS listener, sessions, verify, commit
  flit-core/       engine facade for every UI
  flit-cli/        headless binary: serve, send, doctor, identity
  flit-bench/      reproducible benchmarks
apps/
  desktop/         React 19 window: devices, send tray, history, doctor
    src-tauri/     Tauri shell: engine host, commands, tray icon
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

CI runs the suite on Linux, Windows and macOS. If a local policy blocks a freshly
built executable — Windows Smart App Control is one — the same suite runs in a
container without changing anything else:

```sh
docker run --rm -v "$PWD:/work" -w /work rust:1.99-bookworm cargo test --workspace
```

## The desktop window

```sh
cd apps/desktop
npm install
npm run tauri dev
```

The window puts the devices on the network in front of you: drop files on it,
pick a device, and the lane shows what has crossed the wire with a cancel beside
it. Paste puts whatever is on the clipboard in the tray as text — a link or a
note leaves without a file ever touching the disk. Closing the window does not
stop receiving — Flit stays in the tray, and quitting is a tray menu item,
because a transfer app that stops listening the moment its window is dismissed
is not doing its job.

`npm run dev` opens the same window in a browser against a demo engine, which is
how the design gets reviewed without a display. That demo code is compiled out of
a release build; it is not reachable in a shipped binary.

The Rust half is its own workspace so `cargo test --workspace` never needs
webkit2gtk. To gate it, use the image that has the window libraries:

```sh
docker build -t flit-desktop-gate -f apps/desktop/gate.Dockerfile apps/desktop
docker run --rm -v "$PWD:/src" -w /src/apps/desktop/src-tauri \
  flit-desktop-gate cargo clippy --all-targets -- -D warnings
```

CI runs the same check, plus the frontend build, on every push.

## Trying the transport

Two terminals on one machine are enough to see the M1 pipeline work:

```sh
# terminal 1
cargo run -p flit-cli -- identity        # prints the fingerprint to pin
cargo run -p flit-cli -- serve --bind 127.0.0.1 --dir ./recv

# terminal 2
cargo run -p flit-cli -- send ./some-file.bin \
    --to 127.0.0.1:47821 --fingerprint <fingerprint from terminal 1>

# several files, or a whole folder, in one session
cargo run -p flit-cli -- send notes.txt photos/ \
    --to 127.0.0.1:47821 --fingerprint <fingerprint from terminal 1>

# a link or a note: the text itself, carried inline in the manifest
cargo run -p flit-cli -- send --text "https://example.com/q3" \
    --name q3-link.txt --to 127.0.0.1:47821 --fingerprint <fingerprint from terminal 1>
```

A folder keeps its own name at the receiver: `send photos/` produces
`recv/photos/…`, including subdirectories. Each file is hashed before anything
is announced, and the sender prints one result per file plus a summary.

`send --text` offers a string instead of a path. It travels inside the manifest,
so there is no body to stream: the receiver hashes the UTF-8 bytes at commit,
installs them under `--name` (default `clipboard.txt`), and the result is an
ordinary file on the other side. An empty string is refused before dialling.

A wrong fingerprint fails during the handshake, before any request is sent: that
is the whole point of pinning the key inside the certificate.

Add `--pin 482913` to `serve` and the same `--pin` to `send` to require a code
for first contact. Every endpoint except `/info` then needs it, three wrong
guesses from one address cost a minute, and `/info` still answers so discovery
can learn that a PIN is wanted.

## Finding devices

`serve` announces the device on the local network, so a peer on the same network
can list it without knowing an address:

```sh
# terminal 1 (or another machine)
cargo run -p flit-cli -- serve --dir ./recv

# terminal 2
cargo run -p flit-cli -- discover
cargo run -p flit-cli -- discover --scan          # when multicast is filtered
cargo run -p flit-cli -- discover --json          # for scripts
```

`discover` listens for a few seconds and prints each device's address, device
id, how it was found, and its safety badge; the full fingerprint is printed at
the end, ready to paste into `send --fingerprint`. It does not announce itself,
because it has no control port to advertise.

`--scan` adds the fallback: a TLS `/info` probe across this machine's private
/24s, 32 at a time, 300 ms per host. Public subnets are never scanned, and a scan
that runs out of its 10 s budget says so instead of reporting an empty network.
Use `--scan-port` when the receiver is not on the default 47821.

`serve --no-announce` keeps a device off the list entirely — useful when the
network is shared and being visible is not wanted. A silent device can still
send, it just cannot be found by discovery.

## Security

What Flit defends against, what it explicitly does not, and how to report a
problem are in [`SECURITY.md`](SECURITY.md). Short version: pinned TLS 1.3,
verified files, untrusted names; no audit yet, so compare the safety badge out
of band the first time you trust a device.

## Licence

MIT OR Apache-2.0, at your option.
