import type { ListState } from "../types";
import { Json } from "./Json";

function isListState(v: unknown): v is ListState {
  if (!v || typeof v !== "object") return false;
  const o = v as Record<string, unknown>;
  return Array.isArray(o.items) && typeof o.archived === "boolean";
}

function Checklist({ state }: { state: ListState }) {
  return (
    <>
      {state.archived ? <span className="badge warn">archived</span> : null}
      <ul className="checklist">
        {state.items.map((it) => (
          <li key={it.id} className={it.ticked ? "ticked" : ""}>
            <span className="box">{it.ticked ? "[x]" : "[ ]"}</span>
            <span className="text">{it.text}</span>
            {it.qty !== undefined && it.qty !== null ? <span className="qty">x{it.qty}</span> : null}
            <span className="id mono">{it.id}</span>
          </li>
        ))}
        {state.items.length === 0 ? <li className="empty">empty list</li> : null}
      </ul>
    </>
  );
}

/**
 * The rules state at the head. Always the generic JSON; for `list/1` also a checklist, since
 * that is the one rules id this build knows how to draw.
 */
export function RulesState({ state, rules }: { state: unknown; rules: string | null }) {
  if (state === null || state === undefined) return null;
  const asList = rules === "list/1" && isListState(state);
  return (
    <section className="panel">
      <h2>Rules state{rules ? ` (${rules})` : ""}</h2>
      {asList ? <Checklist state={state} /> : null}
      <details open={!asList}>
        <summary>state as JSON</summary>
        <Json value={state} />
      </details>
    </section>
  );
}
