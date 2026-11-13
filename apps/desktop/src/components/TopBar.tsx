import { getCurrentWindow } from "@tauri-apps/api/window";

import { runningInShell } from "../lib/bridge";
import type { Status } from "../lib/types";
import {
  CloseIcon,
  DeviceIcon,
  HistoryIcon,
  Mark,
  MinusIcon,
  PulseIcon,
  SendIcon,
  WindowIcon,
} from "./Icons";

export type View = "send" | "history" | "doctor" | "device";

const NAV = [
  { id: "send" as const, label: "Send", icon: SendIcon },
  { id: "history" as const, label: "History", icon: HistoryIcon },
  { id: "doctor" as const, label: "Network", icon: PulseIcon },
];

/** The window's one piece of chrome: who you are on the left, where you are in
 * the middle, and this device on the right.
 *
 * It is a header, not a sidebar, because the devices are the picture and a
 * rail would crop it. The bar is also the window's drag handle, and in the
 * shell it carries the three window buttons, because the window has no frame
 * of its own. */
export function TopBar({
  view,
  status,
  onChange,
}: {
  view: View;
  status: Status | null;
  onChange: (view: View) => void;
}) {
  const shell = runningInShell();

  return (
    <header className="topbar" data-tauri-drag-region>
      <div className="brand" title="Flit">
        <Mark size={20} />
        <span>Flit</span>
      </div>

      <nav className="tabs" aria-label="Flit">
        {NAV.map((item) => {
          const Icon = item.icon;
          return (
            <button
              key={item.id}
              type="button"
              className="tab"
              aria-current={view === item.id ? "page" : undefined}
              onClick={() => onChange(item.id)}
            >
              <Icon size={15} />
              {item.label}
            </button>
          );
        })}
      </nav>

      <button
        type="button"
        className="me"
        aria-current={view === "device" ? "page" : undefined}
        title={
          status?.badge?.words
            ? `This device · ${status.badge.words}`
            : "This device"
        }
        onClick={() => onChange("device")}
      >
        {status?.badge?.emoji ? (
          <span className="badge badge-sm" aria-hidden>
            {status.badge.emoji}
          </span>
        ) : (
          <DeviceIcon size={15} />
        )}
        <span className="who">
          <span className="name">{status?.alias ?? "This device"}</span>
          <span className="sub">{status === null ? "starting" : "this device"}</span>
        </span>
      </button>

      {shell ? <WindowControls /> : null}
    </header>
  );
}

/** Minimise, maximise and close for a window without a frame.
 *
 * Close hides to the tray instead of quitting: the receiver has to keep
 * listening, or the device stops being reachable the moment the window is
 * dismissed. Quitting is an explicit choice in the tray menu. */
function WindowControls() {
  const win = getCurrentWindow();

  return (
    <div className="win-controls">
      <button
        type="button"
        className="win"
        aria-label="Minimise"
        title="Minimise"
        onClick={() => {
          void win.minimize();
        }}
      >
        <MinusIcon size={14} />
      </button>
      <button
        type="button"
        className="win"
        aria-label="Maximise"
        title="Maximise"
        onClick={() => {
          void win.toggleMaximize();
        }}
      >
        <WindowIcon size={12} />
      </button>
      <button
        type="button"
        className="win win-close"
        aria-label="Close to the tray"
        title="Close to the tray"
        onClick={() => {
          void win.close();
        }}
      >
        <CloseIcon size={14} />
      </button>
    </div>
  );
}