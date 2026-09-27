// One open game. The client runs the loop. This hook names the chess verbs, draws colours
// when they were left to the reveal, and confirms a reveal because it is not a move.

import { useEffect, useMemo, useRef } from "react";
import type { Party } from "@synonymdev/mayfly-browser";
import { useChainSession, type SessionHandle } from "@synonymdev/mayfly-browser/react";

import { app, RULES, type ChessState } from "./config";

export type ChessHandle = SessionHandle<ChessState, { kind: string; uci?: string }> & {
  move(uci: string): Promise<void>;
  resign(): Promise<void>;
  offerDraw(): Promise<void>;
  acceptDraw(): Promise<void>;
  recordResult(): Promise<void>;
  claimTime(party: number): Promise<void>;
};

export function useChess(party: Party, url: string): ChessHandle {
  const s = useChainSession<ChessState, { kind: string; uci?: string }>(app, party, url, RULES, {
    autoPass: false,
    holdProposals: false,
  });
  const asked = useRef(false);

  useEffect(() => {
    if (s.phase !== "open" || s.state || s.myIndex !== 1 || asked.current) return;
    asked.current = true;
    void s.proposeReveal();
  }, [s.phase, s.state, s.myIndex, s]);

  useEffect(() => {
    for (const d of s.decisions) {
      if (!d.action.repropose && d.action.candidate.kind === "reveal") {
        void s.confirm(d.action.candidate.hash);
      }
    }
  }, [s.decisions, s]);

  return useMemo(
    () => ({
      ...s,
      move: (uci) => s.propose({ kind: "move", uci }, { hold: false }),
      resign: () => s.propose({ kind: "resign" }, { hold: false }),
      offerDraw: () => s.propose({ kind: "offer_draw" }, { hold: false }),
      acceptDraw: () => s.propose({ kind: "accept_draw" }, { hold: false }),
      recordResult: () => s.proposeClose("finished"),
      claimTime: (partyIndex) => s.proposeAbandoned([partyIndex]),
    }),
    [s],
  );
}
