/** A browser stand-in for the engine, loaded only by a dev build.
 *
 * It exists so the UI can be worked on and reviewed without a shell, and so
 * every state (empty, in flight, done, failed) can be seen on demand. Nothing
 * here is reachable from a production bundle: `bridge.ts` imports it behind
 * `import.meta.env.DEV`.
 */

import type { FlitBridge } from "./bridge";
import { baseName } from "./format";
import type {
  Check,
  Doctor,
  EngineEvent,
  HistoryEntry,
  ItemOutcome,
  NetInterface,
  Peer,
  Status,
  TrayEntry,
} from "./types";

const THIS_MACHINE: Status = {
  alias: "Desk",
  fingerprint:
    "8d2f5a7c9e1b4d6f8a0c3e5b7d92f4a6c8e0b2d4f6a8c1e3b5d7f9a0c2e4b6d8",
  badge: { emoji: "🦉🌾🎧🪁", words: "owl-meadow-earphone-kite" },
  addr: "192.168.1.18:47821",
  root: "C:\\Users\\rhea\\Flit",
  proto: 1,
  version: "0.1.0",
  discovery: { state: "running" },
};

const PEERS: Peer[] = [
  {
    deviceId: "dev_studio",
    alias: "Studio PC",
    fingerprint:
      "3f9a1c74b8e25d06a4c7f13b9e8d2a5c60b4e17f9d3a8c2b5e6f0a9d4c7b1e83",
    deviceType: "desktop",
    addr: "192.168.1.24:47821",
    source: "multicast",
    badge: { emoji: "🦊🌊🔧🍋", words: "fox-tide-wrench-lemon" },
  },
  {
    deviceId: "dev_pixel",
    alias: "Rhea – Pixel 9",
    fingerprint:
      "b71c4e9f2a6d8305c1e7b4a9d2f6c8e30a5b1d7f4c9e2a6b8d3f0c5a1e7b9d46",
    deviceType: "phone",
    addr: "192.168.1.31:47821",
    source: "multicast",
    badge: { emoji: "🐚🌿🔔🍇", words: "shell-fern-bell-grape" },
  },
  {
    deviceId: "dev_nas",
    alias: "Attic NAS",
    fingerprint:
      "c2e8b4a7d1f95036e2a8c5b7d9f1a4c60e3b8d2f5a7c9e1b4d6f8a0c3e5b7d92",
    deviceType: "headless",
    addr: "192.168.1.7:47821",
    source: "scan",
    badge: { emoji: "🗿🧭📎🕯", words: "stone-compass-clip-candle" },
  },
];

const FILES: Record<string, { bytes: number; files: number; isDir: boolean }> = {
  "C:/Users/rhea/Pictures/Q3 handover": {
    bytes: 186_400_000,
    files: 14,
    isDir: true,
  },
  "C:/Users/rhea/Pictures/Q3 handover/floorplan.pdf": {
    bytes: 2_480_000,
    files: 1,
    isDir: false,
  },
  "C:/Users/rhea/Music/session-raw.wav": {
    bytes: 412_000_000,
    files: 1,
    isDir: false,
  },
  "C:/Users/rhea/Desktop/handover-notes.md": {
    bytes: 8_900,
    files: 1,
    isDir: false,
  },
};

const INTERFACES: NetInterface[] = [
  { name: "Wi-Fi", addr: "192.168.1.18/24", loopback: false, up: true },
  { name: "Ethernet", addr: "10.0.2.15/24", loopback: false, up: true },
  { name: "Loopback", addr: "127.0.0.1/8", loopback: true, up: true },
];

const CHECKS: Check[] = [
  {
    id: "listen",
    name: "Receiving",
    state: "ok",
    detail: "Listening on all interfaces, port 47821.",
    sub: "A sender that knows this device's fingerprint can reach it now.",
    value: "0.0.0.0:47821",
  },
  {
    id: "discovery",
    name: "Multicast discovery",
    state: "ok",
    detail: "Announcing on 239.255.70.70:47844 every 2 s.",
    sub: "3 devices answered within the last minute.",
    value: "running",
  },
  {
    id: "folder",
    name: "Receive folder",
    state: "ok",
    detail: "Writable, 214 GB free.",
    sub: "Files are verified in .flit/parts and only then given their real name.",
    value: "C:\\Users\\rhea\\Flit",
  },
  {
    id: "firewall",
    name: "Firewall",
    state: "unknown",
    detail: "Not checked by this build.",
    sub: "The installer adds the rule. If a peer cannot connect, allow Flit on private networks.",
    value: null,
  },
];

