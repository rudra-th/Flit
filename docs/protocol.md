# Flit protocol, version 1

**Status:** draft, normative for M1
**Wire version:** `1`
**Last updated:** 2026-11-08

This document describes what crosses the wire between two Flit devices. When it
disagrees with the code in `crates/flit-api`, this document is right and the code
is a bug.

Flit is LAN-only in v1. There is no server in the middle: a transfer is two
devices talking directly to each other.

---

## 1. Versioning

Flit versions its protocol by **major version only**.

| Where | Field | Example |
|---|---|---|
| Path prefix | `/api/v1` | major version 1 |
| Request header | `X-Flit-Proto: 1` | echoed on every request |
| `/api/v1/info` body | `"proto": 1` | authoritative |
| Discovery announce | `"proto": 1` | lets a peer filter before dialling |

Rules:

1. A peer whose `proto` differs from ours is refused with `415 unsupported`. It
   is never partially served.
2. New optional fields may be added at any time. Receivers ignore unknown JSON
   fields, and unknown capability tokens are ignored. Neither is a version bump.
3. Removing a field, or changing what an existing field means, is a major
   version bump and gets an entry in the compatibility table (§10).
4. Capabilities are additive hints, never requirements. A peer that omits a
   capability simply does not get that feature.

---

## 2. Ports

| Purpose | Transport | Port |
|---|---|---|
| Discovery announce | UDP multicast to `239.255.42.99` | 47820 |
| Control and data | HTTPS (TLS 1.3) | 47821 |
| Browser receive session | HTTP (no TLS) | 47822 |

The TLS listener may fail to bind 47821 and fall back to an ephemeral port. In
that case the **announce carries the real port** and peers must use it. Clients
never assume 47821; they use the advertised value.

Port 47822 is bound only while a browser session is open and never accepts
uploads. See §8.

---

## 3. Discovery

### 3.1 Multicast announce

Every two seconds, and immediately on start, a node sends a JSON object to
`239.255.42.99:47820`, TTL 1:

```json
{
  "proto": 1,
  "alias": "Rudra's laptop",
  "device_id": "0123456789abcdef",
  "fingerprint": "9f2c…64 hex characters…",
  "device_type": "laptop",
  "port": 47821,
  "caps": ["resume", "pack", "verify", "text", "progress-sse"],
  "pin_required": false
}
```

A peer that receives an announce replies unicast to the source address and port
with its own announce. There is no distinguishing field: both sides merge by
`device_id`, and duplicates are expected and harmless.

The payload is deliberately small. It carries a display name, a device class and
a public fingerprint — nothing about files, sizes, or history.

### 3.2 Subnet scan

When multicast is blocked — common on guest and corporate networks — a client
scans its own /24 with `GET /api/v1/info` over TLS, concurrency 32, timeout
300 ms per host. A host that answers is a device.

The scan is the *primary* fallback precisely because it does not rely on
multicast working at all.

### 3.3 QR and manual entry

For cross-subnet and AP-isolated cases, the receiving device can display a QR
containing `flit://<ip>:<port>?fp=<fingerprint>&t=<one-time-token>`. The
fingerprint in the QR is authoritative for that session.

---

## 4. Identity and TLS

- Each device generates an Ed25519 keypair on first run.
- `fingerprint = SHA-256(public key)`, lowercase hex, 64 characters.
- `device_id = fingerprint[0..16]`, for logs and UI.
- The TLS certificate is self-signed and derived from the identity key, so the
  certificate changes only when the identity changes.
- The **receiver** pins the fingerprint advertised in the announce or the QR.
  A certificate that does not match is a hard failure. There is no
  "continue anyway" path, and no API to request one.

### 4.1 PIN

`pin_required` in the announce and in `/info` tells a peer that a PIN is needed
for first contact. A PIN is 4 to 12 ASCII digits. It is sent as a header on every
request that is not `GET /api/v1/info`:

```
X-Flit-Pin: 482913
```

Rules:

- The PIN is only ever sent inside TLS. It is never hashed on the wire and never
  stored by the sender.
- `/info` is exempt. Discovery and pinning happen before a peer can know that a
  PIN exists, and `pin_required` in that response is how it finds out. Every
  other endpoint needs the PIN, including `DELETE`.
- Three failed attempts per source address per 60 seconds, then `403
  pin_incorrect` for the rest of that window — **including with the correct
  PIN**, so the limit cannot be guessed through. A missing PIN is not an attempt:
  nothing was guessed. Counting per device rather than per address needs a
  proven device identity, which arrives with `POST /api/v1/register`.
