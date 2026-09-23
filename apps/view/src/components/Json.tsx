import type { ReactNode } from "react";

// Pretty-printed JSON with light syntax colouring: keys, strings, numbers, booleans, null.
// Rendered as nested spans rather than a highlighted string so nothing is ever re-parsed.

const INDENT = "  ";

function render(v: unknown, depth: number): ReactNode {
  if (v === null || v === undefined) return <span className="z">null</span>;
  if (typeof v === "string") return <span className="s">{JSON.stringify(v)}</span>;
  if (typeof v === "number" || typeof v === "bigint") return <span className="n">{String(v)}</span>;
  if (typeof v === "boolean") return <span className="b">{String(v)}</span>;
  const pad = INDENT.repeat(depth + 1);
  const close = INDENT.repeat(depth);
  if (Array.isArray(v)) {
    if (v.length === 0) return <span className="p">[]</span>;
    return (
      <>
        <span className="p">[</span>
        {v.map((item, i) => (
          <span key={i}>
            {"\n"}
            {pad}
            {render(item, depth + 1)}
            {i < v.length - 1 ? <span className="p">,</span> : null}
          </span>
        ))}
        {"\n"}
        {close}
        <span className="p">]</span>
      </>
    );
  }
  if (typeof v === "object") {
    const entries = Object.entries(v as Record<string, unknown>);
    if (entries.length === 0) return <span className="p">{"{}"}</span>;
    return (
      <>
        <span className="p">{"{"}</span>
        {entries.map(([k, val], i) => (
          <span key={k}>
            {"\n"}
            {pad}
            <span className="k">{JSON.stringify(k)}</span>
            <span className="p">: </span>
            {render(val, depth + 1)}
            {i < entries.length - 1 ? <span className="p">,</span> : null}
          </span>
        ))}
        {"\n"}
        {close}
        <span className="p">{"}"}</span>
      </>
    );
  }
  return <span className="z">{String(v)}</span>;
}

export function Json({ value }: { value: unknown }) {
  return (
    <pre className="json">
      <code className="json">{render(value, 0)}</code>
    </pre>
  );
}
