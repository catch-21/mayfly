// Original 3D plants and zombies, built from simple shapes. Feet sit on y = 0.

import * as THREE from "three";

const INK = "#24180f";

function Solid({ color, roughness = 0.58, metalness = 0.04 }: { color: string; roughness?: number; metalness?: number }) {
  return <meshStandardMaterial color={color} roughness={roughness} metalness={metalness} />;
}

function Eyes({ y, z, spread, size = 0.038 }: { y: number; z: number; spread: number; size?: number }) {
  return (
    <group position={[0, y, z]}>
      {[-spread, spread].map((x) => (
        <group key={x} position={[x, 0, 0]}>
          <mesh>
            <sphereGeometry args={[size, 10, 8]} />
            <Solid color="#f6f1e6" roughness={0.35} />
          </mesh>
          <mesh position={[0, 0, size * 0.55]}>
            <sphereGeometry args={[size * 0.42, 8, 8]} />
            <Solid color={INK} roughness={0.4} />
          </mesh>
        </group>
      ))}
    </group>
  );
}

function Leaves() {
  return (
    <>
      <mesh position={[-0.12, 0.06, 0]} rotation={[0.2, 0, 0.9]} scale={[1.5, 0.28, 0.7]}>
        <sphereGeometry args={[0.1, 10, 8]} />
        <Solid color="#3c9a32" />
      </mesh>
      <mesh position={[0.12, 0.06, 0]} rotation={[0.2, 0, -0.9]} scale={[1.5, 0.28, 0.7]}>
        <sphereGeometry args={[0.1, 10, 8]} />
        <Solid color="#2f7d28" />
      </mesh>
    </>
  );
}

function Peashooter() {
  return (
    <group>
      <Leaves />
      <mesh position={[0, 0.2, 0]}>
        <cylinderGeometry args={[0.035, 0.05, 0.22, 8]} />
        <Solid color="#2f7d28" />
      </mesh>
      <mesh position={[-0.02, 0.38, 0]} castShadow>
        <sphereGeometry args={[0.16, 18, 14]} />
        <Solid color="#6fce4e" />
      </mesh>
      <Eyes y={0.4} z={0.12} spread={0.06} />
      <mesh position={[0.16, 0.38, 0]} rotation={[0, 0, Math.PI / 2]} castShadow>
        <cylinderGeometry args={[0.045, 0.06, 0.18, 10]} />
        <Solid color="#257024" />
      </mesh>
      <mesh position={[0.3, 0.38, 0]} castShadow>
        <sphereGeometry args={[0.055, 12, 10]} />
        <Solid color="#c6ef55" />
      </mesh>
    </group>
  );
}

function Chomper() {
  return (
    <group>
      <Leaves />
      <mesh position={[0, 0.18, 0]}>
        <cylinderGeometry args={[0.04, 0.05, 0.16, 8]} />
        <Solid color="#2f7d28" />
      </mesh>
      <mesh position={[0, 0.38, 0.02]} castShadow>
        <sphereGeometry args={[0.2, 18, 14]} />
        <Solid color="#d23d86" />
      </mesh>
      <mesh position={[0, 0.32, 0.14]}>
        <sphereGeometry args={[0.09, 12, 10]} />
        <Solid color="#4a1830" roughness={0.7} />
      </mesh>
      {[-0.05, 0, 0.05].map((x) => (
        <mesh key={x} position={[x, 0.36, 0.16]}>
          <boxGeometry args={[0.028, 0.05, 0.02]} />
          <Solid color="#fff8ee" roughness={0.35} />
        </mesh>
      ))}
      <Eyes y={0.48} z={0.12} spread={0.08} />
    </group>
  );
}

function starGeometry() {
  const shape = new THREE.Shape();
  const spikes = 5;
  for (let i = 0; i < spikes * 2; i++) {
    const r = i % 2 === 0 ? 0.22 : 0.09;
    const a = -Math.PI / 2 + (i * Math.PI) / spikes;
    const x = Math.cos(a) * r;
    const y = Math.sin(a) * r;
    if (i === 0) shape.moveTo(x, y);
    else shape.lineTo(x, y);
  }
  shape.closePath();
  const geo = new THREE.ExtrudeGeometry(shape, {
    depth: 0.1,
    bevelEnabled: true,
    bevelThickness: 0.02,
    bevelSize: 0.015,
    bevelSegments: 1,
  });
  geo.translate(0, 0, -0.05);
  return geo;
}

const STAR = starGeometry();

function Starfruit() {
  return (
    <group>
      <mesh position={[0, 0.05, 0]} rotation={[0.4, 0, 0]} scale={[1.2, 0.25, 0.6]}>
        <sphereGeometry args={[0.1, 10, 8]} />
        <Solid color="#3c9a32" />
      </mesh>
      <mesh geometry={STAR} position={[0, 0.32, 0]} castShadow>
        <Solid color="#ffe14a" />
      </mesh>
      <Eyes y={0.34} z={0.08} spread={0.05} size={0.028} />
    </group>
  );
}

