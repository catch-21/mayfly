import { useEffect, useMemo, useState } from "react";
import type { ChainView, LinkView, Party } from "@synonymdev/mayfly-browser";
import { RulesRegistry, chessPgn, chessView, shippedRules } from "@synonymdev/mayfly-browser";
import { useChainReader } from "@synonymdev/mayfly-browser/react";

import { Board } from "./Board";
import { VIEWER_URL, type ChessState } from "./config";
import { copy, duration, partyLabel, short } from "./format";
import { useChess } from "./useChess";

const PROMOTE = [
  { piece: "q", label: "Queen" },
  { piece: "r", label: "Rook" },
  { piece: "b", label: "Bishop" },
  { piece: "n", label: "Knight" },
];

function thinkAllowance(options: Record<string, unknown> | undefined): number | undefined {
  const tc = options?.time_control;
  if (!tc || typeof tc !== "object") return undefined;
  const ms = (tc as { think_ms?: unknown }).think_ms;
  return typeof ms === "number" ? ms : undefined;
}

function seatColour(state: ChessState | undefined, roles: (string | null)[] | undefined, index: number): string {
  if (state) {
    if (state.white === index) return "White";
    if (state.black === index) return "Black";
  }
  if (roles?.[index] === "white") return "White";
  if (roles?.[index] === "black") return "Black";
  return "colour not drawn yet";
}

