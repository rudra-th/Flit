import { useEffect, useRef, useState } from "react";

/** A destination reached by typing it in, not by discovery.
 *
 * The address is what to dial; the fingerprint is what to believe. Both are
 * required, because the engine pins before it sends and refuses anything else.
 */
export interface AddressTarget {
  addr: string;
  fingerprint: string;
}

/** `192.168.1.8:47821` or `[fe80::1]:47821`. Hostnames are left out on
 * purpose: the engine dials a `SocketAddr`, and promising names would be a
 * promise it cannot keep. */
const ADDRESS = /^(\d{1,3}(?:\.\d{1,3}){3}|\[[0-9a-fA-F:]+\]):(\d{1,5})$/;
const FINGERPRINT = /^[0-9a-fA-F]{64}$/;

/**
 * The way to reach a device whose announcement never arrived.
 *
 * It is a fallback, not a second setup screen: two fields and the place they
 * come from. The fingerprint stays required because an address alone proves
 * nothing about who answers at it, and the copy says so instead of letting the
 * user discover that on the first failure.
 */
export function AddressDialog({
  current,
  onUse,
  onClose,
}: {
  current: AddressTarget | null;
  onUse: (target: AddressTarget) => void;
  onClose: () => void;
}) {
  const [addr, setAddr] = useState(current?.addr ?? "");
  const [fingerprint, setFingerprint] = useState(current?.fingerprint ?? "");
  const [error, setError] = useState<string | null>(null);
  const first = useRef<HTMLInputElement>(null);

  useEffect(() => {
    first.current?.focus();
  }, []);

  // Escape closes without changing the target. Enter is the form's own submit.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const use = () => {
    // Spaces are the common paste artefact, and neither field can contain one.
    const cleanAddr = addr.replace(/\s+/g, "");
    const cleanKey = fingerprint.trim().toLowerCase();

    if (!ADDRESS.test(cleanAddr)) {
      setError("An address looks like 192.168.1.8:47821. Include the port.");
      return;
    }
    const port = Number(cleanAddr.slice(cleanAddr.lastIndexOf(":") + 1));
    if (port < 1 || port > 65_535) {
      setError("That port is not one a device can listen on.");
      return;
    }
    if (!FINGERPRINT.test(cleanKey)) {
      setError("The fingerprint is the 64-character key shown on the other device.");
      return;
    }
    onUse({ addr: cleanAddr, fingerprint: cleanKey });
  };

  return (
    <div
      className="consent-veil"
      role="presentation"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <form
        className="consent address"
        role="dialog"
        aria-modal="true"
        aria-labelledby="address-title"
        aria-describedby="address-note"
        onSubmit={(event) => {
          event.preventDefault();
          use();
        }}
      >
        <p className="consent-kicker">By address</p>
        <h2 id="address-title">Send to a device Flit cannot see</h2>
        <p className="consent-note" id="address-note">
          On the other device, open its identity screen: This device on
          desktop, Settings on Android. The address and fingerprint there copy
          in one click.
        </p>

        <label className="address-field">
          <span>Address</span>
          <input
            ref={first}
            className="mono"
            value={addr}
            onChange={(event) => {
              setAddr(event.target.value);
              setError(null);
            }}
            placeholder="192.168.1.8:47821"
            spellCheck={false}
            autoComplete="off"
          />
          <em>Include the port. It is the one that device names, not always 47821.</em>
        </label>

        <label className="address-field">
          <span>Fingerprint</span>
          <input
            className="mono"
            value={fingerprint}
            onChange={(event) => {
              setFingerprint(event.target.value);
              setError(null);
            }}
            placeholder="64 hex characters"
            spellCheck={false}
            autoComplete="off"
          />
          <em>
            Flit dials only the key you name. If another device answers, the
            send fails instead of landing somewhere unexpected.
          </em>
        </label>

        {error !== null ? (
          <p className="address-error" role="alert">
            {error}
          </p>
        ) : null}

        <div className="consent-actions">
          <button type="button" className="btn btn-quiet" onClick={onClose}>
            Cancel
          </button>
          <button type="submit" className="btn btn-accent">
            Use address
          </button>
        </div>
      </form>
    </div>
  );
}
