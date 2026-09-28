// A 3D board. Legal destinations come from the caller, which got them from shakmaty.
// Pieces on the squares are placed by the position. Captured pieces are Rapier bodies
// and fall into the tray on that player's side.

import { Suspense, useRef } from "react";
import { Canvas, useFrame } from "@react-three/fiber";
import { OrbitControls } from "@react-three/drei";
import { CuboidCollider, Physics, RigidBody } from "@react-three/rapier";
import * as THREE from "three";

import { PieceModel } from "./models";
import { PIECE_NAME } from "./pieces";

const HALF = 4;
const TILE = 0.96;
const TILE_TOP = 0.2;

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

function cell(square: string): [number, number] {
  const file = square.charCodeAt(0) - 97;
  const rank = Number(square[1]) - 1;
  return [file + 0.5 - HALF, HALF - (rank + 0.5)];
}

type RosterPiece = { id: number; letter: string; square: string };

function reconcile(prev: RosterPiece[], placed: Map<string, string>, ids: { n: number }): RosterPiece[] {
  const pool = [...prev];
  const out: RosterPiece[] = [];
  const pending: { square: string; letter: string }[] = [];
  for (const [square, letter] of placed) {
    const i = pool.findIndex((p) => p.square === square && p.letter === letter);
    if (i >= 0) {
      out.push(pool[i]);
      pool.splice(i, 1);
    } else pending.push({ square, letter });
  }
  for (const spot of pending) {
    let i = pool.findIndex((p) => p.letter === spot.letter);
    if (i < 0) {
      const pawn = spot.letter === spot.letter.toUpperCase() ? "P" : "p";
      if (pawn !== spot.letter) i = pool.findIndex((p) => p.letter === pawn);
    }
    if (i >= 0) {
      out.push({ ...pool[i], letter: spot.letter, square: spot.square });
      pool.splice(i, 1);
    } else {
      out.push({ id: ids.n++, letter: spot.letter, square: spot.square });
    }
  }
  return out;
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
  const roster = useRef<RosterPiece[]>([]);
  const ids = useRef({ n: 1 });
  roster.current = reconcile(roster.current, placed, ids.current);
  const targets = new Set(legal.filter((u) => selected && u.startsWith(selected)).map((u) => u.slice(2, 4)));
  const hover = useRef<HTMLParagraphElement>(null);

  const showHover = (letter: string | null) => {
    const el = hover.current;
    if (!el) return;
    el.textContent = letter ? (PIECE_NAME[letter] ?? letter) : "Plants are White. Zombies are Black.";
  };

  return (
    <div className="board-wrap">
      <div className={interactive ? "board-canvas live" : "board-canvas"} aria-label="Chess board">
        <Suspense fallback={<p className="dim board-wait">Setting the board…</p>}>
            <Canvas shadows camera={{ position: [0, 9.2, 12.2], fov: 30 }} dpr={[1, 1.75]}>
            <color attach="background" args={["#d7e8b4"]} />
            <ambientLight intensity={0.55} />
            <hemisphereLight args={["#eef6d4", "#8d6b45", 0.45]} />
            <directionalLight
              position={[6, 14, 8]}
              intensity={1.25}
              castShadow
              shadow-mapSize-width={1024}
              shadow-mapSize-height={1024}
              shadow-camera-near={2}
              shadow-camera-far={28}
              shadow-camera-left={-8}
              shadow-camera-right={8}
              shadow-camera-top={8}
              shadow-camera-bottom={-8}
            />
            <Physics gravity={[0, -18, 0]}>
              <group rotation={[0, orientation === "black" ? Math.PI : 0, 0]}>
                <Table />
                {"abcdefgh".split("").map((file) =>
                  [1, 2, 3, 4, 5, 6, 7, 8].map((rank) => {
                    const square = `${file}${rank}`;
                    return (
                      <Tile
                        key={square}
                        square={square}
                        dark={((file.charCodeAt(0) - 97 + rank) % 2) === 1}
                        selected={selected === square}
                        target={targets.has(square)}
                        interactive={interactive}
                        onSquare={onSquare}
                      />
                    );
                  }),
                )}
                {roster.current.map((piece) => (
                  <LivePiece
                    key={piece.id}
                    piece={piece}
                    selected={selected === piece.square}
                    interactive={interactive}
                    onSquare={onSquare}
                    onHover={showHover}
                  />
                ))}
                <Tray z={4.95} letters={takenByWhite} plant={false} />
                <Tray z={-4.95} letters={takenByBlack} plant />
              </group>
            </Physics>
            <OrbitControls
              makeDefault
              target={[0, 0, 0]}
              enablePan={false}
              minDistance={8}
              maxDistance={16}
              minPolarAngle={0.55}
              maxPolarAngle={1.2}
            />
          </Canvas>
        </Suspense>
      </div>
      <p className="piece-key dim" ref={hover}>
        Plants are White. Zombies are Black.
      </p>
    </div>
  );
}

