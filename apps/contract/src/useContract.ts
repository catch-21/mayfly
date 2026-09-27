// One open contract: the browser client's chain session, with the contract's verbs on top.
//
// The client does the loop. This hook only names the bodies `contract/1` understands.
// Accept and reject name the offer that is open now, so they are not held. An offer or a
// revision is held until a round takes it.

import { useMemo } from "react";
import type { Party } from "@synonymdev/mayfly-browser";
import { useChainSession, type SessionHandle } from "@synonymdev/mayfly-browser/react";

import { app, contractRules } from "./config";
import type { ContractBody, ContractOffer, ContractState } from "./rules";

export type ContractHandle = SessionHandle<ContractState, ContractBody> & {
  offer(text: string): Promise<void>;
  revise(text: string, open: ContractOffer): Promise<void>;
  accept(id: string): Promise<void>;
  rejectOffer(id: string): Promise<void>;
  record(): Promise<void>;
  walkAway(): Promise<void>;
  /** End the negotiation with no contract. The other app confirms the record; it is not passed on. */
  letGo(): Promise<void>;
};

/** A short random id for an offer. Ids are per chain. */
function offerId(): string {
  const bytes = new Uint8Array(6);
  crypto.getRandomValues(bytes);
  return Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
}

export function useContract(party: Party, url: string): ContractHandle {
  const s = useChainSession<ContractState, ContractBody>(app, party, url, contractRules);
  return useMemo(
    () => ({
      ...s,
      offer: (text) => s.propose({ kind: "offer", id: offerId(), text, base: "" }),
      revise: (text, open) => s.propose({ kind: "offer", id: offerId(), text, base: open.id }),
      accept: (id) => s.propose({ kind: "accept", id }, { hold: false }),
      rejectOffer: (id) => s.propose({ kind: "reject", id }, { hold: false }),
      record: () => s.proposeClose("finished"),
      walkAway: () => s.proposeClose("agreed"),
      letGo: () => s.propose({ kind: "lapse" }),
    }),
    [s],
  );
}
