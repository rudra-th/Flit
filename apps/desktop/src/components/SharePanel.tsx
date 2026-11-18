import type { Share } from "../lib/types";
import { CopyButton } from "./CopyButton";

/** A browser share, and the only thing it needs: an address to read.
 *
 * The address is the capability, so it is set in mono and is never truncated:
 * half a URL read across a room is a support call. The line under it says what
 * the other computer has to do, which is nothing but open a browser. */
export function SharePanel({
  share,
  onStop,
}: {
  share: Share;
  onStop: () => void;
}) {
  const files = share.files === 1 ? "1 file" : `${share.files} files`;
  const minutes = Math.ceil(share.expiresIn / 60);
  const left =
    minutes <= 1 ? "about a minute left" : `about ${minutes} minutes left`;

  return (
    <section className="panel share-panel" aria-label="Browser share">
      <div className="share-head">
        <div>
          <p className="share-kicker">Browser share</p>
          <h2>Read this on the other computer</h2>
        </div>
        <span className="share-meta">
          {files} &middot; {left}
        </span>
      </div>

      <p className="share-url">{share.url}</p>

      <p className="share-note">
        Any browser on this Wi-Fi. Nothing to install. This computer serves the
        files itself, and the address stops answering when the time runs out.
      </p>

      <div className="share-actions">
        <CopyButton text={share.url} label="Copy address" />
        <button type="button" className="btn btn-quiet" onClick={onStop}>
          Stop sharing
        </button>
      </div>
    </section>
  );
}
