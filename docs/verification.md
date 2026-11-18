# Verifying consent on real hardware

Consent is the part of Flit that unit tests cannot finish: the question is
drawn by Compose, the answer comes from a tap, and the proof is a TLS handshake
on a phone. These are the cases, what each should look like, and what the wire
says. Run all of them before calling the feature done.

## Setup

- Two devices on the same Wi-Fi: the receiver (phone), and a sender that can
  present or omit a client certificate.
- Receiver: app installed, screen kept awake (`svc power stayon true`), battery
  optimisation off for `dev.flit.android`, Receive tab open. The receiver only
  listens while the process is alive.
- The sender needs: no certificate, an RSA certificate, and an Ed25519
  certificate. Only the Ed25519 one proves an identity the receiver can
  remember (§4.1).
- Read `/info` first: it returns the address the sender should use and must
  answer without any certificate.

## Cases

| # | Case | Expected |
|---|------|----------|
| 1 | `GET /api/v1/info` with no certificate | `200`, no sheet |
| 2 | Prepare with no certificate | Sheet says the sender did not prove an identity; no "always accept" row |
| 3 | Prepare with an RSA certificate | Same as 2: a certificate the receiver cannot read a key from is not an identity |
| 4 | Prepare with an Ed25519 certificate | Sheet shows the alias, the item list, the size, and the fingerprint as a monospace value |
| 5 | Decline | `403 declined`; the sheet closes; the trusted list does not change |
| 6 | No answer for 60 s | `408 no_answer`; the sheet closes by itself |
| 7 | Accept with "always accept" ticked | `201` with `session_id`; the device appears under Settings → Trusted devices, with its name |
| 8 | A fresh send from that same device | No sheet: `201` directly. The log says "trusted device … took its offer without asking" |
| 9 | The same send from a different Ed25519 device | Sheet again |
| 10 | Global "accept files without asking" on | Any device gets `201` with no sheet; the log says "auto-accept is on" |
| 11 | Two devices prepare at once | The second gets `409 busy`, and no second sheet appears |
| 12 | 13 prepares from one address inside a minute | The last ones get `429 rate_limited`, and no sheet appears for them |
| 13 | A prepare that repeats an open manifest from a different address, anonymously | Not adopted: `409 busy` while the first session is open, then a fresh sheet |
| 14 | A tap that lands after the question timed out | "That offer already closed." No trust is granted: case 8 must still ask |

Cases 7–8 and 14 are the ones that were wrong before: trust used to be granted
by the tap rather than by the answered offer, so a late tap silently trusted a
device nobody had accepted anything from.

## Reading the log

The app writes `files/flit.log` in its data directory:

```
adb shell run-as dev.flit.android cat files/flit.log
```

Which decision was taken is logged with its reason, so a missing sheet is
explained rather than guessed at: the policy line on start and on every change
("consent policy: auto-accept …, N trusted device(s)"), the two policy paths
separately, adoption of an open session, and trust being granted only after an
answer was delivered.