- A wrong PIN and a missing PIN are distinct errors (`pin_incorrect` versus
  `pin_required`) so the UI can say something true. "Wrong" and "locked out"
  deliberately share one message, so the error cannot be used to count attempts.
- A receiver with no PIN configured ignores the header entirely.

---

## 5. HTTP conventions

Every request carries:

```
X-Flit-Proto: 1
```

A sender that has an identity of its own also sends `X-Flit-Device: <device_id>`
as a display and logging hint. Receivers never require it and never authorize on
it: in v1 the only identity that is proven is the certificate's, and the client
side of a transfer has no identity of its own until pairing exists.

Bodies are UTF-8 JSON unless stated otherwise. Responses are UTF-8 JSON.

### 5.1 Errors

Every non-2xx response has this body:

```json
{
  "code": "pin_incorrect",
  "message": "three attempts per minute",
  "retryable": false
}
```

`code` is the machine-readable field. `message` is for developers and logs. UI
text is derived from `code`, never from `message`.

| HTTP | `code` | Meaning |
|---|---|---|
| 400 | `bad_request` | Malformed body or failed validation |
| 401 | `unauthorized` | No session, or session not owned by this device |
| 403 | `pin_required` | A PIN is needed to continue |
| 403 | `pin_incorrect` | Wrong PIN |
| 404 | `not_found` | Unknown session or item |
| 409 | `conflict` | Item collides with existing state |
| 413 | `too_large` | Declared size or manifest exceeds a limit |
| 415 | `unsupported` | Protocol version or capability mismatch |
| 500 | `internal` | The receiver cannot act on this |
| 507 | `disk_full` | The receiver ran out of space |

---

## 6. Endpoints

Endpoints described here but not yet served by a build answer
`404 not_found` and are absent from that build's `caps`. A peer must treat a
missing capability as "this feature is unavailable", never as an error. At the
time of writing, §6.2, §6.5, §6.7 and §6.9 are specified but not implemented.

### 6.1 `GET /api/v1/info`

Unauthenticated by design: it is what discovery and pinning are built on.

`caps` lists only what the answering build can actually serve. The list below is
the full v1 vocabulary; a build that has not implemented a feature omits its
token, and its peers must not attempt that feature.

```json
{
  "proto": 1,
  "alias": "Rudra's laptop",
  "device_id": "0123456789abcdef",
  "fingerprint": "9f2c…",
  "device_type": "laptop",
  "port": 47821,
  "version": "0.1.0",
  "caps": ["resume", "pack", "verify", "text", "progress-sse"],
  "pin_required": false
}
```

### 6.2 `POST /api/v1/register`

First request of a session. Exchanges device identity and proves key ownership
with a detached signature over a server-issued nonce.

Request:

```json
{ "device_id": "0123456789abcdef", "alias": "Phone", "nonce": "…", "signature": "…" }
```

Response: `{ "session": "s-…", "server_nonce": "…" }`

### 6.3 `POST /api/v1/upload`

Prepare a session. The body is a **streamed NDJSON manifest**: one
`ManifestItem` per line, so 50,000 files never becomes one in-memory blob.

```json
{"id":"a1","name":"photo.jpg","size":4829134,"sha256":"…","mtime":1731000000,"kind":"file"}
{"id":"a2","name":"notes/readme.txt","size":812,"sha256":"…","kind":"file"}
{"id":"a3","name":"clipboard.txt","kind":"text","text":"https://example.com"}
```

Response `201`:

```json
{
  "session_id": "s-7f3a",
  "accepted": ["a1", "a2", "a3"],
  "rejected": [{ "id": "a4", "reason": "too_large" }],
  "offset": 0
}
```

Items are validated on declared shape before any file is opened. Limits are in
§7. A rejection is per item: one bad item does not fail the session.

If **every** item is rejected there is nothing to do, so the receiver answers
`400 bad_request` and creates no session. A sender that gets this must fix its
manifest rather than retry the same request.

### 6.4 `PUT /api/v1/upload/{session}/{item}`

Send one file's bytes. Body is the raw file.

- First attempt: no `Content-Range`, body starts at byte 0.
- Resume: `Content-Range: bytes 1048576-` (or `bytes 1048576-5242879/5242880`).
- If the receiver already has **more** bytes than the range start, it returns
  `308` with `X-Flit-Offset: <committed>` and an empty body. The client resumes
  from that offset.
