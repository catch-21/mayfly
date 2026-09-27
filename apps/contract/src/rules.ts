// `contract/1`: one open offer between two parties. The other party accepts it, rejects it
// outright, or sends a revision. Accept or reject ends the negotiation. The chain stores the
// full text; a line diff is a view, computed in the page.

import type { CloseLike, GenesisLike, LinkLike, Outcome, RulesModule } from "@synonymdev/mayfly-browser";

/** The open offer. `base` is the id of the offer this one answers, or empty for the first. */
export interface ContractOffer {
  id: string;
  author: number;
  text: string;
  base: string;
  /** The text this offer replaced, so the page can show a redline. */
  previous: string | null;
}

/** Set once a party has accepted, rejected outright, or let the negotiation go. */
export interface ContractOutcome {
  kind: "agreed" | "rejected" | "lapsed";
  text: string;
}

export interface ContractState {
  parties: string[];
  offer: ContractOffer | null;
  outcome: ContractOutcome | null;
}

export type ContractBody =
  | { kind: "offer"; id: string; text: string; base: string }
  | { kind: "accept"; id: string }
  | { kind: "reject"; id: string }
  | { kind: "lapse" };

type OfferBody = { id?: unknown; text?: unknown; base?: unknown };
type IdBody = { id?: unknown };

function seat(state: ContractState, author: string): number {
  const i = state.parties.indexOf(author);
  if (i < 0) throw new Error("author is not a party");
  return i;
}

function responder(state: ContractState): number | undefined {
  const offer = state.offer;
  if (!offer) return undefined;
  return offer.author === 0 ? 1 : 0;
}

function ended(state: ContractState): boolean {
  return state.outcome !== null;
}

/** The rules object passed wherever a rules id is accepted. */
export const contractRules: RulesModule<ContractState, ContractBody> = {
  id: "contract/1",
  referenceHash: "contract/1-reference-hash-placeholder",

  init(genesis: GenesisLike): ContractState {
    if (genesis.parties.length !== 2) throw new Error("a contract is between two parties");
    if (genesis.confirm_quorum !== genesis.parties.length) {
      throw new Error("both parties must confirm every link");
    }
    return {
      parties: genesis.parties.map((p) => p.pubky),
      offer: null,
      outcome: null,
    };
  },

  obliged(state: ContractState): number[] {
    const who = responder(state);
    return who === undefined ? [] : [who];
  },

  mayAppend(state: ContractState, party: number, kind: string): boolean {
    if (ended(state) || party < 0 || party >= state.parties.length) return false;
    if (kind === "lapse") return true;
    if (kind === "offer") return state.offer === null || party === responder(state);
    if (kind === "accept" || kind === "reject") return state.offer !== null && party === responder(state);
    return false;
  },

  apply(state: ContractState, link: LinkLike<ContractBody>): ContractState {
    if (ended(state)) throw new Error("the negotiation has ended");
    const party = seat(state, link.author);
    if (!contractRules.mayAppend(state, party, link.kind)) {
      throw new Error("this party cannot append that now");
    }
    if (link.kind === "offer") return offer(state, party, link.body as OfferBody);
    if (link.kind === "accept") return answer(state, link.body as IdBody, "agreed");
    if (link.kind === "reject") return answer(state, link.body as IdBody, "rejected");
    if (link.kind === "lapse") return lapse(state);
    throw new Error(`unknown kind ${link.kind}`);
  },

  status(state: ContractState): Outcome | null {
    return endedSummary(state);
  },

  close(state: ContractState, close: CloseLike): Outcome {
    if (close.reason === "finished") {
      const summary = endedSummary(state);
      if (!summary) throw new Error("the negotiation has not ended");
      return summary;
    }
    if (close.reason === "agreed" && state.outcome) return endedSummary(state) ?? { summary: "no contract" };
    return { summary: "no contract" };
  },
};

function endedSummary(state: ContractState): Outcome | null {
  if (!state.outcome) return null;
  if (state.outcome.kind === "lapsed") return { summary: "no contract" };
  return { summary: state.outcome.kind };
}

function lapse(state: ContractState): ContractState {
  return {
    parties: state.parties,
    offer: null,
    outcome: { kind: "lapsed", text: state.offer?.text ?? "" },
  };
}

function offer(state: ContractState, party: number, body: OfferBody): ContractState {
  const id = typeof body.id === "string" ? body.id : "";
  const text = typeof body.text === "string" ? body.text : "";
  const base = typeof body.base === "string" ? body.base : "";
  if (!id) throw new Error("an offer needs an id");
  if (!text) throw new Error("offer text is empty");
  const open = state.offer;
  if (!open) {
    if (base !== "") throw new Error("the first offer has no base");
    return { ...state, offer: { id, author: party, text, base, previous: null } };
  }
  if (base !== open.id) throw new Error("revision must name the open offer");
  if (text === open.text) throw new Error("revision must change the text");
  return { ...state, offer: { id, author: party, text, base, previous: open.text } };
}

function answer(state: ContractState, body: IdBody, kind: "agreed" | "rejected"): ContractState {
  const id = typeof body.id === "string" ? body.id : "";
  const open = state.offer;
  if (!open || id !== open.id) throw new Error("that offer is not open");
  return { parties: state.parties, offer: null, outcome: { kind, text: open.text } };
}
