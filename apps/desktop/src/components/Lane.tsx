import { bytes, duration, percent, rate } from "../lib/format";
import type { Transfer } from "../lib/types";
import { CloseIcon } from "./Icons";

/** One send, drawn as a lane: a track from here to there with a head that
 * moves along it. The bar is the truth about bytes; the head is only where
 * they have reached. */
export function Lane({
  transfer,
  onCancel,
}: {
  transfer: Transfer;
  onCancel: () => void;
}) {
  const running = transfer.state === "running";
  const done = percent(transfer.sent, transfer.total);
  const live = running
    ? transfer.rate
    : transfer.endedAt
      ? // A finished lane reports what actually crossed the wire, not the size
        // of the batch: a send that stopped early did not move the rest.
        transfer.sent /
        Math.max(1, (transfer.endedAt - transfer.startedAt) / 1000)
      : 0;

  const summary = running
    ? `${transfer.filesDone} of ${transfer.files} ${transfer.files === 1 ? "file" : "files"} · ${bytes(transfer.sent)} of ${bytes(transfer.total)}`
    : transfer.state === "done"
      ? `Saved · ${duration((transfer.endedAt ?? Date.now()) - transfer.startedAt)}`
      : transfer.state === "cancelled"
        ? `Cancelled after ${bytes(transfer.sent)}`
        : `Failed · ${transfer.reason ?? "the peer stopped answering"}`;

  return (
    <div className="lane" data-state={transfer.state}>
      <div className="lane-name">
        <span className="file">{transfer.label}</span>
        {transfer.extra > 0 ? (
          <span className="rest">+{transfer.extra} more</span>
        ) : null}
      </div>

      <div className="lane-rate">{rate(live)}</div>
      <div className="lane-pct">{done}%</div>

      {running ? (
        <button
          type="button"
          className="btn btn-icon lane-cancel"
          onClick={onCancel}
          aria-label={`Cancel sending ${transfer.label}`}
          title="Cancel"
        >
          <CloseIcon size={13} />
        </button>
      ) : (
        <span className="lane-cancel" aria-hidden />
      )}

      <div className="lane-track" role="presentation">
        <div className="lane-fill" style={{ width: `${done}%` }} />
        {running ? <div className="lane-head" style={{ left: `${done}%` }} /> : null}
      </div>

      <div className="lane-sub">
        <span className="peer">{transfer.peerAlias}</span>
        <span className="sep">·</span>
        <span>{summary}</span>
      </div>
    </div>
  );
}