function seedHistory(): HistoryEntry[] {
  const now = Date.now();
  return [
    {
      at: now - 12 * 60_000,
      peer: "Studio PC",
      emoji: "🦊🌊🔧🍋",
      fingerprint: PEERS[0].fingerprint,
      addr: PEERS[0].addr,
      files: 1,
      bytes: 412_000_000,
      ms: 7_400,
      ok: true,
      error: null,
      paths: ["C:/Users/rhea/Music/session-raw.wav"],
    },
    {
      at: now - 26 * 60 * 60_000,
      peer: "Rhea – Pixel 9",
      emoji: "🐚🌿🔔🍇",
      fingerprint: PEERS[1].fingerprint,
      addr: PEERS[1].addr,
      files: 6,
      bytes: 18_200_000,
      ms: 1_100,
      ok: true,
      error: null,
      paths: ["C:/Users/rhea/Pictures/Q3 handover"],
    },
    {
      at: now - 8 * 24 * 60 * 60_000,
      peer: "Attic NAS",
      emoji: "🗿🧭📎🕯",
      fingerprint: PEERS[2].fingerprint,
      addr: PEERS[2].addr,
      files: 1,
      bytes: 2_100_000_000,
      ms: 38_000,
      ok: false,
      error: "the peer stopped answering after 38 s",
      paths: ["C:/Users/rhea/Q3 handover/floorplan.pdf"],
    },
  ];
}

/** Timing of a simulated send: quick enough to watch, slow enough to read. */
const TICK_MS = 220;
const DURATION_MS = 6_800;

/** `?demo=lanes` paints a running, a saved and a failed send on load. Design
 * review needs all three states on one screen without waiting for three
 * transfers; a shipped build never loads this file at all. */
function lanesWanted(): boolean {
  if (typeof location === "undefined") return false;
  return new URLSearchParams(location.search).get("demo") === "lanes";
}

interface Runner {
  timer: number;
  cancelled: boolean;
}

