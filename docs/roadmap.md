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

**The browser page is served by the sender.** A person who wants to hand files
to a borrowed computer picks them as usual and chooses to open them in a
browser. The phone binds port 47822, shows a short address, and the computer
downloads from it. Nothing is installed on the computer, and nothing is
uploaded to the phone.

The page shows that address as a code as well as text, so a device with a
camera can scan it instead of typing. The code is drawn by the sender, because
the sender is the side that already has the files. A scanner in the app would
only be needed if the phone were the one receiving from a screen, which is the
opposite direction; v1 does not include one.

## Order

1. **Consent on hardware, before anything ships.** The matrix in
   `docs/verification.md` has to pass on two real phones. It is the gate, not a
   later chore, and it can run in parallel with the work below because the
   share path never touches consent.
2. **The browser share, sender side.** The token lifecycle and read-only
   listener from §8, the page, and downloads that resume. The page loads no
   script and makes no third-party request, so it works on a network with no
   internet.
3. **The share UI.** Pick files as usual, choose "open in a browser", show the
   address and the code, and stop the share when the person is done.

## Not planned

- Typed addresses are never removed when the QR path lands. A code is faster,
  not a replacement for the escape hatch.
- No cloud relay, no account, no address book. The network is the address.
