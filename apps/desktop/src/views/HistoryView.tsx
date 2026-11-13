import { useEffect, useState } from "react";

import { HistoryHead, HistoryRow } from "../components/HistoryRow";
import { bytes, when } from "../lib/format";
import type { HistoryEntry } from "../lib/types";

/** Everything sent from this device, newest first. The job this screen exists
 * for is "send those files there again", so that action lives on the row. */
export function HistoryView({
  history,
  onResend,
}: {
  history: HistoryEntry[];
  onResend: (entry: HistoryEntry) => void;
}) {
  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 60_000);
    return () => window.clearInterval(timer);
  }, []);

  const totalBytes = history.reduce((sum, entry) => sum + entry.bytes, 0);
  const sentFiles = history.reduce((sum, entry) => sum + entry.files, 0);

  return (
    <div className="page">
      <header className="title-row">
        <div>
          <h1>History</h1>
          <p className="title-note">
            {history.length === 0
              ? "Nothing sent from this device yet."
              : `${history.length} sends · ${sentFiles} files · ${bytes(totalBytes)}`}
          </p>
        </div>
      </header>

      <section className="panel" aria-label="Sent files">
        <div className="rows">
          {history.length === 0 ? (
            <p className="lines-empty">
              A send that finishes, fails or is cancelled is recorded here — with
              the peer, the size and the rate, so a slow evening is explainable.
            </p>
          ) : (
            <>
              <HistoryHead />
              {history.map((entry) => (
                <HistoryRow
                  key={`${entry.at}-${entry.fingerprint}`}
                  entry={entry}
                  now={now}
                  onResend={onResend}
                />
              ))}
            </>
          )}
        </div>
      </section>

      <p className="foot-note">
        History is kept on this machine only. Re-sending needs the files to still
        be where they were; a missing one is reported per item.
        {history.length > 0 ? ` Last send ${when(history[0].at, now)}.` : ""}
      </p>
    </div>
  );
}
