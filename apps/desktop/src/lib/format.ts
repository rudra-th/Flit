/** Every number the UI prints goes through here, so units and decimals are
 * decided once instead of per component. */

const UNITS = ["B", "KB", "MB", "GB", "TB"];

/** Bytes as a short human size: `412 MB`, `1.4 GB`. */
export function bytes(value: number): string {
  if (!Number.isFinite(value) || value <= 0) return "0 B";
  let unit = 0;
  let size = value;
  while (size >= 1000 && unit < UNITS.length - 1) {
    size /= 1000;
    unit += 1;
  }
  // Under 10 keep one decimal; above it the decimal is noise at this size.
  const digits = size >= 10 || unit === 0 ? 0 : 1;
  return `${size.toFixed(digits)} ${UNITS[unit]}`;
}

/** A transfer rate: `62.4 MB/s`, `—` before the first sample. */
export function rate(bytesPerSecond: number): string {
  if (!Number.isFinite(bytesPerSecond) || bytesPerSecond <= 0) return "—";
  return `${bytes(bytesPerSecond)}/s`;
}

/** A duration as words, not clock digits: `840 ms`, `24 s`, `4 m 12 s`. */
export function duration(ms: number): string {
  if (!Number.isFinite(ms) || ms < 0) return "—";
  if (ms < 1000) return `${Math.round(ms)} ms`;
  const seconds = Math.round(ms / 1000);
  if (seconds < 60) return `${seconds} s`;
  const minutes = Math.floor(seconds / 60);
  const rest = seconds % 60;
  if (minutes < 60) return rest === 0 ? `${minutes} m` : `${minutes} m ${rest} s`;
  const hours = Math.floor(minutes / 60);
  const restMinutes = minutes % 60;
  return restMinutes === 0 ? `${hours} h` : `${hours} h ${restMinutes} m`;
}

/** A whole percentage, never `NaN`, never 101. */
export function percent(done: number, total: number): number {
  if (!Number.isFinite(total) || total <= 0) return done > 0 ? 100 : 0;
  return Math.max(0, Math.min(100, Math.round((done / total) * 100)));
}

const DAY = 86_400_000;
const MONTHS = [
  "Jan",
  "Feb",
  "Mar",
  "Apr",
  "May",
  "Jun",
  "Jul",
  "Aug",
  "Sep",
  "Oct",
  "Nov",
  "Dec",
];

function clock(date: Date): string {
  return `${date.getHours().toString().padStart(2, "0")}:${date
    .getMinutes()
    .toString()
    .padStart(2, "0")}`;
}

/** Recency, in words, on one line: `Today 14:02`, `Yesterday 09:41`,
 * `4 Nov 18:20`. */
export function when(at: number, now = Date.now()): string {
  if (!Number.isFinite(at) || at <= 0) return "—";
  const date = new Date(at);
  const startOfToday = new Date(now);
  startOfToday.setHours(0, 0, 0, 0);
  const diff = startOfToday.getTime() - at;
  if (diff <= 0) return `Today ${clock(date)}`;
  if (diff <= DAY) return `Yesterday ${clock(date)}`;
  return `${date.getDate()} ${MONTHS[date.getMonth()]} ${clock(date)}`;
}

/** The trailing segment of a path, for a row that has limited width. */
export function baseName(path: string): string {
  const parts = path.split(/[\\/]/).filter(Boolean);
  return parts.length > 0 ? parts[parts.length - 1] : path;
}

/** First 10 hex characters: enough to compare against a peer's screen, short
 * enough to read. The full value is always one copy away. */
export function shortFingerprint(fingerprint: string): string {
  return fingerprint.length > 12 ? fingerprint.slice(0, 10) : fingerprint;
}

/** `desk` -> `Desk`, `headless` -> `Headless box`. */
export function deviceKind(kind: string): string {
  switch (kind) {
    case "headless":
      return "headless box";
    case "laptop":
      return "laptop";
    case "phone":
      return "phone";
    case "tablet":
      return "tablet";
    default:
      return "desktop";
  }
}

/** `192.168.1.24:47821` -> `192.168.1.24`; IPv6 kept whole. */
export function hostPart(addr: string): string {
  if (addr.startsWith("[")) {
    const end = addr.indexOf("]");
    return end > 0 ? addr.slice(1, end) : addr;
  }
  const colon = addr.lastIndexOf(":");
  return colon > 0 ? addr.slice(0, colon) : addr;
}
