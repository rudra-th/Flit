import { CopyButton } from "../components/CopyButton";
import { AlertIcon, CheckIcon } from "../components/Icons";
import type { Doctor } from "../lib/types";

const MARKS = {
  ok: <CheckIcon size={12} />,
  bad: <AlertIcon size={12} />,
  wait: null,
  unknown: null,
} as const;

/** The checks a person can act on, each with its result in one line and a
 * copyable value. "Unknown" is a state, not a red mark: a check this build
 * cannot read is not a failure. */
export function DoctorView({
  doctor,
  onRefresh,
}: {
  doctor: Doctor | null;
  onRefresh: () => void;
}) {
  const checks = doctor?.checks ?? [];
  const bad = checks.filter((check) => check.state === "bad");
  const pending = checks.filter((check) => check.state === "wait");
  const unknown = checks.filter((check) => check.state === "unknown");

  const headline =
    doctor === null
      ? "Reading the network…"
      : bad.length > 0
        ? `${bad.length} check${bad.length === 1 ? "" : "s"} need attention.`
        : pending.length > 0
          ? `All answering checks pass · ${pending.length} still running.`
          : `All ${checks.length - unknown.length} checks that can run here pass.`;

  return (
    <div className="page">
      <header className="title-row">
        <div>
          <h1>Network doctor</h1>
          <p className="title-note">{headline}</p>
        </div>
        <div className="title-actions">
          <button type="button" className="btn" onClick={onRefresh}>
            Check again
          </button>
        </div>
      </header>

      <section className="panel" aria-label="Checks">
        <div className="checks">
          {checks.map((check) => (
            <div className="check" key={check.id}>
              <span className="check-name">
                <span className="check-dot" data-state={check.state} aria-hidden />
                {check.name}
              </span>
              <div className="check-detail">
                {check.detail}
                {check.sub ? <p className="sub">{check.sub}</p> : null}
              </div>
              <span className="check-value">
                {check.value ? <span className="mono">{check.value}</span> : null}
                {check.value ? (
                  <CopyButton text={check.value} label="" />
                ) : (
                  <span className="check-mark">{MARKS[check.state]}</span>
                )}
              </span>
            </div>
          ))}
        </div>
      </section>

      <section className="panel interface-panel" aria-label="Network interfaces">
        <div className="panel-head">
          <span className="panel-title">Interfaces</span>
          <span className="panel-meta">{doctor?.interfaces.length ?? 0}</span>
        </div>
        <div className="iface-list">
          {(doctor?.interfaces ?? []).map((iface) => (
            <div className="iface-row" key={`${iface.name}-${iface.addr}`}>
              <span className="iface-name">{iface.name}</span>
              <span className="iface-addr mono">{iface.addr}</span>
              <span className="tag">
                {iface.loopback ? "loopback" : iface.up ? "up" : "down"}
              </span>
            </div>
          ))}
        </div>
      </section>

      {bad.length > 0 || pending.length > 0 ? (
        <section className="panel section-gap" aria-label="What to try">
          <div className="panel-head">
            <span className="panel-title">What to try</span>
          </div>
          <ul className="hint-list">
            <li>
              Allow Flit on private networks in the firewall, then send again.
              Public networks are refused by Windows by default.
            </li>
            <li>
              Both devices must be on the same network. Guest Wi-Fi often blocks
              device-to-device traffic even when multicast works.
            </li>
            <li>
              Multicast does not cross subnets. If a device is on another VLAN,
              send to its address instead.
            </li>
          </ul>
        </section>
      ) : null}

      <p className="foot-note">
        Every value here can be copied. Nothing on this screen leaves the device.
      </p>
    </div>
  );
}
