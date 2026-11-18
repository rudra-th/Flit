import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { DropIcon } from "./components/Icons";
import { ConsentDialog } from "./components/ConsentDialog";
import { TopBar, type View } from "./components/TopBar";
import { getBridge, watchDrag, type FlitBridge } from "./lib/bridge";
import { hostPart } from "./lib/format";
import type {
  Doctor,
  EngineEvent,
  HistoryEntry,
  ItemOutcome,
  Offer,
  Peer,
  ReceivingPolicy,
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

/** The receiving policy is two calls on the bridge; read them together so a
 * half-read state is never drawn. */
function readPolicy(live: FlitBridge): Promise<ReceivingPolicy> {
  return Promise.all([live.autoAccept(), live.trustedDevices()]).then(
    ([autoAccept, trusted]) => ({ autoAccept, trusted }),
  );
}

export default function App() {
  const [bridge, setBridge] = useState<FlitBridge | null>(null);
  const [status, setStatus] = useState<Status | null>(null);
  const [peers, setPeers] = useState<Record<string, Peer>>({});
  const [history, setHistory] = useState<HistoryEntry[]>([]);
  const [doctor, setDoctor] = useState<Doctor | null>(null);
  const [transfers, setTransfers] = useState<Record<number, Transfer>>({});
  const [tray, setTray] = useState<TrayEntry[]>([]);
  const [selected, setSelected] = useState<string[]>([]);
  const [view, setView] = useState<View>(viewFromHash);
  const [dragging, setDragging] = useState(false);
  const [toast, setToast] = useState<Toast | null>(null);
  /** The question on screen, if a device is waiting for an answer. */
  const [offer, setOffer] = useState<Offer | null>(null);
  /** What this computer does when an offer arrives with nobody deciding. */
  const [policy, setPolicy] = useState<ReceivingPolicy>({
    autoAccept: false,
    trusted: [],
  });

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

  /** Save a new name. The shell restarts to announce it, so the toast is the
   * last thing this window says. */
  const rename = useCallback(
    async (name: string) => {
      const live = bridgeRef.current;
      if (!live) return;
      try {
        const saved = await live.rename(name);
        say(`This device is now ${saved}. Flit is restarting.`, "ok");
      } catch (error) {
        say(describe(error), "bad");
        throw error;
      }
    },
    [say],
  );

  useEffect(() => {
    const onHash = () => setView(viewFromHash());
    window.addEventListener("hashchange", onHash);
    return () => window.removeEventListener("hashchange", onHash);
  }, []);

  const reload = useCallback(async () => {
    const live = bridgeRef.current;
    if (!live) return;
    const [list, sends, checks, receiving] = await Promise.all([
      live.peers(),
      live.history(),
      live.doctor(),
      readPolicy(live),
    ]);
    setPeers(Object.fromEntries(list.map((peer) => [peer.deviceId, peer])));
    setHistory(sends);
    setDoctor(checks);
    setPolicy(receiving);
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

  const addClipboard = useCallback(async () => {
    const live = bridgeRef.current;
    if (!live) return;
    let text: string;
    try {
      text = await live.readClipboard();
    } catch (error) {
      say(describe(error), "bad");
      return;
    }
    // Whitespace decides whether there is anything to send; the payload itself
    // is handed over exactly as it was copied.
    if (text.trim().length === 0) {
      say("Your clipboard is empty.", "bad");
      return;
    }
    if (trayRef.current.some((entry) => entry.text === text)) {
      say("That text is already in the tray.");
      return;
    }
    const size = new TextEncoder().encode(text).length;
    setTray((current) => [
      ...current,
      {
        path: "",
        name: "clipboard.txt",
        isDir: false,
        files: 1,
        bytes: size,
        text,
      },
    ]);
  }, [say]);

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

        case "incomingOffer":
          setOffer({
            id: event.id,
            sender: event.sender,
            fingerprint: event.fingerprint,
            items: event.items,
            bytes: event.bytes,
          });
          return;

        case "offerClosed":
          // Only the question it names: a later one may already be up.
          setOffer((current) => (current?.id === event.id ? null : current));
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
            say(savedNote(event.items));
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

        const [me, list, sends, checks, receiving] = await Promise.all([
          live.status(),
          live.peers(),
          live.history(),
          live.doctor(),
          readPolicy(live),
        ]);
        if (disposed) return;

        setStatus(me);
        setPeers(Object.fromEntries(list.map((peer) => [peer.deviceId, peer])));
        setHistory(sends);
        setDoctor(checks);
        setPolicy(receiving);

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
      const alive = current.filter((id) => peers[id] !== undefined);
      // The same selection must keep its identity, or every announce would
      // re-render the device list for nothing.
      if (alive.length === current.length && alive.length > 0) return current;
      return peerList.length === 1 ? [peerList[0].deviceId] : alive;
    });
  }, [peerList, peers]);

  // Choosing is a toggle, because one send can go to several devices.
  const toggle = useCallback((deviceId: string) => {
    setSelected((current) =>
      current.includes(deviceId)
        ? current.filter((id) => id !== deviceId)
        : [...current, deviceId],
    );
  }, []);

  const send = useCallback(
    async (deviceIds?: string[]) => {
      const live = bridgeRef.current;
      if (!live) return;
      const targets = (deviceIds ?? selectedRef.current).filter(
        (id) => peersRef.current[id] !== undefined,
      );
      const items = trayRef.current;
      if (targets.length === 0) {
        say("Choose a device first.", "bad");
        return;
      }
      if (items.length === 0) {
        say("Add files before sending.", "bad");
        return;
      }

      // Files ride in one session per device. Text is its own item with no
      // bytes to stream, so it gets its own lane instead of joining the pile.
      const paths = items.flatMap((entry) =>
        entry.text === undefined ? [entry.path] : [],
      );
      const texts = items.filter((entry) => entry.text !== undefined);
      const extra = Math.max(0, paths.length - 1);

      // One independent session per device. A device that packed up should not
      // take the others down with it, and every session has its own lane.
      const results = await Promise.allSettled(
        targets.map(async (id) => {
          if (paths.length > 0) {
            const started = await live.sendToPeer(id, paths);
            if (extra > 0) {
              setTransfers((current) => {
                const transfer = current[started];
                if (!transfer) return current;
                return { ...current, [started]: { ...transfer, extra } };
              });
            }
          }
          for (const entry of texts) {
            await live.sendTextToPeer(id, entry.text ?? "", entry.name);
          }
        }),
      );

      const refused = results.flatMap((result, index) =>
        result.status === "rejected"
          ? [
              {
                alias: peersRef.current[targets[index]]?.alias ?? targets[index],
                error: describe(result.reason),
              },
            ]
          : [],
      );

      if (refused.length === 0) {
        setTray([]);
        return;
      }

      // The tray keeps its files when a device refused: the ones that went
      // already landed, and re-sending them would leave copies at the receiver.
      if (refused.length === targets.length) {
        say(
          targets.length === 1
            ? refused[0].error
            : `Nothing left the machine. ${refused[0].alias}: ${refused[0].error}`,
          "bad",
        );
        return;
      }
      say(
        `Sent to ${targets.length - refused.length} of ${targets.length}. ${refused[0].alias}: ${refused[0].error}`,
        "bad",
      );
    },
    [say],
  );

  const resend = useCallback(
    async (entry: HistoryEntry) => {
      const live = bridgeRef.current;
      if (!live) return;
      if (entry.paths.length === 0) {
        say("That send has nothing to send again.", "bad");
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

  /**
   * Answer the question on screen.
   *
   * It comes down first: the sender is blocked until this returns, and a person
   * who has already decided should not sit in front of a dead question while a
   * round trip finishes.
   */
  const answerOffer = useCallback(
    async (id: number, accept: boolean, remember: boolean) => {
      const live = bridgeRef.current;
      setOffer((current) => (current?.id === id ? null : current));
      if (!live) return;
      try {
        const answered = await live.answerOffer(id, accept, remember);
        if (!answered) {
          say("That transfer already closed.", "bad");
        } else if (accept && remember) {
          // The shell wrote the key before answering, but the panel was read
          // separately; refresh it so the new row is there when the question
          // comes down, not a reload later.
          void live
            .trustedDevices()
            .then((trusted) => setPolicy((current) => ({ ...current, trusted })))
            .catch(() => undefined);
          say("Accepted, and this device is trusted from now on.");
        }
      } catch (error) {
        say(describe(error), "bad");
      }
    },
    [say],
  );

  /**
   * Turn "accept every device" on or off.
   *
   * Drawn at once and undone if the shell refuses, because the switch is the
   * only feedback a person gets and waiting on a round trip makes it feel
   * broken.
   */
  const setAutoAccept = useCallback(
    (enabled: boolean) => {
      const live = bridgeRef.current;
      if (!live) return;
      setPolicy((current) => ({ ...current, autoAccept: enabled }));
      void live
        .setAutoAccept(enabled)
        .catch((error) => {
          setPolicy((current) => ({ ...current, autoAccept: !enabled }));
          say(describe(error), "bad");
        });
    },
    [say],
  );

  /** Forget a trusted key. The row goes when the shell says it did. */
  const forgetTrusted = useCallback(
    (fingerprint: string) => {
      const live = bridgeRef.current;
      if (!live) return;
      void (async () => {
        try {
          const forgotten = await live.forgetTrusted(fingerprint);
          if (!forgotten) return;
          setPolicy((current) => ({
            ...current,
            trusted: current.trusted.filter((key) => key !== fingerprint),
          }));
          say("That device will be asked again.");
        } catch (error) {
          say(describe(error), "bad");
        }
      })();
    },
    [say],
  );

  // How the tray names where this is going: one name, or a count.
  const chosen = peerList.filter((peer) => selected.includes(peer.deviceId));
  const targetName =
    chosen.length === 0
      ? null
      : chosen.length === 1
        ? chosen[0].alias
        : `${chosen.length} devices`;

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
            onSelect={toggle}
            onAdd={() => {
              void (async () => {
                const live = bridgeRef.current;
                if (!live) return;
                await addPaths(await live.pickFiles());
              })();
            }}
            onPaste={() => {
              void addClipboard();
            }}
            onRemove={(removed) =>
              setTray((current) =>
                current.filter((entry) =>
                  removed.text === undefined
                    ? entry.path !== removed.path
                    : entry.text !== removed.text,
                ),
              )
            }
            onClear={() => setTray([])}
            onSend={(deviceIds) => {
              void send(deviceIds);
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
          <DeviceView
            status={status}
            policy={policy}
            peers={peerList}
            onRename={rename}
            onAutoAccept={setAutoAccept}
            onForget={forgetTrusted}
          />
        )}
      </main>

      {dragging ? (
        <div className="drop-veil" aria-hidden>
          <div className="drop-veil-inner">
            <DropIcon size={26} />
            <span>
              <strong>Drop to send</strong>
              <br />
              {targetName === null
                ? "Then choose the device it goes to."
                : `To ${targetName}.`}
            </span>
          </div>
        </div>
      ) : null}

      {toast ? (
        <div className="toast" data-tone={toast.tone} role="status">
          {toast.text}
        </div>
      ) : null}

      {offer !== null ? (
        <ConsentDialog
          key={offer.id}
          offer={offer}
          onAnswer={(accept, remember) => {
            void answerOffer(offer.id, accept, remember);
          }}
        />
      ) : null}
    </div>
  );
}

function describe(error: unknown): string {
  if (error instanceof Error) return error.message;
  if (typeof error === "string") return error;
  return "Something went wrong.";
}

/** What to say when everything arrived. Files and pasted text read differently:
 * one is "saved it", the other is "it is on the other side, named". */
function savedNote(items: ItemOutcome[]): string {
  const text = items.length > 0 && items.every((item) => item.path === "");
  if (text) {
    return items.length === 1
      ? `Saved as ${items[0].name} at the receiver.`
      : `${items.length} text items saved at the receiver.`;
  }
  return `${items.length === 1 ? "File" : `${items.length} files`} saved at the receiver.`;
}

/** The screen is part of the URL, so a deep link from the tray and a screenshot
 * of one view are the same mechanism. Anything else lands on Send. */
function viewFromHash(): View {
  const hash = window.location.hash.replace(/^#/, "");
  return hash === "history" || hash === "doctor" || hash === "device"
    ? hash
    : "send";
}
