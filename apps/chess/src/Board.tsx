// An 8×8 board. Legal destinations come from the caller, which got them from shakmaty.
// This component does not decide whether a move is legal.

const GLYPH: Record<string, string> = {
  K: "♔",
  Q: "♕",
  R: "♖",
  B: "♗",
  N: "♘",
  P: "♙",
  k: "♚",
  q: "♛",
  r: "♜",
  b: "♝",
  n: "♞",
  p: "♟",
};

export function squaresFromFen(fen: string): Map<string, string> {
  const map = new Map<string, string>();
  const rows = fen.split(" ")[0]?.split("/") ?? [];
  rows.forEach((row, rankIndex) => {
    let file = 0;
    for (const ch of row) {
      const n = Number(ch);
      if (n >= 1 && n <= 8) {
        file += n;
        continue;
      }
      const square = `${"abcdefgh"[file]}${8 - rankIndex}`;
      map.set(square, ch);
      file += 1;
    }
  });
  return map;
}

export function Board({
  fen,
  legal,
  orientation,
  interactive,
  selected,
  onSquare,
  takenByWhite,
  takenByBlack,
}: {
  fen: string;
  legal: string[];
  orientation: "white" | "black";
  interactive: boolean;
  selected: string | null;
  onSquare: (square: string) => void;
  /** Pieces White has taken, as `QRNBP` letters. */
  takenByWhite: string;
  /** Pieces Black has taken, as `QRNBP` letters. */
  takenByBlack: string;
}) {
  const placed = squaresFromFen(fen);
  const ranks = orientation === "white" ? [8, 7, 6, 5, 4, 3, 2, 1] : [1, 2, 3, 4, 5, 6, 7, 8];
  const files = orientation === "white" ? "abcdefgh" : "hgfedcba";
  const targets = new Set(
    legal.filter((u) => selected && u.startsWith(selected)).map((u) => u.slice(2, 4)),
  );
  const top = orientation === "white" ? takenByBlack : takenByWhite;
  const bottom = orientation === "white" ? takenByWhite : takenByBlack;
  const topColour = orientation === "white" ? "Black" : "White";
  const bottomColour = orientation === "white" ? "White" : "Black";

  return (
    <div className="board-wrap">
      <Taken letters={top} asWhite={orientation === "white"} who={topColour} />
      <div className={`board ${orientation}`} role="grid" aria-label="Chess board">
        {ranks.map((rank) =>
          [...files].map((file) => {
            const square = `${file}${rank}`;
            const piece = placed.get(square);
            const fileNumber = file.charCodeAt(0) - "a".charCodeAt(0);
            const dark = (fileNumber + rank) % 2 === 1;
            const classes = [
              "square",
              dark ? "dark" : "light",
              selected === square ? "selected" : "",
              targets.has(square) ? "target" : "",
            ]
              .filter(Boolean)
              .join(" ");
            return (
              <button
                key={square}
                type="button"
                className={classes}
                disabled={!interactive}
                aria-label={piece ? `${square} ${piece}` : square}
                onClick={() => onSquare(square)}
              >
                {piece ? GLYPH[piece] ?? piece : targets.has(square) ? "·" : ""}
              </button>
            );
          }),
        )}
      </div>
      <Taken letters={bottom} asWhite={orientation === "black"} who={bottomColour} />
    </div>
  );
}

/** Pieces `who` has taken. `asWhite` draws them as White's pieces when the taker is Black. */
function Taken({ letters, asWhite, who }: { letters: string; asWhite: boolean; who: string }) {
  if (!letters) return <div className="taken" aria-hidden="true" />;
  return (
    <div className="taken" aria-label={`Taken by ${who}`}>
      {[...letters].map((letter, i) => {
        const glyph = GLYPH[asWhite ? letter : letter.toLowerCase()] ?? letter;
        return (
          <span key={`${letter}-${i}`} className="taken-piece">
            {glyph}
          </span>
        );
      })}
    </div>
  );
}
