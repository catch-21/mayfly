import { useState } from "react";

import { copyText, shorten } from "../format";

/** A long identifier shown shortened, full on hover, copied on click. */
export function Short({ value, head, tail }: { value: string; head?: number; tail?: number }) {
  const [copied, setCopied] = useState(false);
  const onClick = async () => {
    if (await copyText(value)) {
      setCopied(true);
      setTimeout(() => setCopied(false), 1200);
    }
  };
  return (
    <span
      className={`mono short${copied ? " copied" : ""}`}
      title={copied ? "copied" : `${value} (click to copy)`}
      onClick={onClick}
    >
      {shorten(value, head, tail)}
    </span>
  );
}