export function GamePage({ party, url, onBack }: { party: Party; url: string; onBack: () => void }) {
  const game = useChess(party, url);
  const registry = useMemo(() => new RulesRegistry(shippedRules()), []);
  const reading = game.phase === "stranger";
  const reader = useChainReader<ChessState>(reading ? { store: party.store, registry, url } : undefined);
  const view = reading ? reader.loaded?.view ?? undefined : game.view;
  const state = view?.state ?? undefined;
  const board = state ? chessView(state) : undefined;
  const [selected, setSelected] = useState<string | null>(null);
  const [promos, setPromos] = useState<string[] | null>(null);
  const [copied, setCopied] = useState(false);
  const [pgn, setPgn] = useState<string>();
  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, []);

  const share = async () => {
    setCopied(await copy(url));
    setTimeout(() => setCopied(false), 1500);
  };

  const header = (
    <header className="bar">
      <button className="link" onClick={onBack}>
        ← your games
      </button>
      <span className="grow" />
      <button className="small" onClick={share}>
        {copied ? "link copied" : "copy invite link"}
      </button>
      {VIEWER_URL && (
        <a className="small button" href={`${VIEWER_URL}#${encodeURIComponent(url)}`} target="_blank" rel="noreferrer">
          open in viewer
        </a>
      )}
    </header>
  );

  if (game.phase === "loading") {
    return (
      <main className="narrow">
        {header}
        <p className="dim">{game.error ?? "Reading the game from both homeservers…"}</p>
      </main>
    );
  }

  if (game.phase === "invited" || game.phase === "waiting") {
    const watchmen = game.arrangement?.witnesses ?? [];
    const allowance = thinkAllowance(game.arrangement?.options);
    return (
      <main className="narrow">
        {header}
        <section className="card">
          {game.phase === "waiting" ? (
            <>
              <h2>Waiting for the other player</h2>
              <p>
                {game.myIndex === 0 ? "You started this game. Send them the invite link." : "You have joined."}{" "}
                The game starts when both of you have signed the terms.
              </p>
            </>
          ) : (
            <>
              <h2>Join this game?</h2>
              <p>
                Joining signs your agreement to these two seats, a quorum of 2,{" "}
                {allowance ? `${duration(allowance)} for each move` : "the time allowance written in the terms"}, and{" "}
                {watchmen.length ? `a watchman (${short(watchmen[0])}) timing every move` : "no watchman, so the moves will not be timed"}.
              </p>
            </>
          )}
          <ul className="lists">
            {(game.arrangement?.parties ?? []).map((p, i) => (
              <li key={p}>
                {seatColour(undefined, game.arrangement?.roles, i)} · {p === game.me ? "you" : short(p)}
              </li>
            ))}
          </ul>
          {game.phase === "invited" && (
            <button onClick={() => void game.join()} disabled={game.busy}>
              {game.busy ? "Joining…" : "Join"}
            </button>
          )}
          {game.error && <p className="error">{game.error}</p>}
        </section>
      </main>
    );
  }

  return (
    <main className="wide">
      {header}
      {reading && (
        <p className="dim">
          You are not one of the two players. This is the game as anyone can read it
          {reader.final ? ", and it is final" : ""}.
        </p>
      )}
      {reading && reader.error && <p className="error">{reader.error}</p>}
      <Play
        view={view}
        state={state}
        boardFen={board?.fen}
        legal={board?.legal_uci ?? []}
        sans={board?.san ?? []}
        takenByWhite={board?.captured_by_white ?? ""}
        takenByBlack={board?.captured_by_black ?? ""}
        turnParty={board?.turn_party ?? null}
        inCheck={board?.in_check ?? false}
        myIndex={game.myIndex}
        busy={game.busy}
        interactive={!reading && game.phase === "open" && !state?.result && board?.turn_party === game.myIndex}
        selected={selected}
        promos={promos}
        now={now}
        allowance={thinkAllowance(game.arrangement?.options ?? genesisOptions(view))}
        onSquare={(square) => {
          if (!board || board.turn_party !== game.myIndex || state?.result) return;
          if (!selected) {
            setSelected(square);
            setPromos(null);
            return;
          }
          const ucis = board.legal_uci.filter((u) => u.startsWith(selected) && u.slice(2, 4) === square);
          if (ucis.length === 0) {
            setSelected(square);
            setPromos(null);
            return;
          }
          const promotions = ucis.filter((u) => u.length > 4);
          if (promotions.length > 1) {
            setPromos(promotions);
            return;
          }
          setSelected(null);
          setPromos(null);
          void game.move(ucis[0]);
        }}
        onPromote={(uci) => {
          setSelected(null);
          setPromos(null);
          void game.move(uci);
        }}
        onResign={() => void game.resign()}
        onOffer={() => void game.offerDraw()}
        onAccept={() => void game.acceptDraw()}
        onRecord={() => void game.recordResult()}
        onConfirmClose={(hash) => void game.confirm(hash)}
        onClaim={(partyIndex) => void game.claimTime(partyIndex)}
        onPgn={() => {
          if (!state) return;
          const thinks = (view?.committed ?? []).filter((l) => l.kind === "move").map((l) => l.think.ms);
          setPgn(chessPgn(state, thinks));
        }}
      />
      {game.decisions
        .filter((d) => d.action.candidate.kind !== "reveal" && d.action.candidate.kind !== "close")
        .map((d) => (
          <section className="card decision" key={d.action.candidate.hash}>
            <h2>Needs your answer</h2>
            <p>
              {partyLabel(game.parties, d.action.candidate.author, game.me)} proposes to {d.action.candidate.kind}{" "}
              the game.
            </p>
            <div className="row">
              <button onClick={() => void game.confirm(d.action.candidate.hash)} disabled={game.busy}>
                Agree
              </button>
              <button className="ghost" onClick={() => void game.reject(d.action.candidate.hash)} disabled={game.busy}>
                Refuse
              </button>
            </div>
          </section>
        ))}
      {pgn && (
        <section className="card">
          <h2>PGN</h2>
          <pre className="mono pgn">{pgn}</pre>
        </section>
      )}
      {game.error && <p className="error">{game.error}</p>}
    </main>
  );
}

function genesisOptions(view: ChainView<ChessState> | undefined): Record<string, unknown> | undefined {
  const body = view?.committed[0]?.body;
  if (!body || typeof body !== "object") return undefined;
  const options = (body as { options?: unknown }).options;
  return options && typeof options === "object" ? (options as Record<string, unknown>) : undefined;
}

