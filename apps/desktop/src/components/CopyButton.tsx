import { useEffect, useState } from "react";

import { copyText } from "../lib/clipboard";
import { CheckIcon, CopyIcon } from "./Icons";

/** Copies its text and says so for a moment, without a toast. */
export function CopyButton({
  text,
  label = "Copy",
}: {
  text: string;
  label?: string;
}) {
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    if (!copied) return;
    const timer = window.setTimeout(() => setCopied(false), 1400);
    return () => window.clearTimeout(timer);
  }, [copied]);

  return (
    <button
      type="button"
      className="btn btn-quiet"
      onClick={async () => {
        if (await copyText(text)) setCopied(true);
      }}
      aria-label={copied ? "Copied" : `${label}: ${text}`}
    >
      {copied ? <CheckIcon size={12} /> : <CopyIcon size={12} />}
      {copied ? "Copied" : label}
    </button>
  );
}
