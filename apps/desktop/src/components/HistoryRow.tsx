import { bytes, duration, rate, when } from "../lib/format";
import type { HistoryEntry } from "../lib/types";
import { AlertIcon, CheckIcon } from "./Icons";

/** One past send. Numbers are right-aligned in mono, the outcome is a word
 * and a state colour, and the reason (when there is one) sits under the
 * device name rather than in a tooltip nobody opens. */
export function HistoryRow({
  entry,
  now,
  onResend,
}: {
  entry: HistoryEntry;
  /** Shared clock, so every row agrees on what "Today" means. */
  now?: number;
  onResend: (entry: HistoryEntry) => void;
}) {
  const speed = entry.ms > 0 ? entry.bytes / (entry.ms / 1000) : 0;

  return (
    <div className="row">
      <span className="when">{when(entry.at, now)}</span>

      <span className="who">
        <span className="badge badge-sm" aria-hidden>
          {entry.emoji ?? ""}
        </span>
        <span className="who-text">
          <span className="name">{entry.peer}</span>
          {entry.ok ? null : <span className="row-error">{entry.error}</span>}
        </span>
      </span>

      <span className="num">{entry.files}</span>

      <span className="cell-stack">
        <span className="num">{bytes(entry.bytes)}</span>
        <span className="sub">{duration(entry.ms)}</span>
      </span>

      <span className="num">{rate(speed)}</span>

      <span className="outcome" data-ok={entry.ok}>
        {entry.ok ? <CheckIcon size={12} /> : <AlertIcon size={12} />}
        {entry.ok ? "Saved" : "Not sent"}
      </span>

      <span className="row-actions">
        {entry.paths.length > 0 ? (
          <button
            type="button"
            className="btn btn-quiet"
            onClick={() => onResend(entry)}
            title={`Send the same items to ${entry.peer} again`}
          >
            Send again
          </button>
        ) : null}
      </span>
    </div>
  );
}

export function HistoryHead() {
  return (
    <div className="row row-head">
      <span>When</span>
      <span>Device</span>
      <span className="num">Files</span>
      <span className="num">Size</span>
      <span className="num">Rate</span>
      <span>Result</span>
      <span />
    </div>
  );
}
