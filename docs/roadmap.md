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
browser. The sending device — phone or desktop — serves them on port 47822 and
shows a short address, and the computer downloads from it. Nothing is installed
on the computer, and nothing is uploaded to the sender: the bytes go straight
from the sender's disk to the browser.

Port 47822 is tried first because it is the one a person may have seen on
another screen, but it is not a requirement. If something else already holds
it, the share opens on a free port and the address shows that port; the URL is
the whole instruction, so the fallback costs the person nothing.

The address is set in mono and never truncated, because half a URL read out
loud is a support call. A scannable code is a later addition: v1 keeps the
address readable and copyable, and a scanner in the app would only matter if
the phone were receiving from a screen, which is the opposite direction.

## Where it stands

1. **Consent.** The engine, the sheet, the saved policy and the trusted list are
   in place on the phone and on the desktop. The matrix in
   `docs/verification.md` is the gate and has been run through the identity
   cases on one phone; the remaining cases need a second LAN address and a
   second sender.
2. **The browser share.** The token lifecycle and read-only listener from §8,
   the page, and downloads that resume are in place. The page loads no script
   and makes no third-party request, so it works on a network with no internet.
3. **The share UI.** Both shells offer "Share in a browser", show the address
   and when the share ends, copy it, and stop it on request. The panel is the
   same instruction on either screen, and the address is never truncated.
4. **The first build anyone can run.** `v0.1.0` is published as a GitHub
   pre-release from a tag: the Windows installer, the portable zip and a debug
   APK, with checksums. CI builds all three, so the release holds exactly what
   the run passed. It is deliberately unsigned and labeled a preview; code
   signing and a store listing are the remaining release work, not engine work.

## Not planned

- Typed addresses are never removed when a code path lands. A code is faster,
  not a replacement for the escape hatch.
- No cloud relay, no account, no address book. The network is the address.
