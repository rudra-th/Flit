import { useEffect, useState } from "react";

import { DeviceCard } from "../components/DeviceCard";
import { HistoryHead, HistoryRow } from "../components/HistoryRow";
import { AlertIcon, CopyIcon, PlusIcon } from "../components/Icons";
import { Lane } from "../components/Lane";
import { Tray } from "../components/Tray";
import { rate } from "../lib/format";
import type { HistoryEntry, Peer, Status, Transfer, TrayEntry } from "../lib/types";

/** The screen answers one question first: where can this go, right now?
 *
 * Devices are the hero and the widest thing on the page. What you are sending
 * sits under them at the same width, so the two halves of the decision read as
 * one block. Transfers appear only while there is something to watch, and
 * recent sends only once there is history: an empty panel is not information. */
export function SendView({
  status,
  peers,
  selected,
  tray,
  transfers,
  history,
  onSelect,
  onAdd,
  onPaste,
  onRemove,
  onClear,
  onSend,
  onCancel,
  onResend,
  onOpenHistory,
  onOpenDoctor,
}: {
  status: Status | null;
  peers: Peer[];
  selected: string[];
  tray: TrayEntry[];
  transfers: Transfer[];
  history: HistoryEntry[];
  onSelect: (deviceId: string) => void;
  onAdd: () => void;
  onPaste: () => void;
  onRemove: (entry: TrayEntry) => void;
  onClear: () => void;
  onSend: (deviceIds?: string[]) => void;
  onCancel: (id: number) => void;
  onResend: (entry: HistoryEntry) => void;
  onOpenHistory: () => void;
  onOpenDoctor: () => void;
}) {
  const [now, setNow] = useState(() => Date.now());

  // "Today 14:02" goes stale if the window is left open. One minute is enough
  // for a recency label and costs nothing.
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 60_000);
    return () => window.clearInterval(timer);
  }, []);

  const running = transfers.filter((transfer) => transfer.state === "running");
  const chosen = peers.filter((peer) => selected.includes(peer.deviceId));
  const target =
    chosen.length === 1
      ? chosen[0].alias
      : chosen.length > 1
        ? `${chosen.length} devices`
        : null;
  const only = peers.length === 1 ? peers[0] : null;
  const discovery = status?.discovery;

  const finding =
    discovery?.state === "unavailable" ? (
      <>
        Multicast is blocked here, so devices are found by a scan.{" "}
        <strong>Receiving is not affected.</strong>
      </>
    ) : discovery?.state === "off" ? (
      <>Discovery is off. Send to a device by its address.</>
    ) : peers.length === 0 ? (
      <>
        No device has answered yet.{" "}
        <strong>Check the other device is running Flit.</strong>
      </>
    ) : (
      <>
        <strong>{peers.length === 1 ? "1 device" : `${peers.length} devices`}</strong>{" "}
        {discovery?.state === "running"
          ? "on the local network"
          : "found by a scan"}
      </>
    );

  return (
    <div className="page">
      <header className="title-row">
        <div>
          <h1>Send</h1>
          <p className="title-note">{finding}</p>
        </div>
        <div className="title-actions">
          <button
            type="button"
            className="btn btn-quiet"
            onClick={onPaste}
            title="Send whatever is on your clipboard as text"
          >
            <CopyIcon size={13} />
            Paste
          </button>
          <button type="button" className="btn" onClick={onAdd}>
            <PlusIcon size={14} />
            Add files
          </button>
        </div>
      </header>

      <section className="panel" aria-label="Devices">
        <div className="section-label">
          Nearby
          <span className="count">{peers.length}</span>
          <span className="right">
            {peers.length > 0 ? "choose one or more, then send" : ""}
          </span>
        </div>

        {peers.length === 0 ? (
          <div className="devices-empty">
            <AlertIcon size={18} />
            <span>
              <strong>Nothing nearby.</strong> Both devices must be on the same
              network.{" "}
              <button type="button" className="link" onClick={onOpenDoctor}>
                Check the network
              </button>
            </span>
          </div>
        ) : (
          <div className={only === null ? "devices" : "devices single"}>
            {peers.map((peer) => (
              <DeviceCard
                key={peer.deviceId}
                peer={peer}
                selected={selected.includes(peer.deviceId)}
                lone={only !== null}
                onSelect={() => onSelect(peer.deviceId)}
                onSend={() => onSend([peer.deviceId])}
              />
            ))}
          </div>
        )}
      </section>

      <div className="section-gap">
        <Tray
          entries={tray}
          target={target}
          canSend={chosen.length > 0}
          onAdd={onAdd}
          onRemove={onRemove}
          onClear={onClear}
          onSend={() => onSend()}
        />
      </div>

      {transfers.length > 0 ? (
        <section className="panel section-gap" aria-label="Transfers">
          <div className="panel-head">
            <span className="panel-title">Transfers</span>
            <span className="panel-meta">
              {running.length > 0
                ? `${running.length} running${
                    running.length > 1
                      ? ` · ${rate(running.reduce((sum, item) => sum + item.rate, 0))}`
                      : ""
                  }`
                : `${transfers.length} this session`}
            </span>
          </div>
          <div className="lanes">
            {[...transfers]
              .sort((a, b) => b.startedAt - a.startedAt)
              .map((transfer) => (
                <Lane
                  key={transfer.id}
                  transfer={transfer}
                  onCancel={() => onCancel(transfer.id)}
                />
              ))}
          </div>
        </section>
      ) : null}

      {history.length > 0 ? (
        <section className="panel section-gap" aria-label="Recent sends">
          <div className="panel-head">
            <span className="panel-title">Recent</span>
            <button
              type="button"
              className="btn btn-quiet panel-link"
              onClick={onOpenHistory}
            >
              All history
            </button>
          </div>
          <div className="rows">
            <HistoryHead />
            {history.slice(0, 3).map((entry) => (
              <HistoryRow
                key={`${entry.at}-${entry.fingerprint}`}
                entry={entry}
                now={now}
                onResend={onResend}
              />
            ))}
          </div>
        </section>
      ) : null}
    </div>
  );
}