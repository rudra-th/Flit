/** The one door between the UI and the engine.
 *
 * In the shell every call is a Tauri command; in a plain browser (design
 * review, `npm run dev`) a demo engine stands in, and only when the build is
 * a dev build. A production bundle has no demo code in it at all.
 */

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open } from "@tauri-apps/plugin-dialog";

import type {
  Doctor,
  EngineEvent,
  HistoryEntry,
  Peer,
  Status,
  TrayEntry,
} from "./types";

export interface FlitBridge {
  /** True when this is the browser stand-in, never in a shipped build. */
  readonly demo: boolean;
  status(): Promise<Status>;
  peers(): Promise<Peer[]>;
  history(): Promise<HistoryEntry[]>;
  doctor(): Promise<Doctor>;
  /** Expand dropped paths into what the tray shows. */
  inspect(paths: string[]): Promise<TrayEntry[]>;
  sendToPeer(deviceId: string, paths: string[]): Promise<number>;
  /** Offer text — a link, a note — as one item the receiver saves as `name`. */
  sendTextToPeer(deviceId: string, text: string, name: string): Promise<number>;
  sendToAddress(addr: string, fingerprint: string, paths: string[]): Promise<number>;
  /** Whatever is on the system clipboard, as text. Empty means nothing there. */
  readClipboard(): Promise<string>;
  cancel(id: number): Promise<boolean>;
  pickFiles(): Promise<string[]>;
  subscribe(handler: (event: EngineEvent) => void): Promise<() => void>;
}

function inShell(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

/** True when this UI is running inside the desktop shell, false in a browser
 * (design review, `npm run dev`). Window chrome and the picker only exist in
 * the shell. */
export function runningInShell(): boolean {
  return inShell();
}

const shell: FlitBridge = {
  demo: false,
  status: () => invoke<Status>("status"),
  peers: () => invoke<Peer[]>("peers"),
  history: () => invoke<HistoryEntry[]>("history"),
  doctor: () => invoke<Doctor>("doctor"),
  inspect: (paths) => invoke<TrayEntry[]>("inspect_paths", { paths }),
  sendToPeer: (deviceId, paths) =>
    invoke<number>("send_to_peer", { deviceId, paths }),
  sendTextToPeer: (deviceId, text, name) =>
    invoke<number>("send_text_to_peer", { deviceId, text, name }),
  sendToAddress: (addr, fingerprint, paths) =>
    invoke<number>("send_to_address", { addr, fingerprint, paths }),
  readClipboard: () => invoke<string>("read_clipboard"),
  cancel: (id) => invoke<boolean>("cancel_transfer", { id }),
  async pickFiles() {
    const picked = await open({
      multiple: true,
      title: "Send with Flit",
    });
    if (picked === null) return [];
    return Array.isArray(picked) ? picked : [picked];
  },
  async subscribe(handler) {
    return listen<EngineEvent>("flit://event", (event) => handler(event.payload));
  },
};

let cached: Promise<FlitBridge> | null = null;

/** The bridge for this build, resolved once. */
export function getBridge(): Promise<FlitBridge> {
  if (cached === null) {
    cached = (async () => {
      if (inShell()) return shell;
      if (import.meta.env.DEV) {
        const { createDemo } = await import("./demo");
        return createDemo();
      }
      throw new Error(
        "Flit's engine is not reachable: this page must run inside the desktop shell.",
      );
    })();
  }
  return cached;
}

/** Drag state for the drop veil. The shell reports it; a browser has no such
 * event, so the demo path is a no-op. */
export async function watchDrag(
  handler: (event: { type: "over" | "drop" | "leave"; paths?: string[] }) => void,
): Promise<() => void> {
  if (!inShell()) return () => {};
  return getCurrentWindow().onDragDropEvent((event) => {
    const payload = event.payload;
    if (payload.type === "drop") {
      handler({ type: "drop", paths: payload.paths });
    } else if (payload.type === "over" || payload.type === "enter") {
      handler({ type: "over" });
    } else {
      handler({ type: "leave" });
    }
  });
}