- If the range start is **beyond** what the receiver has, that is a request to
  skip bytes that were never sent: `409 conflict`, and the sender restarts the
  item from the receiver's offset.
- Otherwise the receiver appends, computes SHA-256 on the same pass that writes,
  and returns `204` when the item is fully received.

Bytes are written to a `.part` file. Nothing is renamed into place until §6.6.

### 6.5 `POST /api/v1/upload/{session}/pack`

Batched upload for many small files. The body is a sequence of entries until the
body ends:

```
u32 little-endian header length
header bytes (UTF-8 JSON: { "id", "name", "size", "sha256" })
size bytes of payload
repeat
```

Every item in a pack must be at most 256 KiB as declared in the manifest. A
4-byte header length of `0` is invalid and ends the pack with `bad_request`.

### 6.6 `POST /api/v1/upload/{session}/finish`

Verification and commit.

Request: `{ "item_ids": ["a1", "a2"] }`

For each item the receiver compares the running SHA-256 to the manifest value,
resolves conflicts (`report (2).pdf`), and installs `.part` under its real name.
Installation must be atomic and must never overwrite: a receiver that cannot
link or rename a finished item into place reports `verified: false` rather than
falling back to a destructive copy.

Response:

```json
{
  "items": [
    { "id": "a1", "verified": true, "saved_as": "photo.jpg" },
    { "id": "a2", "verified": false }
  ]
}
```

A `verified: false` item is not renamed and is reported to the sender. The
sender may re-send that item's byte range and finish again.

`finish` is idempotent per item: an item that already verified is not installed
twice. A session in which some items verified and others did not **stays open**,
so the sender can repair the rest and call `finish` again; only a session in
which every requested item verified is closed. A sender that gives up should
`DELETE` the session (§6.8) instead of leaving it for the receiver's sweep.

### 6.7 `GET /api/v1/upload/{session}/events`

Server-sent events, `text/event-stream`, one JSON object per `data:` line:

```json
{ "item": "a1", "bytes": 1048576, "total": 4829134, "done": false }
```

Requires the `progress-sse` capability. A peer without it polls nothing; the
sender's own byte counters are enough for its UI.

### 6.8 `DELETE /api/v1/upload/{session}`

Cancel. The receiver stops accepting bytes, deletes `.part` files for the
session, and returns `204`. Either side may cancel; both sides converge on the
same state.

### 6.9 `POST /api/v1/text`

Send text or a link as a first-class item.

```json
{ "text": "https://example.com", "label": "from clipboard" }
```

---

## 7. Limits

Enforced before any I/O:

| Limit | Value | Constant |
|---|---|---|
| Items per session | 100,000 | `MAX_ITEMS` |
| Manifest body | 64 MiB | `MAX_MANIFEST_BYTES` |
| Single item | 1 TiB | `MAX_FILE_BYTES` |
| Session total | 8 TiB | `MAX_TOTAL_BYTES` |
| Pack item | 256 KiB | `PACK_THRESHOLD` |

A manifest that exceeds any of these is rejected whole, with `413`. It is never
partially applied.

---

## 8. Browser receive

The browser client exists because HTTP does. It is receive-only.

- Opening `/r/{token}` on port 47822 serves a static page.
- The token is single-use, expires in 10 minutes, and is bound to one IP.
- The port is bound only while a session is open, and only accepts `GET`s for
  the page and `POST`s for `finish`. It never accepts uploads.
- Plain HTTP is deliberate: a self-signed certificate would show a browser
  warning, and a warning trains users to click through warnings. The token, the
  binding and the short lifetime are the trade instead.

---

## 9. Security notes

- Discovery leaks: alias, device class, public fingerprint. Nothing else.
- The fingerprint protects against a passive listener and against a peer
  impersonating a device you already paired with. It does **not** protect a
  first contact against an active man-in-the-middle. The PIN and the safety
  badge are the visible mitigations, and both are shown before the first byte
  moves.
- Every incoming name is hostile: paths are sanitized segment by segment, and
  traversal, absolute paths, reserved device names and control characters are
  rejected rather than rewritten.
- The receiver never trusts a declared size. Disk space is checked before
  accepting, and writes stop at the declared length.
- Telemetry: none.

---

## 10. Compatibility

| Protocol | App versions | Notes |
|---|---|---|
| 1 | 0.1.x | Initial version. |

There is no protocol 0. Any future version bumps the path prefix and the
`X-Flit-Proto` header together, and the table above gains a row.
