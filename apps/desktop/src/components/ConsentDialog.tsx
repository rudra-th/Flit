import { useEffect, useState } from "react";

import { bytes, shortFingerprint } from "../lib/format";
import type { Offer } from "../lib/types";

/** How many files the question names before it stops counting them out. */
const LISTED = 4;

/**
 * The question, when a device wants to put files on this computer.
 *
 * It exists because receiving has no other moment of consent: with the sheet
 * closed there is no way to take back a file that already landed. So it says
 * who is asking, what would arrive, and how big it is, and it names the
 * fingerprint rather than only the name, because the name is a claim.
 */
export function ConsentDialog({
  offer,
  folder,
  onAnswer,
}: {
  offer: Offer;
  /** Where the files will land, so the question names the place as well as
   * the sender. Null while the shell is still reporting its status. */
  folder: string | null;
  onAnswer: (accept: boolean, remember: boolean) => void;
}) {
  const [remember, setRemember] = useState(false);

  // Trust needs a key. A sender that proved nothing can still be accepted, but
  // the checkbox is not offered, because there would be nothing to remember it
  // under that a later device could not also claim.
  const canRemember = offer.fingerprint !== "";
  const listed = offer.items.slice(0, LISTED);
  const extra = offer.items.length - listed.length;
  const files =
    offer.items.length === 1 ? "1 file" : `${offer.items.length} files`;
  const accepting = () => onAnswer(true, canRemember && remember);

  // Escape is the only keyboard path. Accepting a transfer is a decision about
  // what lands on this disk, so it does not get a key that is easy to hit by
  // accident while typing somewhere else.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") onAnswer(false, false);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onAnswer]);

  return (
    <div
      className="consent-veil"
      role="presentation"
      onMouseDown={(event) => {
        // A click on the veil is a decline: an offer left unanswered times out,
        // and a question that keeps drawing attention is not a better default.
        if (event.target === event.currentTarget) onAnswer(false, false);
      }}
    >
      <div
        className="consent"
        role="alertdialog"
        aria-modal="true"
        aria-labelledby="consent-title"
        aria-describedby="consent-note"
      >
        <p className="consent-kicker">Incoming</p>
        <h2 id="consent-title">
          {offer.sender} wants to send {files}
        </h2>
        <p className="consent-note" id="consent-note">
          {bytes(offer.bytes)} &middot;{" "}
          {folder === null ? (
            "Nothing is saved until you accept."
          ) : (
            <>
              Nothing is saved until you accept. These land in{" "}
              <span className="consent-folder">{folder}</span>.
            </>
          )}
        </p>

        <ul className="consent-files">
          {listed.map((item, index) => (
            <li key={`${item.name}-${index}`}>
              <span className="consent-name">{item.name}</span>
              <span className="consent-size">
                {item.text ? "text" : bytes(item.bytes)}
              </span>
            </li>
          ))}
          {extra > 0 ? (
            <li className="consent-more">and {extra} more</li>
          ) : null}
        </ul>

        {canRemember ? (
          <label className="consent-remember">
            <input
              type="checkbox"
              checked={remember}
              onChange={() => setRemember((value) => !value)}
            />
            <span>
              <strong>Always accept from this device</strong>
              <em>from {shortFingerprint(offer.fingerprint)}</em>
            </span>
          </label>
        ) : (
          <p className="consent-anon">
            This sender did not prove an identity, so Flit cannot remember it.
          </p>
        )}

        <div className="consent-actions">
          <button
            type="button"
            className="btn btn-quiet"
            onClick={() => onAnswer(false, false)}
          >
            Decline
          </button>
          <button type="button" className="btn btn-accent" onClick={accepting}>
            Accept
          </button>
        </div>
      </div>
    </div>
  );
}
