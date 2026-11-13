import { CopyButton } from "../components/CopyButton";
import { shortFingerprint } from "../lib/format";
import type { Status } from "../lib/types";

function Field({
  label,
  value,
  text,
  mono,
  copy,
}: {
  label: string;
  value: string | null;
  text?: boolean;
  mono?: boolean;
  copy?: string;
}) {
  const className = ["field-value", text ? "text" : "", mono ? "mono" : ""]
    .filter(Boolean)
    .join(" ");

  return (
    <div className="field">
      <span className="field-label">{label}</span>
      <span className={className}>{value ?? "—"}</span>
      {copy ? <CopyButton text={copy} /> : null}
    </div>
  );
}

/** Who this device is to everyone else: the name they see, the fingerprint
 * they pin, and the four symbols they compare. */
export function DeviceView({ status }: { status: Status | null }) {
  const discovery =
    status === null
      ? "—"
      : status.discovery.state === "running"
        ? "on"
        : status.discovery.state === "off"
          ? "off"
          : `blocked · ${status.discovery.reason}`;

  return (
    <div className="page">
      <header className="title-row">
        <div>
          <h1>This device</h1>
          <p className="title-note">
            Other devices see this name and these four symbols. The fingerprint is
            the identity; the rest is cosmetic.
          </p>
        </div>
      </header>

      <div className="identity">
        <section className="panel" aria-label="Identity">
          <Field label="Name" value={status?.alias ?? null} text />
          <Field
            label="Fingerprint"
            value={
              status === null
                ? null
                : `${shortFingerprint(status.fingerprint)}…`
            }
            mono
            copy={status?.fingerprint}
          />
          <Field label="Symbols" value={status?.badge?.words ?? null} copy={status?.badge?.words} />
          <Field
            label="Listening"
            value={status?.addr ?? null}
            mono
            copy={status?.addr}
          />
          <Field
            label="Receive folder"
            value={status?.root ?? null}
            mono
            copy={status?.root}
          />
          <Field label="Discovery" value={discovery} />
          <Field
            label="Build"
            value={
              status === null
                ? null
                : `Flit ${status.version} · protocol ${status.proto}`
            }
          />
        </section>

        <aside className="panel badge-card" aria-label="Safety badge">
          <span className="badge" aria-hidden>
            {status?.badge?.emoji ?? ""}
          </span>
          <span className="words">{status?.badge?.words ?? ""}</span>
          <p>
            Compare these four symbols with the other device before the first
            send. They are derived from the key, so a peer cannot show a matching
            badge without holding the matching identity.
          </p>
        </aside>
      </div>

      <p className="foot-note">
        Flit keeps running in the tray when the window is closed, so this device
        stays reachable. Nothing about it is sent to a server.
      </p>
    </div>
  );
}
