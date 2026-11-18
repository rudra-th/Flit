# Roadmap

What is decided, what is deferred, and the order the remaining work has to
happen in. Protocol details live in [`protocol.md`](protocol.md); this file is
about scope and sequence.

## v1

Files and text, between the Android app, the desktop app and the CLI. Consent
is cryptographic (§4 of the protocol): a sender proves an identity with a
client certificate, the person answers a sheet, and "always accept" remembers
the fingerprint, never the name.

Nothing about receiving may require setup in advance. A person who has never
opened the app still sees a question, answers it, and gets the file.

## Decided

**Folder sending is v1.1.** A folder needs its own manifest shape, path rules
and a design pass; the sheet cannot say "3 items" for a tree of ninety files.
v1 sends what it can explain in one line.

**"Stay discoverable" means while the app is open.** No foreground service, no
persistent notification, no promise the battery has to keep. The receiver lives
as long as the process does, and the UI is honest about that: the Receive tab
is the state. Revisit only if people ask for receiving while the phone is in a
pocket.

**The browser page is reached by scanning a code.** The page in §8 is
receive-only and runs on a machine that has a browser and no Flit install. The
phone is the sender, so the phone scans: the page shows
`flit://<ip>:<port>?fp=<fingerprint>&t=<token>` and the app turns it into the
same manual target it would otherwise have typed.

## Order

1. **The browser page, before the scanner.** A scanner with nothing to scan is
   unverifiable. The page needs the token lifecycle and the port binding from
   §8, and it renders the code itself: SVG generated on the receiving side, so
   the page loads no script, makes no third-party request and works with the
   network unplugged.
2. **The scanner.** Camera preview and QR decoding in the app, offline, no
   Google services, no network. Camera permission is asked when a person taps
   "Scan a code" and never before. Result: a manual target, which the send path
   already understands, so no new consent or addressing rules.
3. **Consent on hardware.** Before either of the above ships, the matrix in
   `docs/verification.md` has to pass on two real phones.

## Not planned

- Typed addresses are never removed when the QR path lands. A code is faster,
  not a replacement for the escape hatch.
- No cloud relay, no account, no address book. The network is the address.