function Play({
  view,
  state,
  boardFen,
  legal,
  sans,
  takenByWhite,
  takenByBlack,
  turnParty,
  inCheck,
  myIndex,
  busy,
  interactive,
  selected,
  promos,
  now,
  allowance,
  onSquare,
  onPromote,
  onResign,
  onOffer,
  onAccept,
  onRecord,
  onConfirmClose,
  onClaim,
  onPgn,
}: {
  view: ChainView<ChessState> | undefined;
  state: ChessState | undefined;
  boardFen: string | undefined;
  legal: string[];
  sans: string[];
  takenByWhite: string;
  takenByBlack: string;
  turnParty: number | null;
  inCheck: boolean;
  myIndex: number;
  busy: boolean;
  interactive: boolean;
  selected: string | null;
  promos: string[] | null;
  now: number;
  allowance: number | undefined;
  onSquare: (square: string) => void;
  onPromote: (uci: string) => void;
  onResign: () => void;
  onOffer: () => void;
  onAccept: () => void;
  onRecord: () => void;
  onConfirmClose: (hash: string) => void;
  onClaim: (party: number) => void;
  onPgn: () => void;
}) {
  const orientation: "white" | "black" =
    state && myIndex >= 0 && state.black === myIndex ? "black" : "white";
  const ready = view?.open?.ready_at.ms ?? null;
  const used = ready === null ? null : Math.max(0, now - ready);
  const timedOut = used !== null && allowance !== undefined && used > allowance && !state?.result;
  const watched = (view?.engaged.length ?? 0) > 0;
  const ended = view?.status.kind === "closed" || view?.status.kind === "abandoned";
  const open = ended ? undefined : view?.open;
  const pendingClose =
    open && !open.dead ? open.candidates.find((c) => c.kind === "close" && c.round === open.round) : undefined;

  return (
    <section className="card play">
      <div className="play-grid">
        {boardFen ? (
          <Board
            fen={boardFen}
            legal={interactive ? legal : []}
            orientation={orientation}
            interactive={interactive && !busy}
            selected={selected}
            onSquare={onSquare}
            takenByWhite={takenByWhite}
            takenByBlack={takenByBlack}
          />
        ) : (
          <p className="dim">Waiting for the colour to be drawn…</p>
        )}
        <div>
          <p>
            {outcome(state, view, myIndex, sans) ??
              (turnParty === null ? (
                <>Waiting for the position.</>
              ) : (
                <>
                  {turnParty === myIndex ? "Your move" : `${colourName(state, turnParty)} to move`}
                  {inCheck ? ", in check" : ""}.
                </>
              ))}
          </p>
          <Clock used={used} allowance={allowance} watched={watched} over={timedOut} ended={ended || Boolean(state?.result)} />
          {timedOut && turnParty !== null && turnParty !== myIndex && (
            <button onClick={() => onClaim(turnParty)} disabled={busy}>
              Record the time loss
            </button>
          )}
          {promos && (
            <div className="row">
              {PROMOTE.filter((p) => promos.some((u) => u.endsWith(p.piece))).map((p) => (
                <button key={p.piece} onClick={() => onPromote(promos.find((u) => u.endsWith(p.piece))!)} disabled={busy}>
                  {p.label}
                </button>
              ))}
            </div>
          )}
          {state && !state.result && myIndex >= 0 && (
            <div className="actions">
              <div className="row">
                <button onClick={onResign} disabled={busy}>
                  Resign
                </button>
                {state.draw_offer === null ? (
                  <button className="ghost" onClick={onOffer} disabled={busy}>
                    Offer draw
                  </button>
                ) : state.draw_offer !== myIndex ? (
                  <button onClick={onAccept} disabled={busy}>
                    Accept draw
                  </button>
                ) : (
                  <span className="dim">Draw offered. A move declines it.</span>
                )}
              </div>
              <p className="dim">Resign ends the game for you. The other player confirms that it happened.</p>
            </div>
          )}
          {state?.result && !ended &&
            (pendingClose ? (
              pendingClose.author === myIndex ? (
                <p className="dim">Waiting for the other player to confirm the result.</p>
              ) : (
                <button onClick={() => onConfirmClose(pendingClose.hash)} disabled={busy}>
                  Record the result
                </button>
              )
            ) : (
              <button onClick={onRecord} disabled={busy}>
                Record the result
              </button>
            ))}
          {state && state.moves.length > 0 && (
            <button className="small ghost" onClick={onPgn}>
              Show PGN
            </button>
          )}
        </div>
      </div>
      <ol className="moves">
        {history(view).map((link) => (
          <li key={link.hash}>
            {link.kind === "resign" ? (
              <ResignLine link={link} state={state} myIndex={myIndex} />
            ) : (
              <MoveLine san={sans[moveIndex(view, link)]} link={link} />
            )}
          </li>
        ))}
      </ol>
      {!watched && history(view).length > 0 && (
        <p className="dim">No watchman was named, so these moves are ordered and not timed.</p>
      )}
    </section>
  );
}

