# Security

Flit moves files between devices that already share a network. It is pre-alpha:
the protocol is specified and implemented, but no external audit has happened,
and the threat model below is what the code defends today.

## Reporting a problem

Please do not open a public issue for a security bug.

Use GitHub's private advisory form:
<https://github.com/rudra-th/Flit/security/advisories/new>. A useful report says
which commit you ran, the OS on both ends, what you expected, what happened, and
the smallest reproduction you have. Expect an acknowledgement within a week.

## What Flit assumes

- The network is hostile, but the two devices are not. Anyone on the same LAN can
  send traffic to a listening device, so every byte from a sender is treated as
  untrusted input.
- The receiver's key is learned once, out of band or from a multicast announce,
  and then pinned. A certificate for any other key is refused before a request
  is sent.

## What it defends with

- **Pinned TLS 1.3.** The receiver's Ed25519 identity key is inside its
  certificate, so the device fingerprint *is* the key that terminates the
  session. Pinning the fingerprint is pinning the key; there is no CA in the
  path.
- **Path safety.** Every offered name is sanitised before it reserves a path:
  absolute paths, `..`, Windows device names, control characters and over-long
  segments are rejected, and one bad name does not fail the rest of a batch.
- **Verification before install.** Each file is hashed while it is written, and
  it only gets its real name — via a hard link from the `.part` file — after the
  hash matches what the sender announced. An interrupted or tampered transfer
  leaves nothing installed.
- **Optional PIN.** A receiver can require a code for first contact; three wrong
  guesses from one address cost a minute, and every endpoint except `/info`
  needs it.
- **Local key hygiene.** The identity file is created owner-only (0600), and the
  secret is zeroized when dropped.

## What it does not defend against

- **First contact.** The fingerprint has to come from somewhere you trust — a QR
  code, the other screen, discovery on a network you trust. On a network where
  someone can answer in the receiver's place before you have pinned anything,
  compare the safety badge out of band.
- **Metadata.** Sizes, names and timing are visible to anyone watching the wire;
  the payload is encrypted, the shape of the transfer is not.
- **Compromise of either device.** Flit protects the wire, not the endpoints.
- **Availability.** The PIN backoff slows guessing, but there is no general rate
  limit, and a malicious peer can open sessions it never finishes until the
  receiver's limits reject it.
- **Files at rest.** What lands on disk is an ordinary file with ordinary
  permissions; there is no encryption layer above the filesystem.
- **A booby-trapped receive folder.** Names are sanitised, but the receiver
  writes into a folder you chose: if that folder already contains a symlinked
  directory, an offered name can follow it. Receive into a folder you control.

## Scope

The Rust crates (`flit-api`, `flit-crypto`, `flit-storage`, `flit-discovery`,
`flit-client`, `flit-server`, `flit-core`), the desktop shell in
`apps/desktop/`, and the published page in `site/`. Third-party dependencies are
out of scope except where Flit uses them in a way that removes a guarantee above.