function Table() {
  return (
    <>
      <mesh position={[0, -0.08, 0]} receiveShadow>
        <boxGeometry args={[11.2, 0.16, 12.4]} />
        <meshStandardMaterial color="#6f8f45" roughness={0.85} />
      </mesh>
      <mesh position={[0, 0.04, 0]} receiveShadow>
        <boxGeometry args={[8.35, 0.1, 8.35]} />
        <meshStandardMaterial color="#8d6848" roughness={0.75} />
      </mesh>
      <RigidBody type="fixed" colliders={false} position={[0, 0.08, 0]}>
        <CuboidCollider args={[4.05, 0.08, 4.05]} />
      </RigidBody>
    </>
  );
}

function Tile({
  square,
  dark,
  selected,
  target,
  interactive,
  onSquare,
}: {
  square: string;
  dark: boolean;
  selected: boolean;
  target: boolean;
  interactive: boolean;
  onSquare: (square: string) => void;
}) {
  const [x, z] = cell(square);
  return (
    <group position={[x, 0, z]}>
      <mesh
        position={[0, 0.12, 0]}
        receiveShadow
        onClick={(e) => {
          e.stopPropagation();
          if (interactive) onSquare(square);
        }}
      >
        <boxGeometry args={[TILE, 0.16, TILE]} />
        <meshStandardMaterial color={selected ? "#d7c48a" : dark ? "#b58863" : "#f0e6d2"} roughness={0.8} />
      </mesh>
      {target && (
        <mesh position={[0, TILE_TOP + 0.01, 0]} rotation={[-Math.PI / 2, 0, 0]}>
          <ringGeometry args={[0.16, 0.28, 20]} />
          <meshBasicMaterial color="#2f6f4f" />
        </mesh>
      )}
    </group>
  );
}

function LivePiece({
  piece,
  selected,
  interactive,
  onSquare,
  onHover,
}: {
  piece: RosterPiece;
  selected: boolean;
  interactive: boolean;
  onSquare: (square: string) => void;
  onHover: (letter: string | null) => void;
}) {
  const ref = useRef<THREE.Group>(null);
  const [x, z] = cell(piece.square);
  const shown = useRef(new THREE.Vector3(x, TILE_TOP, z));
  const goal = useRef(new THREE.Vector3(x, TILE_TOP, z));
  goal.current.set(x, TILE_TOP, z);
  useFrame((_, dt) => {
    const node = ref.current;
    if (!node) return;
    shown.current.lerp(goal.current, 1 - Math.exp(-8 * dt));
    node.position.copy(shown.current);
  });
  return (
    <group
      ref={ref}
      position={[x, TILE_TOP, z]}
      onClick={(e) => {
        e.stopPropagation();
        if (interactive) onSquare(piece.square);
      }}
      onPointerOver={(e) => {
        e.stopPropagation();
        onHover(piece.letter);
      }}
      onPointerOut={() => onHover(null)}
    >
      <PieceModel letter={piece.letter} />
      {selected && (
        <mesh position={[0, 0.02, 0]} rotation={[-Math.PI / 2, 0, 0]}>
          <ringGeometry args={[0.28, 0.38, 24]} />
          <meshBasicMaterial color="#2f6f4f" />
        </mesh>
      )}
    </group>
  );
}

function pile(letters: string, plant: boolean): { key: string; letter: string; position: [number, number, number] }[] {
  const seen: Record<string, number> = {};
  return [...letters].map((raw, i) => {
    const letter = plant ? raw.toUpperCase() : raw.toLowerCase();
    const n = seen[letter] ?? 0;
    seen[letter] = n + 1;
    const col = i % 5;
    const row = Math.floor(i / 5);
    return {
      key: `${letter}-${n}`,
      letter,
      position: [(col - 2) * 0.52, 1.15 + row * 0.5, (row % 2) * 0.12],
    };
  });
}

function Tray({ z, letters, plant }: { z: number; letters: string; plant: boolean }) {
  const fallen = pile(letters, plant);
  return (
    <group position={[0, 0, z]}>
      <mesh position={[0, 0.04, 0]} receiveShadow>
        <boxGeometry args={[3.2, 0.06, 0.9]} />
        <meshStandardMaterial color="#6f8f45" roughness={0.9} />
      </mesh>
      <RigidBody type="fixed" colliders={false}>
        <CuboidCollider args={[1.6, 0.04, 0.45]} position={[0, 0.04, 0]} />
        <CuboidCollider args={[1.6, 0.16, 0.04]} position={[0, 0.16, -0.45]} />
        <CuboidCollider args={[1.6, 0.16, 0.04]} position={[0, 0.16, 0.45]} />
        <CuboidCollider args={[0.04, 0.16, 0.45]} position={[-1.6, 0.16, 0]} />
        <CuboidCollider args={[0.04, 0.16, 0.45]} position={[1.6, 0.16, 0]} />
      </RigidBody>
      {fallen.map((item) => (
        <RigidBody
          key={item.key}
          position={item.position}
          colliders={false}
          restitution={0.2}
          friction={0.85}
          linearDamping={0.15}
          angularDamping={0.25}
        >
          <CuboidCollider args={[0.16, 0.22, 0.16]} position={[0, 0.22, 0]} />
          <PieceModel letter={item.letter} />
        </RigidBody>
      ))}
    </group>
  );
}
