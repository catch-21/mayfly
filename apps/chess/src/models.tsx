// Staunton-style pieces turned on a lathe, plus a carved knight.
// Feet sit on y = 0. Uppercase letters are White.

import * as THREE from "three";

function lathe(profile: [number, number][]) {
  const geo = new THREE.LatheGeometry(
    profile.map(([radius, y]) => new THREE.Vector2(radius, y)),
    28,
  );
  geo.computeVertexNormals();
  return geo;
}

const PAWN = lathe([
  [0.001, 0],
  [0.18, 0],
  [0.2, 0.035],
  [0.15, 0.07],
  [0.08, 0.12],
  [0.05, 0.24],
  [0.075, 0.3],
  [0.042, 0.34],
  [0.08, 0.38],
  [0.115, 0.45],
  [0.08, 0.5],
  [0.001, 0.5],
]);

const ROOK = lathe([
  [0.001, 0],
  [0.2, 0],
  [0.22, 0.04],
  [0.16, 0.09],
  [0.12, 0.15],
  [0.115, 0.44],
  [0.15, 0.48],
  [0.15, 0.54],
  [0.09, 0.54],
  [0.001, 0.54],
]);

const KNIGHT_BASE = lathe([
  [0.001, 0],
  [0.2, 0],
  [0.22, 0.04],
  [0.16, 0.09],
  [0.09, 0.16],
  [0.08, 0.3],
  [0.1, 0.34],
  [0.001, 0.34],
]);

const BISHOP = lathe([
  [0.001, 0],
  [0.19, 0],
  [0.21, 0.04],
  [0.15, 0.09],
  [0.07, 0.16],
  [0.05, 0.4],
  [0.1, 0.46],
  [0.05, 0.5],
  [0.08, 0.6],
  [0.035, 0.7],
  [0.012, 0.76],
  [0.001, 0.76],
]);

const QUEEN = lathe([
  [0.001, 0],
  [0.2, 0],
  [0.22, 0.04],
  [0.15, 0.1],
  [0.065, 0.18],
  [0.048, 0.48],
  [0.11, 0.54],
  [0.055, 0.58],
  [0.09, 0.66],
  [0.045, 0.72],
  [0.001, 0.72],
]);

const KING = lathe([
  [0.001, 0],
  [0.2, 0],
  [0.22, 0.04],
  [0.15, 0.1],
  [0.07, 0.18],
  [0.052, 0.52],
  [0.12, 0.58],
  [0.06, 0.62],
  [0.075, 0.68],
  [0.035, 0.72],
  [0.001, 0.72],
]);

function Mat({ color }: { color: string }) {
  return <meshStandardMaterial color={color} roughness={0.38} metalness={0.08} />;
}

function Turned({ geo, color }: { geo: THREE.LatheGeometry; color: string }) {
  return (
    <mesh geometry={geo} castShadow>
      <Mat color={color} />
    </mesh>
  );
}

function Rook({ color }: { color: string }) {
  return (
    <group>
      <Turned geo={ROOK} color={color} />
      {[0, 1, 2, 3, 4, 5].map((i) => {
        const a = (i / 6) * Math.PI * 2;
        return (
          <mesh key={i} position={[Math.cos(a) * 0.12, 0.58, Math.sin(a) * 0.12]} castShadow>
            <boxGeometry args={[0.055, 0.08, 0.05]} />
            <Mat color={color} />
          </mesh>
        );
      })}
    </group>
  );
}

function Knight({ color }: { color: string }) {
  return (
    <group>
      <Turned geo={KNIGHT_BASE} color={color} />
      <group position={[0, 0.36, 0]}>
        <mesh position={[0, 0.08, 0.02]} rotation={[0.55, 0, 0]} castShadow>
          <cylinderGeometry args={[0.055, 0.085, 0.18, 12]} />
          <Mat color={color} />
        </mesh>
        <mesh position={[0, 0.2, 0.08]} rotation={[0.7, 0, 0]} scale={[0.72, 1.05, 1.35]} castShadow>
          <sphereGeometry args={[0.1, 16, 12]} />
          <Mat color={color} />
        </mesh>
        <mesh position={[0, 0.14, 0.18]} rotation={[1.15, 0, 0]} scale={[0.5, 1.1, 0.75]} castShadow>
          <sphereGeometry args={[0.07, 12, 10]} />
          <Mat color={color} />
        </mesh>
        <mesh position={[0.035, 0.3, 0.04]} rotation={[0.15, 0, 0.25]} castShadow>
          <coneGeometry args={[0.028, 0.08, 6]} />
          <Mat color={color} />
        </mesh>
        <mesh position={[0.05, 0.2, 0.12]}>
          <sphereGeometry args={[0.012, 8, 6]} />
          <meshStandardMaterial color="#1a140e" roughness={0.4} />
        </mesh>
      </group>
    </group>
  );
}

function Bishop({ color, slit }: { color: string; slit: string }) {
  return (
    <group>
      <Turned geo={BISHOP} color={color} />
      <mesh position={[0, 0.64, 0.045]}>
        <boxGeometry args={[0.014, 0.16, 0.02]} />
        <meshStandardMaterial color={slit} roughness={0.5} />
      </mesh>
    </group>
  );
}

function Queen({ color }: { color: string }) {
  return (
    <group>
      <Turned geo={QUEEN} color={color} />
      <mesh position={[0, 0.78, 0]} castShadow>
        <sphereGeometry args={[0.045, 16, 12]} />
        <Mat color={color} />
      </mesh>
    </group>
  );
}

function King({ color }: { color: string }) {
  return (
    <group>
      <Turned geo={KING} color={color} />
      <mesh position={[0, 0.8, 0]} castShadow>
        <boxGeometry args={[0.018, 0.14, 0.018]} />
        <Mat color={color} />
      </mesh>
      <mesh position={[0, 0.84, 0]} castShadow>
        <boxGeometry args={[0.09, 0.018, 0.018]} />
        <Mat color={color} />
      </mesh>
    </group>
  );
}

export function PieceModel({ letter }: { letter: string }) {
  const black = letter === letter.toLowerCase();
  const color = black ? "#2a241e" : "#f4efe6";
  const slit = black ? "#d5cdc2" : "#3a322c";
  const kind = letter.toUpperCase();
  const yaw = kind === "N" ? (black ? 0 : Math.PI) : 0;
  return (
    <group rotation={[0, yaw, 0]}>
      {kind === "P" && <Turned geo={PAWN} color={color} />}
      {kind === "R" && <Rook color={color} />}
      {kind === "N" && <Knight color={color} />}
      {kind === "B" && <Bishop color={color} slit={slit} />}
      {kind === "Q" && <Queen color={color} />}
      {kind === "K" && <King color={color} />}
    </group>
  );
}