function WallNut() {
  return (
    <group>
      <mesh position={[0, 0.28, 0]} scale={[1, 1.15, 0.9]} castShadow>
        <sphereGeometry args={[0.22, 18, 14]} />
        <Solid color="#e0a45a" />
      </mesh>
      <Eyes y={0.32} z={0.16} spread={0.07} />
      <mesh position={[0, 0.22, 0.18]} rotation={[0.4, 0, 0]}>
        <torusGeometry args={[0.05, 0.012, 6, 10, Math.PI]} />
        <Solid color={INK} roughness={0.5} />
      </mesh>
    </group>
  );
}

function MelonPult() {
  return (
    <group>
      <mesh position={[0, 0.08, 0]} rotation={[0.5, 0, 0]} scale={[1.6, 0.35, 1]}>
        <sphereGeometry args={[0.16, 12, 8]} />
        <Solid color="#4aaa3c" />
      </mesh>
      <mesh position={[0, 0.36, 0]} castShadow>
        <sphereGeometry args={[0.2, 20, 16]} />
        <Solid color="#3e9a46" />
      </mesh>
      <mesh position={[0, 0.36, 0]} rotation={[0.4, 0.2, 0]}>
        <torusGeometry args={[0.14, 0.018, 8, 18]} />
        <Solid color="#1d5c2c" />
      </mesh>
      <Eyes y={0.4} z={0.16} spread={0.07} />
    </group>
  );
}

function CrazyDave() {
  return (
    <group>
      <mesh position={[-0.06, 0.1, 0]}>
        <cylinderGeometry args={[0.035, 0.04, 0.2, 8]} />
        <Solid color="#5c4632" />
      </mesh>
      <mesh position={[0.06, 0.1, 0]}>
        <cylinderGeometry args={[0.035, 0.04, 0.2, 8]} />
        <Solid color="#5c4632" />
      </mesh>
      <mesh position={[0, 0.28, 0]} castShadow>
        <boxGeometry args={[0.22, 0.2, 0.14]} />
        <Solid color="#8a5a32" />
      </mesh>
      <mesh position={[0, 0.3, 0.06]}>
        <sphereGeometry args={[0.09, 12, 10]} />
        <Solid color="#6b4224" />
      </mesh>
      <mesh position={[0, 0.46, 0]} castShadow>
        <sphereGeometry args={[0.13, 16, 12]} />
        <Solid color="#f0c09a" />
      </mesh>
      <Eyes y={0.48} z={0.1} spread={0.05} size={0.032} />
      <mesh position={[0, 0.6, 0]} castShadow>
        <cylinderGeometry args={[0.12, 0.14, 0.12, 12]} />
        <Solid color="#d5dbe3" metalness={0.35} roughness={0.35} />
      </mesh>
      <mesh position={[0, 0.54, 0]} rotation={[Math.PI / 2, 0, 0]}>
        <torusGeometry args={[0.15, 0.02, 8, 16]} />
        <Solid color="#9aa3ad" metalness={0.4} roughness={0.35} />
      </mesh>
      <mesh position={[0.16, 0.6, 0]} rotation={[0, 0, Math.PI / 2]}>
        <torusGeometry args={[0.04, 0.012, 6, 10]} />
        <Solid color="#8e98a6" metalness={0.4} roughness={0.35} />
      </mesh>
    </group>
  );
}

function Zombie({ hat }: { hat?: "cone" | "bucket" }) {
  return (
    <group>
      <mesh position={[-0.05, 0.08, 0]}>
        <cylinderGeometry args={[0.03, 0.035, 0.16, 8]} />
        <Solid color="#3e4654" />
      </mesh>
      <mesh position={[0.05, 0.08, 0]}>
        <cylinderGeometry args={[0.03, 0.035, 0.16, 8]} />
        <Solid color="#3e4654" />
      </mesh>
      <mesh position={[0, 0.24, 0]} castShadow>
        <boxGeometry args={[0.18, 0.18, 0.1]} />
        <Solid color="#c47b3a" />
      </mesh>
      <mesh position={[0, 0.22, 0.04]}>
        <coneGeometry args={[0.03, 0.08, 6]} />
        <Solid color="#d63c32" />
      </mesh>
      <mesh position={[-0.14, 0.26, 0]} rotation={[0, 0, 0.8]}>
        <cylinderGeometry args={[0.025, 0.025, 0.16, 6]} />
        <Solid color="#c5dc8a" />
      </mesh>
      <mesh position={[0.14, 0.26, 0]} rotation={[0, 0, -0.8]}>
        <cylinderGeometry args={[0.025, 0.025, 0.16, 6]} />
        <Solid color="#c5dc8a" />
      </mesh>
      <mesh position={[0, 0.42, 0]} castShadow>
        <sphereGeometry args={[0.11, 16, 12]} />
        <Solid color="#c5dc8a" />
      </mesh>
      <Eyes y={0.44} z={0.08} spread={0.04} size={0.028} />
      {hat === "cone" && (
        <mesh position={[0, 0.58, 0]} castShadow>
          <coneGeometry args={[0.1, 0.2, 10]} />
          <Solid color="#ef8b22" />
        </mesh>
      )}
      {hat === "bucket" && (
        <mesh position={[0, 0.56, 0]} castShadow>
          <cylinderGeometry args={[0.1, 0.12, 0.14, 10]} />
          <Solid color="#d5dbe3" metalness={0.45} roughness={0.3} />
        </mesh>
      )}
    </group>
  );
}

