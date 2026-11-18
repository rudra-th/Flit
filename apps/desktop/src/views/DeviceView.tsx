import { useState, type FormEvent } from "react";

import { CopyButton } from "../components/CopyButton";
import { shortFingerprint } from "../lib/format";
import type { Peer, ReceivingPolicy, Status } from "../lib/types";

/** Longest name the shell will announce or a peer list can show. */
const MAX_NAME = 32;

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

/** The name, with the one control that changes it. Saving restarts the shell,
 * because the announce is fixed when the receiver starts. */
function NameField({
  status,
  onRename,
}: {
  status: Status | null;
  onRename: (name: string) => Promise<void>;
}) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState("");
  const [saving, setSaving] = useState(false);

  if (status === null) {
    return <Field label="Name" value={null} />;
  }

  if (!editing) {
    return (
      <div className="field">
        <span className="field-label">Name</span>
        <span className="field-value text">{status.alias}</span>
        <button
          type="button"
          className="btn btn-quiet"
          onClick={() => {
            setDraft(status.alias);
            setEditing(true);
          }}
        >
          Change
        </button>
      </div>
    );
  }

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    setSaving(true);
    try {
      await onRename(draft);
      // The shell restarts in a moment; leave the form while it does.
      setEditing(false);
    } catch {
      setSaving(false);
    }
  };

  return (
    <form className="field" onSubmit={submit}>
      <label className="field-label" htmlFor="device-name">
        Name
      </label>
      <span className="name-form">
        <input
          id="device-name"
          value={draft}
          maxLength={MAX_NAME}
          autoFocus
          onChange={(event) => setDraft(event.target.value)}
        />
        <button
          type="submit"
          className="btn"
          disabled={saving || draft.trim().length === 0}
        >
          {saving ? "Saving" : "Save"}
        </button>
        <button
          type="button"
          className="btn btn-quiet"
          disabled={saving}
          onClick={() => setEditing(false)}
        >
          Cancel
        </button>
      </span>
    </form>
  );
}

/** The two rules receiving follows: one switch, and the keys the person chose
 * to trust. Off is the default and it is the honest one — a file that lands
 * cannot be taken back from here. */
function ReceivingPanel({
  policy,
  peers,
  onAutoAccept,
  onForget,
}: {
  policy: ReceivingPolicy;
  peers: Peer[];
  onAutoAccept: (enabled: boolean) => void;
  onForget: (fingerprint: string) => void;
}) {
  const trusted = policy.trusted;

  return (
    <section className="panel receiving" aria-label="Receiving">
      <div className="panel-head">
        <span className="panel-title">Receiving</span>
        <span className="panel-meta">
          {policy.autoAccept
            ? "accepts every device"
            : trusted.length > 0
              ? `asks first · ${trusted.length} trusted`
              : "asks first"}
        </span>
      </div>

      <div className="receiving-switch">
        <div className="receiving-copy">
          <strong>Accept every device without asking</strong>
          <p>
            Off is the default. While it is off, Flit asks before each transfer
            except for the devices below. A PIN is still checked.
          </p>
        </div>
        <button
          type="button"
          role="switch"
          aria-checked={policy.autoAccept}
          aria-label="Accept every device without asking"
          className="switch"
          onClick={() => onAutoAccept(!policy.autoAccept)}
        >
          <span className="switch-knob" aria-hidden />
        </button>
      </div>

      <div className="receiving-trusted">
        <span className="receiving-label">Trusted devices</span>
        {trusted.length === 0 ? (
          <p className="receiving-none">
            None yet. Tick &ldquo;Always accept from this device&rdquo; on a
            transfer to add one.
          </p>
        ) : (
          <ul className="trusted">
            {trusted.map((fingerprint) => {
              // A live peer name is the current one, which is easier to
              // recognise than a key and grants nothing on its own.
              const live = peers.find(
                (peer) =>
                  peer.fingerprint.toLowerCase() === fingerprint.toLowerCase(),
              );
              return (
                <li key={fingerprint}>
                  <span className="trusted-name">
                    {live?.alias ?? "Trusted device"}
                  </span>
                  <span className="trusted-key" title={fingerprint}>
                    {shortFingerprint(fingerprint)}&hellip;
                  </span>
                  <button
                    type="button"
                    className="btn btn-quiet"
                    onClick={() => onForget(fingerprint)}
                  >
                    Forget
                  </button>
                </li>
              );
            })}
          </ul>
        )}
      </div>
    </section>
  );
}

/** Who this device is to everyone else: the name they see, the fingerprint
 * they pin, and where it answers. */
export function DeviceView({
  status,
  policy,
  peers,
  onRename,
  onAutoAccept,
  onForget,
}: {
  status: Status | null;
  policy: ReceivingPolicy;
  peers: Peer[];
  onRename: (name: string) => Promise<void>;
  onAutoAccept: (enabled: boolean) => void;
  onForget: (fingerprint: string) => void;
}) {
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
            Other devices see this name. Anyone can type any name, so the
            fingerprint — not the name — is what Flit pins on the first send.
          </p>
        </div>
      </header>

      <div className="identity">
        <section className="panel" aria-label="Identity">
          <NameField status={status} onRename={onRename} />
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

        <aside className="panel name-card" aria-label="Name and identity">
          <span className="alias">{status?.alias ?? "—"}</span>
          <p>
            Two words are easy to read across a room, which is all a name is
            for. The key behind it never changes: Flit pins the fingerprint on
            the first send, so a device that renames itself cannot quietly
            become one you already trust.
          </p>
        </aside>
      </div>

      <ReceivingPanel
        policy={policy}
        peers={peers}
        onAutoAccept={onAutoAccept}
        onForget={onForget}
      />

      <p className="foot-note">
        Flit keeps running in the tray when the window is closed, so this device
        stays reachable. Nothing about it is sent to a server.
      </p>
    </div>
  );
}
