/** Line icons at 1.5px stroke, 16px by default.
 *
 * They are drawn here instead of pulled from an icon package so the set has
 * one weight and one corner treatment, and so nothing in the UI is a stock
 * glyph from a kit. */

import type { SVGProps } from "react";

type IconProps = SVGProps<SVGSVGElement> & { size?: number };

function base({ size = 16, ...rest }: IconProps) {
  return {
    width: size,
    height: size,
    viewBox: "0 0 24 24",
    fill: "none",
    stroke: "currentColor",
    strokeWidth: 1.5,
    strokeLinecap: "round" as const,
    strokeLinejoin: "round" as const,
    "aria-hidden": true,
    ...rest,
  };
}

/** The product mark: two chevrons, the whole idea in 16px. */
export function Mark({ size = 20, ...rest }: IconProps) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={2}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
      {...rest}
    >
      <path d="M4 6.5 10 12l-6 5.5" />
      <path d="M12.5 6.5 18.5 12l-6 5.5" opacity={0.55} />
    </svg>
  );
}

export function SendIcon(props: IconProps) {
  return (
    <svg {...base(props)}>
      <path d="M4 12h11" />
      <path d="M10.5 7 15.5 12l-5 5" />
      <path d="M20 5v14" opacity={0.5} />
    </svg>
  );
}

export function HistoryIcon(props: IconProps) {
  return (
    <svg {...base(props)}>
      <circle cx="12" cy="12" r="7.75" />
      <path d="M12 8.25V12l2.75 1.75" />
    </svg>
  );
}

export function PulseIcon(props: IconProps) {
  return (
    <svg {...base(props)}>
      <path d="M3 12h3.5l2.25-5.5 3.5 11 2.5-5.5H21" />
    </svg>
  );
}

export function DeviceIcon(props: IconProps) {
  return (
    <svg {...base(props)}>
      <rect x="3.25" y="4.25" width="17.5" height="11.5" rx="1.75" />
      <path d="M9 20h6M12 15.75V20" />
    </svg>
  );
}

export function LaptopIcon(props: IconProps) {
  return (
    <svg {...base(props)}>
      <rect x="4.5" y="5" width="15" height="10" rx="1.5" />
      <path d="M2.75 18.5h18.5" />
    </svg>
  );
}

export function PhoneIcon(props: IconProps) {
  return (
    <svg {...base(props)}>
      <rect x="7.25" y="2.75" width="9.5" height="18.5" rx="2.25" />
      <path d="M10.5 18.5h3" />
    </svg>
  );
}

export function TabletIcon(props: IconProps) {
  return (
    <svg {...base(props)}>
      <rect x="5.25" y="3" width="13.5" height="18" rx="2" />
      <path d="M10.5 18.25h3" />
    </svg>
  );
}

export function ServerIcon(props: IconProps) {
  return (
    <svg {...base(props)}>
      <rect x="3.75" y="4.25" width="16.5" height="6" rx="1.5" />
      <rect x="3.75" y="13.75" width="16.5" height="6" rx="1.5" />
      <path d="M7.25 7.25h.01M7.25 16.75h.01" />
    </svg>
  );
}

export function PlusIcon(props: IconProps) {
  return (
    <svg {...base(props)}>
      <path d="M12 5.5v13M5.5 12h13" />
    </svg>
  );
}

export function CloseIcon(props: IconProps) {
  return (
    <svg {...base(props)}>
      <path d="M6.5 6.5l11 11M17.5 6.5l-11 11" />
    </svg>
  );
}

export function MinusIcon(props: IconProps) {
  return (
    <svg {...base(props)}>
      <path d="M5.5 12h13" />
    </svg>
  );
}

export function WindowIcon(props: IconProps) {
  return (
    <svg {...base(props)}>
      <rect x="5.5" y="5.5" width="13" height="13" rx="1.5" />
    </svg>
  );
}

export function DropIcon(props: IconProps) {
  return (
    <svg {...base(props)}>
      <path d="M12 4v8.5M8.75 9.5 12 12.75l3.25-3.25" />
      <path d="M4.75 17v1.25a1.5 1.5 0 0 0 1.5 1.5h11.5a1.5 1.5 0 0 0 1.5-1.5V17" />
    </svg>
  );
}

export function FolderIcon(props: IconProps) {
  return (
    <svg {...base(props)}>
      <path d="M3.75 6.75A1.75 1.75 0 0 1 5.5 5h3.1a1.5 1.5 0 0 1 1.1.46l1.2 1.24H18.5a1.75 1.75 0 0 1 1.75 1.75v7.3a1.75 1.75 0 0 1-1.75 1.75h-13a1.75 1.75 0 0 1-1.75-1.75Z" />
    </svg>
  );
}

export function FileIcon(props: IconProps) {
  return (
    <svg {...base(props)}>
      <path d="M13.5 4H7.25A1.75 1.75 0 0 0 5.5 5.75v12.5A1.75 1.75 0 0 0 7.25 20h9.5a1.75 1.75 0 0 0 1.75-1.75V8.5Z" />
      <path d="M13.5 4v4.5h5" />
    </svg>
  );
}

export function CopyIcon(props: IconProps) {
  return (
    <svg {...base(props)}>
      <rect x="9" y="9" width="10.5" height="10.5" rx="1.5" />
      <path d="M6.5 15H5a1.5 1.5 0 0 1-1.5-1.5V5A1.5 1.5 0 0 1 5 3.5h8.5A1.5 1.5 0 0 1 15 5v1.5" />
    </svg>
  );
}

export function CheckIcon(props: IconProps) {
  return (
    <svg {...base(props)}>
      <path d="M5 12.5 9.5 17 19 7" />
    </svg>
  );
}

export function AlertIcon(props: IconProps) {
  return (
    <svg {...base(props)}>
      <circle cx="12" cy="12" r="8" />
      <path d="M12 8v4.75M12 15.75v.25" />
    </svg>
  );
}