function PoleVaulter() {
  return (
    <group rotation={[0, 0, -0.35]}>
      <Zombie />
      <mesh position={[-0.05, 0.34, 0.08]} rotation={[0, 0, 0.9]} castShadow>
        <cylinderGeometry args={[0.02, 0.02, 0.7, 8]} />
        <Solid color="#3a78d0" />
      </mesh>
    </group>
  );
}

function Football() {
  return (
    <group>
      <mesh position={[-0.06, 0.1, 0]}>
        <cylinderGeometry args={[0.04, 0.045, 0.2, 8]} />
        <Solid color="#3e4654" />
      </mesh>
      <mesh position={[0.06, 0.1, 0]}>
        <cylinderGeometry args={[0.04, 0.045, 0.2, 8]} />
        <Solid color="#3e4654" />
      </mesh>
      <mesh position={[0, 0.28, 0]} castShadow>
        <boxGeometry args={[0.26, 0.2, 0.14]} />
        <Solid color="#9a2c2c" />
      </mesh>
      <mesh position={[-0.16, 0.32, 0]}>
        <sphereGeometry args={[0.07, 10, 8]} />
        <Solid color="#7a2424" />
      </mesh>
      <mesh position={[0.16, 0.32, 0]}>
        <sphereGeometry args={[0.07, 10, 8]} />
        <Solid color="#7a2424" />
      </mesh>
      <mesh position={[0, 0.48, 0]} scale={[1.1, 0.9, 1]} castShadow>
        <sphereGeometry args={[0.13, 16, 12]} />
        <Solid color="#f4f0e6" roughness={0.4} />
      </mesh>
      <mesh position={[0, 0.5, 0.1]}>
        <boxGeometry args={[0.16, 0.02, 0.02]} />
        <Solid color={INK} />
      </mesh>
      <Eyes y={0.48} z={0.1} spread={0.05} size={0.026} />
    </group>
  );
}

function Zomboss() {
  return (
    <group>
      <mesh position={[-0.05, 0.08, 0]}>
        <cylinderGeometry args={[0.03, 0.035, 0.16, 8]} />
        <Solid color="#3e4654" />
      </mesh>
      <mesh position={[0.05, 0.08, 0]}>
        <cylinderGeometry args={[0.03, 0.035, 0.16, 8]} />
        <Solid color="#3e4654" />
      </mesh>
      <mesh position={[0, 0.24, 0]} castShadow>
        <boxGeometry args={[0.22, 0.2, 0.12]} />
        <Solid color="#f4f7f8" />
      </mesh>
      <mesh position={[0, 0.24, 0.04]}>
        <boxGeometry args={[0.04, 0.12, 0.02]} />
        <Solid color={INK} />
      </mesh>
      <mesh position={[0, 0.5, 0]} scale={[1.15, 1, 1]} castShadow>
        <sphereGeometry args={[0.2, 20, 16]} />
        <Solid color="#b7d48a" />
      </mesh>
      <Eyes y={0.5} z={0.16} spread={0.08} />
      {[-0.08, 0.08].map((x) => (
        <mesh key={x} position={[x, 0.5, 0.14]} rotation={[Math.PI / 2, 0, 0]}>
          <torusGeometry args={[0.045, 0.008, 6, 12]} />
          <Solid color={INK} metalness={0.2} roughness={0.4} />
        </mesh>
      ))}
      <mesh position={[0, 0.5, 0.14]} rotation={[0, 0, Math.PI / 2]}>
        <cylinderGeometry args={[0.008, 0.008, 0.07, 6]} />
        <Solid color={INK} />
      </mesh>
    </group>
  );
}

const SCALE: Record<string, number> = {
  P: 0.92,
  p: 0.92,
  Q: 1.05,
  q: 1.06,
  K: 1.02,
  k: 1.02,
};

export function PieceModel({ letter }: { letter: string }) {
  const scale = SCALE[letter] ?? 1;
  return (
    <group scale={scale}>
      {body(letter)}
    </group>
  );
}

function body(letter: string) {
  switch (letter) {
    case "P":
      return <Peashooter />;
    case "N":
      return <Chomper />;
    case "B":
      return <Starfruit />;
    case "R":
      return <WallNut />;
    case "Q":
      return <MelonPult />;
    case "K":
      return <CrazyDave />;
    case "p":
      return <Zombie />;
    case "b":
      return <Zombie hat="cone" />;
    case "r":
      return <Zombie hat="bucket" />;
    case "n":
      return <PoleVaulter />;
    case "q":
      return <Football />;
    case "k":
      return <Zomboss />;
    default:
      return null;
  }
}
