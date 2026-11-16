import type { ComponentType, SVGProps } from "react";

import { deviceKind, hostPart } from "../lib/format";
import type { DeviceType, Peer } from "../lib/types";
import {
  DeviceIcon,
  LaptopIcon,
  PhoneIcon,
  ServerIcon,
  TabletIcon,
} from "./Icons";

const SOURCE: Record<Peer["source"], string> = {
  multicast: "nearby",
  scan: "found by scan",
  manual: "by address",
};

const KIND_ICON: Record<
  DeviceType,
  ComponentType<SVGProps<SVGSVGElement> & { size?: number }>
> = {
  desktop: DeviceIcon,
  laptop: LaptopIcon,
  phone: PhoneIcon,
  tablet: TabletIcon,
  headless: ServerIcon,
};

/** One reachable device. The whole card is the target: there is no separate
 * "send" button to hunt for, and a double-click sends straight away.
 *
 * The name is the device's own label, so it is recognition, not proof; the
 * fingerprint behind it is what the shell pins on the first send. */
export function DeviceCard({
  peer,
  selected,
  lone,
  onSelect,
  onSend,
}: {
  peer: Peer;
  selected: boolean;
  lone: boolean;
  onSelect: () => void;
  onSend: () => void;
}) {
  const Icon = KIND_ICON[peer.deviceType];

  return (
    <button
      type="button"
      className={lone ? "device single" : "device"}
      aria-pressed={selected}
      aria-label={`${peer.alias}, ${deviceKind(peer.deviceType)} at ${peer.addr}. ${
        selected
          ? "Chosen. Choose again to leave it out."
          : "Choose as a destination."
      }`}
      title={
        selected
          ? "Chosen. Double-click sends to this device alone."
          : "Choose as a destination. Double-click sends now."
      }
      onClick={onSelect}
      onDoubleClick={onSend}
    >
      <span className="device-top">
        <span className="source">{SOURCE[peer.source]}</span>
      </span>

      <span className="device-icon" data-kind={peer.deviceType} aria-hidden>
        <Icon size={22} />
      </span>

      <span className="device-name">{peer.alias}</span>

      <span className="device-meta">
        <span>{deviceKind(peer.deviceType)}</span>
        <span className="dot">·</span>
        <span className="mono">{hostPart(peer.addr)}</span>
      </span>

      <span className="device-hint">
        {selected ? "Selected" : SOURCE[peer.source]}
      </span>
    </button>
  );
}
