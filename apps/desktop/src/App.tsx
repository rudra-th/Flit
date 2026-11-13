import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { DropIcon } from "./components/Icons";
import { TopBar, type View } from "./components/TopBar";
import { getBridge, watchDrag, type FlitBridge } from "./lib/bridge";
import { hostPart } from "./lib/format";
import type {
  Doctor,
  EngineEvent,
  HistoryEntry,
  Peer,
  Status,
  Transfer,
  TrayEntry,
} from "./lib/types";
import { DeviceView } from "./views/DeviceView";
import { DoctorView } from "./views/DoctorView";
import { HistoryView } from "./views/HistoryView";
import { SendView } from "./views/SendView";

interface Sample {
  sent: number;
  at: number;
  rate: number;
}

interface Toast {
  text: string;
  tone: "ok" | "bad";
}

export default function App() {
  const [bridge, setBridge] = useState<FlitBridge | null>(null);
  const [status, setStatus] = useState<Status | null>(null);
  const [peers, setPeers] = useState<Record<string, Peer>>({});
  const [history, setHistory] = useState<HistoryEntry[]>([]);
  const [doctor, setDoctor] = useState<Doctor | null>(null);
  const [transfers, setTransfers] = useState<Record<number, Transfer>>({});
  const [tray, setTray] = useState<TrayEntry[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [view, setView] = useState<View>(viewFromHash);
  const [dragging, setDragging] = useState(false);
  const [toast, setToast] = useState<Toast | null>(null);

  // Event handlers run outside React's render, so they read live values from
  // refs instead of from a closure captured at subscribe time.
  const peersRef = useRef(peers);
  peersRef.current = peers;
  const selectedRef = useRef(selected);
  selectedRef.current = selected;
  const trayRef = useRef(tray);
  trayRef.current = tray;
  const bridgeRef = useRef<FlitBridge | null>(null);
  bridgeRef.current = bridge;
  const samples = useRef(new Map<number, Sample>());
  const toastTimer = useRef<number | undefined>(undefined);

  const say = useCallback((text: string, tone: Toast["tone"] = "ok") => {
    setToast({ text, tone });
    window.clearTimeout(toastTimer.current);
    toastTimer.current = window.setTimeout(() => setToast(null), 4600);
  }, []);

  // The view lives in the hash: the tray can open straight onto History, and
  // a screenshot of one screen is a URL instead of a clicked path.
  const go = useCallback((next: View) => {
    setView(next);
    const hash = `#${next}`;
    if (window.location.hash !== hash) window.location.hash = hash;
  }, []);

  useEffect(() => {
    const onHash = () => setView(viewFromHash());
    window.addEventListener("hashchange", onHash);
    return () => window.removeEventListener("hashchange", onHash);
  }, []);

  const reload = useCallback(async () => {
    const live = bridgeRef.current;
    if (!live) return;
    const [list, sends, checks] = await Promise.all([
      live.peers(),
      live.history(),
      live.doctor(),
    ]);
    setPeers(Object.fromEntries(list.map((peer) => [peer.deviceId, peer])));
    setHistory(sends);
    setDoctor(checks);
  }, []);

  const addPaths = useCallback(
    async (paths: string[]) => {
      const live = bridgeRef.current;
      if (!live || paths.length === 0) return;
      try {
        const entries = await live.inspect(paths);
        setTray((current) => {
          const known = new Set(current.map((entry) => entry.path));
          const added = entries.filter((entry) => !known.has(entry.path));
          return [...current, ...added];
        });
      } catch (error) {
        say(
          `Could not read those paths: ${describe(error)}`,
          "bad",
        );
      }
    },
    [say],
  );

  const handleEvent = useCallback(
    (event: EngineEvent) => {
      switch (event.kind) {
        case "peerSeen":
          setPeers((current) => ({
            ...current,
            [event.peer.deviceId]: event.peer,
          }));
          return;

        case "peerLost":
          setPeers((current) => {
            const next = { ...current };
            delete next[event.deviceId];
            return next;
          });
          return;

        case "discoveryUnavailable":
          setStatus((current) =>
            current === null
              ? current
              : {
                  ...current,
                  discovery: { state: "unavailable", reason: event.reason },
                },
          );
          say(
            "Multicast is blocked on this network, so devices are found by a scan. Receiving still works.",
            "bad",
          );
          return;

        case "transferStarted": {
          const alias =
            Object.values(peersRef.current).find(
              (peer) => peer.addr === event.peer,
            )?.alias ?? hostPart(event.peer);
          samples.current.set(event.id, {
            sent: 0,
            at: Date.now(),
            rate: 0,
          });
          setTransfers((current) => ({
            ...current,
            [event.id]: {
              id: event.id,
              peer: event.peer,
              peerAlias: alias,
              label: event.label,
              extra: 0,
              files: event.files,
              bytes: event.bytes,
              sent: 0,
              total: event.bytes,
              filesDone: 0,
              state: "running",
              reason: null,
              startedAt: Date.now(),
              endedAt: null,
              rate: 0,
            },
          }));
          return;
        }

        case "transferProgress": {
          const now = Date.now();
          const previous = samples.current.get(event.id) ?? {
            sent: 0,
            at: now,
            rate: 0,
          };
          // A floor of half a second: the first sample can arrive a few
          // milliseconds after the start event, and dividing by that would
          // print a rate no network has ever reached.
          const seconds = Math.max(500, now - previous.at) / 1000;
          const delta = Math.max(0, event.sent - previous.sent);
          // Smoothed, because a per-tick rate jitters by an order of magnitude
          // and a number that jumps reads as a broken counter.
          const measured = delta / seconds;
          const rate =
            previous.rate === 0
              ? measured
              : previous.rate * 0.65 + measured * 0.35;
          samples.current.set(event.id, {
            sent: event.sent,
            at: now,
            rate,
          });
          setTransfers((current) => {
            const transfer = current[event.id];
            if (!transfer) return current;
            return {
              ...current,
              [event.id]: {
                ...transfer,
                sent: event.sent,
                total: event.total,
                filesDone: event.filesDone,
                rate,
              },
            };
          });
          return;
        }

        case "transferFinished": {
          const refused = event.items.filter((item) => item.error !== null);
          setTransfers((current) => {
            const transfer = current[event.id];
            if (!transfer) return current;
            return {
              ...current,
              [event.id]: {
                ...transfer,
                state: "done",
                sent: transfer.total,
                filesDone: transfer.files,
                endedAt: Date.now(),
              },
            };
          });
          samples.current.delete(event.id);
          if (refused.length === 0) {
            say(
              `${event.items.length === 1 ? "File" : `${event.items.length} files`} saved at the receiver.`,
            );
          } else if (refused.length === event.items.length) {
            say(
              `Nothing was accepted. ${refused[0].error ?? "The receiver refused every item."}`,
              "bad",
            );
          } else {
            say(
              `${refused.length} of ${event.items.length} files were refused: ${
                refused[0].error ?? "the receiver said no"
              }`,
              "bad",
            );
          }
          void reload();
          return;
        }

        case "transferFailed":
          setTransfers((current) => {
            const transfer = current[event.id];
            if (!transfer) return current;
            return {
              ...current,
              [event.id]: {
                ...transfer,
                state: "failed",
                reason: event.reason,
                endedAt: Date.now(),
              },
            };
          });
          samples.current.delete(event.id);
          say(event.reason, "bad");
          void reload();
          return;

        case "transferCancelled":
          setTransfers((current) => {
            const transfer = current[event.id];
            if (!transfer) return current;
            return {
              ...current,
              [event.id]: {
                ...transfer,
                state: "cancelled",
                endedAt: Date.now(),
              },
            };
          });
          samples.current.delete(event.id);
          say("Send cancelled. The receiver dropped its partial bytes.");
          void reload();
          return;
      }
    },
    [reload, say],
  );

  useEffect(() => {
    let disposed = false;
    const cleanups: Array<() => void> = [];

    void (async () => {
      try {
        const live = await getBridge();
        if (disposed) return;
        setBridge(live);

        const [me, list, sends, checks] = await Promise.all([
          live.status(),
          live.peers(),
          live.history(),
          live.doctor(),
        ]);
        if (disposed) return;

        setStatus(me);
        setPeers(Object.fromEntries(list.map((peer) => [peer.deviceId, peer])));
        setHistory(sends);
        setDoctor(checks);

        cleanups.push(await live.subscribe(handleEvent));
        cleanups.push(
          await watchDrag((event) => {
            if (event.type === "drop") {
              setDragging(false);
              if (event.paths && event.paths.length > 0) {
                void addPaths(event.paths);
              }
              return;
            }
            setDragging(event.type === "over");
          }),
        );
      } catch (error) {
        say(describe(error), "bad");
      }
    })();

    return () => {
      disposed = true;
      for (const off of cleanups) off();
    };
  }, [addPaths, handleEvent, say]);

  const peerList = useMemo(
    () =>
      Object.values(peers).sort((left, right) =>
        left.alias.localeCompare(right.alias),
      ),
    [peers],
  );

  // Progressive disclosure: with one device there is nothing to choose, so
  // choosing it is not a step the user should have to take.
  useEffect(() => {
    setSelected((current) => {
      if (current !== null && peers[current]) return current;
      return peerList.length === 1 ? peerList[0].deviceId : null;
    });
  }, [peerList, peers]);

  const send = useCallback(
    async (deviceId?: string) => {
      const live = bridgeRef.current;
      if (!live) return;
      const targetId = deviceId ?? selectedRef.current;
      const peer = targetId === null ? undefined : peersRef.current[targetId];
      const items = trayRef.current;
      if (!peer) {
        say("Choose a device first.", "bad");
        return;
      }
      if (items.length === 0) {
        say("Add files before sending.", "bad");
        return;
      }

      const paths = items.map((entry) => entry.path);
      const extra = paths.length - 1;

      try {
        const id = await live.sendToPeer(peer.deviceId, paths);
        if (extra > 0) {
          setTransfers((current) => {
            const transfer = current[id];
            if (!transfer) return current;
            return { ...current, [id]: { ...transfer, extra } };
          });
        }
        setTray([]);
      } catch (error) {
        say(describe(error), "bad");
      }
    },
    [say],
  );

  const resend = useCallback(
    async (entry: HistoryEntry) => {
      const live = bridgeRef.current;
      if (!live) return;
      if (entry.paths.length === 0) {
        say("That entry no longer has its file paths.", "bad");
        return;
      }
      try {
        await live.sendToAddress(entry.addr, entry.fingerprint, entry.paths);
        go("send");
      } catch (error) {
        say(describe(error), "bad");
      }
    },
    [say],
  );

  const refreshDoctor = useCallback(() => {
    void (async () => {
      const live = bridgeRef.current;
      if (!live) return;
      setDoctor(null);
      setDoctor(await live.doctor());
    })();
  }, []);

  const targetAlias =
    selected !== null ? (peers[selected]?.alias ?? null) : null;

  return (
    <div className="app">
      <TopBar view={view} status={status} onChange={go} />

      <main className="content">
        {view === "send" ? (
          <SendView
            status={status}
            peers={peerList}
            selected={selected}
            tray={tray}
            transfers={Object.values(transfers)}
            history={history}
            onSelect={setSelected}
            onAdd={() => {
              void (async () => {
                const live = bridgeRef.current;
                if (!live) return;
                await addPaths(await live.pickFiles());
              })();
            }}
            onRemove={(path) =>
              setTray((current) => current.filter((entry) => entry.path !== path))
            }
            onClear={() => setTray([])}
            onSend={(deviceId) => {
              void send(deviceId);
            }}
            onCancel={(id) => {
              void bridgeRef.current?.cancel(id);
            }}
            onResend={(entry) => {
              void resend(entry);
            }}
            onOpenHistory={() => go("history")}
            onOpenDoctor={() => go("doctor")}
          />
        ) : view === "history" ? (
          <HistoryView
            history={history}
            onResend={(entry) => {
              void resend(entry);
            }}
          />
        ) : view === "doctor" ? (
          <DoctorView doctor={doctor} onRefresh={refreshDoctor} />
        ) : (
          <DeviceView status={status} />
        )}
      </main>

      {dragging ? (
        <div className="drop-veil" aria-hidden>
          <div className="drop-veil-inner">
            <DropIcon size={26} />
            <span>
              <strong>Drop to send</strong>
              <br />
              {targetAlias === null
                ? "Then choose the device it goes to."
                : `To ${targetAlias}.`}
            </span>
          </div>
        </div>
      ) : null}

      {toast ? (
        <div className="toast" data-tone={toast.tone} role="status">
          {toast.text}
        </div>
      ) : null}
    </div>
  );
}

function describe(error: unknown): string {
  if (error instanceof Error) return error.message;
  if (typeof error === "string") return error;
  return "Something went wrong.";
}

/** The screen is part of the URL, so a deep link from the tray and a screenshot
 * of one view are the same mechanism. Anything else lands on Send. */
function viewFromHash(): View {
  const hash = window.location.hash.replace(/^#/, "");
  return hash === "history" || hash === "doctor" || hash === "device"
    ? hash
    : "send";
}