export function createDemo(): FlitBridge {
  const listeners = new Set<(event: EngineEvent) => void>();
  const peers = new Map(PEERS.map((peer) => [peer.deviceId, peer]));
  const history = seedHistory();
  const runners = new Map<number, Runner>();
  let nextId = 1;

  const emit = (event: EngineEvent) => {
    for (const listener of listeners) listener(event);
  };

  const run = (
    id: number,
    peer: Peer,
    label: string,
    paths: string[],
    bytes: number,
    files: number,
    outcomes: ItemOutcome[],
    duration = DURATION_MS,
  ) => {
    const started = Date.now();
    emit({
      kind: "transferStarted",
      id,
      label,
      files,
      bytes,
      peer: peer.addr,
    });

    const runner: Runner = {
      cancelled: false,
      timer: window.setInterval(() => {
        if (runner.cancelled) return;
        const elapsed = Date.now() - started;
        const t = Math.min(1, elapsed / duration);
        // Real transfers slow near the end (small tail items). Easing here is
        // for the reviewer's eye, not a model of the wire.
        const eased = 1 - Math.pow(1 - t, 1.6);
        const sent = Math.round(bytes * eased);
        emit({
          kind: "transferProgress",
          id,
          sent,
          total: bytes,
          filesDone: t >= 1 ? files : Math.floor(files * t),
          filesTotal: files,
        });

        if (t >= 1) {
          window.clearInterval(runner.timer);
          runners.delete(id);
          emit({ kind: "transferFinished", id, items: outcomes });
          history.unshift({
            at: Date.now(),
            peer: peer.alias,
            emoji: peer.badge?.emoji ?? null,
            fingerprint: peer.fingerprint,
            addr: peer.addr,
            files,
            bytes,
            ms: duration,
            ok: true,
            error: null,
            paths,
          });
        }
      }, TICK_MS),
    };
    runners.set(id, runner);
    return id;
  };

  /** The outcomes a file send ends with, one per path. */
  const fileOutcomes = (paths: string[], bytes: number): ItemOutcome[] =>
    paths.map((path) => ({
      path,
      name: baseName(path),
      savedAs: baseName(path),
      bytes: Math.round(bytes / paths.length),
      error: null,
    }));

  const describe = (paths: string[]) =>
    paths.map((path) => {
      const known = FILES[path];
      return {
        path,
        name: baseName(path),
        isDir: known?.isDir ?? false,
        files: known?.files ?? 1,
        bytes: known?.bytes ?? 1_240_000,
      };
    });

  // The three lane states, painted once the UI is listening.
  const seedLanes = () => {
    emit({
      kind: "transferStarted",
      id: 902,
      label: "session-raw.wav",
      files: 1,
      bytes: 412_000_000,
      peer: PEERS[0].addr,
    });
    emit({
      kind: "transferProgress",
      id: 902,
      sent: 188_400_000,
      total: 412_000_000,
      filesDone: 0,
      filesTotal: 1,
    });

    emit({
      kind: "transferStarted",
      id: 903,
      label: "floorplan.pdf",
      files: 1,
      bytes: 2_100_000_000,
      peer: PEERS[2].addr,
    });
    emit({
      kind: "transferProgress",
      id: 903,
      sent: 41_000_000,
      total: 2_100_000_000,
      filesDone: 0,
      filesTotal: 1,
    });
    emit({
      kind: "transferFailed",
      id: 903,
      reason: "the peer stopped answering after 38 s",
    });

    emit({
      kind: "transferStarted",
      id: 904,
      label: "Q3 handover",
      files: 14,
      bytes: 186_400_000,
      peer: PEERS[1].addr,
    });
    window.setTimeout(() => {
      emit({
        kind: "transferProgress",
        id: 904,
        sent: 186_400_000,
        total: 186_400_000,
        filesDone: 14,
        filesTotal: 14,
      });
      emit({
        kind: "transferFinished",
        id: 904,
        items: Array.from({ length: 14 }, (_, index) => ({
          path: `C:/Users/rhea/Pictures/Q3 handover/file-${index + 1}.jpg`,
          name: `Q3 handover/file-${index + 1}.jpg`,
          savedAs: `Q3 handover/file-${index + 1}.jpg`,
          bytes: 13_300_000,
          error: null,
        })),
      });
    }, 1_500);
  };

  let seeded = !lanesWanted();

  return {
    demo: true,

    async status() {
      return THIS_MACHINE;
    },

    async peers() {
      return [...peers.values()];
    },

    async history() {
      return [...history];
    },

    async doctor(): Promise<Doctor> {
      return { checks: CHECKS, interfaces: INTERFACES };
    },

    async inspect(paths: string[]): Promise<TrayEntry[]> {
      return describe(paths);
    },

    async sendToPeer(deviceId, paths) {
      const peer = peers.get(deviceId);
      if (!peer) throw new Error(`no nearby device with id ${deviceId}`);
      const entries = describe(paths);
      const bits = entries.reduce((sum, entry) => sum + entry.bytes, 0);
      const files = entries.reduce((sum, entry) => sum + entry.files, 0);
      const id = nextId++;
      return run(
        id,
        peer,
        baseName(paths[0] ?? ""),
        paths,
        bits,
        files,
        fileOutcomes(paths, bits),
      );
    },

    // Text is one item with no bytes on the wire, so the lane is a blink rather
    // than a progress bar — the same shape a real send takes.
    async sendTextToPeer(deviceId, text, name) {
      const peer = peers.get(deviceId);
      if (!peer) throw new Error(`no nearby device with id ${deviceId}`);
      const bytes = new TextEncoder().encode(text).length;
      return run(
        nextId++,
        peer,
        name,
        [],
        bytes,
        1,
        [{ path: "", name, savedAs: name, bytes, error: null }],
        900,
      );
    },

    // Nothing to ask the system for in a browser: a paste is simulated with the
    // link the demo already uses for copy.
    async readClipboard() {
      return "https://example.com/q3-handover";
    },

    async sendToAddress(addr, fingerprint, paths) {
      const peer = [...peers.values()].find((entry) => entry.addr === addr);
      const target: Peer =
        peer ??
        ({
          deviceId: "manual",
          alias: addr,
          fingerprint,
          deviceType: "desktop",
          addr,
          source: "manual",
          badge: null,
        } satisfies Peer);
      const entries = describe(paths);
      const bits = entries.reduce((sum, entry) => sum + entry.bytes, 0);
      const files = entries.reduce((sum, entry) => sum + entry.files, 0);
      const id = nextId++;
      return run(
        id,
        target,
        baseName(paths[0] ?? ""),
        paths,
        bits,
        files,
        fileOutcomes(paths, bits),
      );
    },

    async cancel(id) {
      const runner = runners.get(id);
      if (!runner) return false;
      runner.cancelled = true;
      window.clearInterval(runner.timer);
      runners.delete(id);
      emit({ kind: "transferCancelled", id });
      return true;
    },

    async pickFiles() {
      return [
        "C:/Users/rhea/Pictures/Q3 handover",
        "C:/Users/rhea/Desktop/handover-notes.md",
      ];
    },

    async subscribe(handler) {
      listeners.add(handler);
      if (!seeded) {
        seeded = true;
        window.setTimeout(seedLanes, 120);
      }
      return () => listeners.delete(handler);
    },
  };
}
