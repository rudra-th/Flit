/** The wire shape between the Rust shell and this UI. Mirrors the DTOs in
 * `src-tauri/src/dto.rs`; a field renamed on one side must be renamed on both,
 * and TypeScript will say so here first. */

export type DeviceType = "desktop" | "laptop" | "phone" | "tablet" | "headless";
export type PeerSource = "multicast" | "scan" | "manual";

export interface Peer {
  deviceId: string;
  alias: string;
  fingerprint: string;
  deviceType: DeviceType;
  addr: string;
  source: PeerSource;
}

export type DiscoveryState =
  | { state: "running" }
  | { state: "off" }
  | { state: "unavailable"; reason: string };

export interface Status {
  alias: string;
  fingerprint: string;
  addr: string;
  root: string;
  proto: number;
  version: string;
  discovery: DiscoveryState;
}

/** What this computer does when another device asks to send something.
 *
 * Off is the default, and trust is always a fingerprint: a name is a claim, a
 * key is proof. Mirrors `Policy` in `src-tauri/src/state.rs`. */
export interface ReceivingPolicy {
  autoAccept: boolean;
  trusted: string[];
}

export interface ItemOutcome {
  path: string;
  name: string;
  savedAs: string | null;
  bytes: number;
  error: string | null;
}

/** One thing a device wants to send, as the question about it shows it. */
export interface Offered {
  name: string;
  bytes: number;
  /** True when it lands as text handed over in the manifest, not a file. */
  text: boolean;
}

/** A device asking to send something, and waiting for an answer. */
export interface Offer {
  id: number;
  /** The name it claims. Never proof of anything. */
  sender: string;
  /** The key it proved, or empty when it proved none. */
  fingerprint: string;
  items: Offered[];
  bytes: number;
}

export interface HistoryEntry {
  at: number;
  peer: string;
  fingerprint: string;
  addr: string;
  files: number;
  bytes: number;
  ms: number;
  ok: boolean;
  error: string | null;
  paths: string[];
}

export interface Check {
  id: string;
  name: string;
  /** `wait` is a check still running; `unknown` is one this build cannot
   * read. They are different answers and must not share a colour. */
  state: "ok" | "bad" | "wait" | "unknown";
  detail: string;
  sub: string | null;
  value: string | null;
}

export interface NetInterface {
  name: string;
  addr: string;
  loopback: boolean;
  up: boolean;
}

export interface Doctor {
  checks: Check[];
  interfaces: NetInterface[];
}

/** A browser share that is open: one address, and how long it answers. */
export interface Share {
  /** The address to read out; it names the port that answered. */
  url: string;
  /** How many files the page lists. */
  files: number;
  /** Seconds left before the share closes on its own. */
  expiresIn: number;
}

export interface TrayEntry {
  path: string;
  name: string;
  isDir: boolean;
  files: number;
  bytes: number;
  /** Set when this row is text pasted from the clipboard rather than a path:
   * the payload the receiver saves as `name`. Absent for files and folders. */
  text?: string;
}

export type EngineEvent =
  | { kind: "peerSeen"; peer: Peer }
  | { kind: "peerLost"; deviceId: string }
  | { kind: "discoveryUnavailable"; reason: string }
  | {
      kind: "incomingOffer";
      id: number;
      sender: string;
      fingerprint: string;
      items: Offered[];
      bytes: number;
    }
  | { kind: "offerClosed"; id: number; outcome: "accepted" | "declined" | "timedOut" }
  | { kind: "transferStarted"; id: number; label: string; files: number; bytes: number; peer: string }
  | {
      kind: "transferProgress";
      id: number;
      sent: number;
      total: number;
      filesDone: number;
      filesTotal: number;
    }
  | { kind: "transferFinished"; id: number; items: ItemOutcome[] }
  | { kind: "transferFailed"; id: number; reason: string }
  | { kind: "transferCancelled"; id: number };

export interface Transfer {
  id: number;
  peer: string;
  peerAlias: string;
  /** First file or folder name, so the row says what is moving. */
  label: string;
  /** How many more items ride along with it. */
  extra: number;
  files: number;
  bytes: number;
  sent: number;
  total: number;
  filesDone: number;
  state: "running" | "done" | "failed" | "cancelled";
  reason: string | null;
  startedAt: number;
  endedAt: number | null;
  /** Bytes per second, smoothed. Zero until two samples exist. */
  rate: number;
}