function colourName(state: ChessState | undefined, party: number): string {
  if (!state) return "The other player";
  if (state.white === party) return "White";
  if (state.black === party) return "Black";
  return "The other player";
}

/** Who resigned, a checkmate, or the score when the game ended some other way. */
function outcome(
  state: ChessState | undefined,
  view: ChainView<ChessState> | undefined,
  myIndex: number,
  sans: string[],
): string | null {
  const pending = view?.open?.candidates.find((c) => c.kind === "resign");
  if (pending) {
    if (pending.author === myIndex) return "You resigned. Waiting for the other player to confirm.";
    return `${colourName(state, pending.author)} resigned. Waiting for confirmation.`;
  }
  const resign = view?.committed.find((l) => l.kind === "resign");
  if (resign) {
    const who = resign.author === myIndex ? "You resigned" : `${colourName(state, resign.author)} resigned`;
    return state?.result ? `${who}. ${state.result}.` : `${who}.`;
  }
  if (state?.result === "1-0" || state?.result === "0-1") {
    if (sans.at(-1)?.endsWith("#")) {
      const whiteWins = state.result === "1-0";
      const winner = whiteWins ? state.white : state.black;
      const loser = whiteWins ? state.black : state.white;
      if (myIndex === winner) return `Checkmate. You win. ${state.result}.`;
      if (myIndex === loser) return `Checkmate. You lose. ${state.result}.`;
      return `Checkmate. ${whiteWins ? "White" : "Black"} wins. ${state.result}.`;
    }
  }
  return state?.result ? `Result ${state.result}.` : null;
}

function history(view: ChainView<ChessState> | undefined): LinkView[] {
  return (view?.committed ?? []).filter((l) => l.kind === "move" || l.kind === "resign");
}

function moveIndex(view: ChainView<ChessState> | undefined, link: LinkView): number {
  return (view?.committed ?? []).filter((l) => l.kind === "move" && l.seq <= link.seq).length - 1;
}

function Clock({
  used,
  allowance,
  watched,
  over,
  ended,
}: {
  used: number | null;
  allowance: number | undefined;
  watched: boolean;
  over: boolean;
  ended: boolean;
}) {
  if (ended) return null;
  if (!watched) return <p className="dim">This game has no watchman, so the move is not being timed.</p>;
  if (used === null) return <p className="dim">The watchman has not yet timed the start of this move.</p>;
  return (
    <p className={over ? "error" : undefined}>
      Time on this move {duration(used)}
      {allowance !== undefined ? ` of ${duration(allowance)}` : ""}.
    </p>
  );
}

function ResignLine({
  link,
  state,
  myIndex,
}: {
  link: LinkView;
  state: ChessState | undefined;
  myIndex: number;
}) {
  const who = link.author === myIndex ? "You resigned" : `${colourName(state, link.author)} resigned`;
  const time =
    link.think.ms !== null
      ? duration(link.think.ms)
      : link.think.split.length
        ? "watchmen disagree"
        : "not yet timed";
  return (
    <>
      {who} <span className="dim">{time}</span>
    </>
  );
}
function MoveLine({ san, link }: { san: string | undefined; link: LinkView }) {
  const label = san ?? (typeof link.body.uci === "string" ? link.body.uci : "move");
  const time =
    link.think.ms !== null
      ? duration(link.think.ms)
      : link.think.split.length
        ? "watchmen disagree"
        : "not yet timed";
  return (
    <>
      {label} <span className="dim">{time}</span>
    </>
  );
}
