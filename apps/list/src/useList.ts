// One open list: the browser client's chain session, with the list's verbs on top.
//
// The client does the loop — act() after every action, on a timer, and when a member's
// homeserver reports a change; decisions, held proposals, live streams, timeouts. This hook
// only names the bodies `list/1` understands.

import { useMemo } from "react";
import type { Party } from "@synonymdev/mayfly-browser";
import { useChainSession, type SessionHandle } from "@synonymdev/mayfly-browser/react";

import { app, RULES, type ListBody, type ListState } from "./config";

export type ListHandle = SessionHandle<ListState, ListBody> & {
  items: ListState["items"];
  add(text: string): Promise<void>;
  edit(id: string, text: string): Promise<void>;
  tick(id: string, on: boolean): Promise<void>;
  remove(id: string): Promise<void>;
  archive(): Promise<void>;
  close(): Promise<void>;
};

/** A short random id for a new item; ids are per list, never shown. */
function itemId(): string {
  const bytes = new Uint8Array(6);
  crypto.getRandomValues(bytes);
  return Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
}

export function useList(party: Party, url: string): ListHandle {
  const s = useChainSession<ListState, ListBody>(app, party, url, RULES);
  return useMemo(
    () => ({
      ...s,
      items: s.state?.items ?? [],
      // An add is held until the round takes it: typed while the chain is between rounds, or
      // during another member's round, it shows as waiting rather than failing.
      add: (text) => s.propose({ kind: "add", id: itemId(), text }),
      // Everything else acts on an item that is there now; a wait would act on a stale list.
      edit: (id, text) => s.propose({ kind: "edit", id, text }, { hold: false }),
      tick: (id, on) => s.propose({ kind: on ? "tick" : "untick", id }, { hold: false }),
      remove: (id) => s.propose({ kind: "remove", id }, { hold: false }),
      archive: () => s.propose({ kind: "archive" }, { hold: false }),
      close: () => s.proposeClose("agreed"),
    }),
    [s],
  );
}
