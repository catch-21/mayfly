// Rules: the ones the wasm module ships, named by id, and the ones an app writes in
// JavaScript and passes as an object (§10). A registry maps every id a chain may name to one
// or the other, so the reader can verify a chain whose genesis names an app's own rules.

import type { Body } from "./types.js";

/** `{ summary, winners? }`: what a chain ended with. */
export interface Outcome {
  summary: string;
  winners?: number[];
}

/**
 * Rules written in JavaScript. Every method is synchronous and must be a pure function of its
 * arguments: the verifier runs `apply` on every party's machine and expects identical
 * `canonicalState` bytes (§10). Throw to refuse.
 */
export interface RulesModule<S = unknown, B extends Body = Body> {
  /** Pinned in genesis, e.g. `"chess/1"`. */
  id: string;
  /** Pinned in genesis (§6.6); a placeholder is fine until release. */
  referenceHash: string;
  /** The initial state once genesis is committed. `genesis.parties[i].pubky` is party `i`. */
  init(genesis: GenesisLike, confirmations: unknown[], nonces: Uint8Array[]): S;
  /** Commit-reveal nonces before `init`; default false. */
  wantsReveals?(genesis: GenesisLike): boolean;
  /** Whose clock runs (§11.3); default nobody. */
  obliged?(state: S): number[];
  /** May `party` propose a link of `kind` now? */
  mayAppend(state: S, party: number, kind: string): boolean;
  /** The pure transition. `link.author` is a pubky; `link.body` is `B` without its `kind`. */
  apply(state: S, link: LinkLike<B>): S;
  /** A finished outcome, or `null`/`undefined` while ongoing. Default ongoing. */
  status?(state: S): Outcome | null | undefined;
  /** Outcome recorded by a close (§6.8); throw when the close is not valid now. */
  close(state: S, close: CloseLike): Outcome;
  /** Deterministic bytes for hashing `state`; default JSON with sorted keys. */
  canonicalState?(state: S): Uint8Array | string;
  /** Type witness only; never read. */
  readonly __body?: B;
}

/** The genesis as `init` sees it (the fields an app usually needs). */
export interface GenesisLike {
  rules: string;
  parties: { pubky: string; kid: string; [key: string]: unknown }[];
  witnesses: { pubky: string; [key: string]: unknown }[];
  confirm_quorum: number;
  options: Record<string, unknown>;
  [key: string]: unknown;
}

/** A link as `apply` sees it. */
export interface LinkLike<B extends Body = Body> {
  seq: number;
  round: number;
  author: string;
  kind: string;
  body: Omit<B, "kind"> & Record<string, unknown>;
  ts: number;
  [key: string]: unknown;
}

/** A close body as `close` sees it. */
export interface CloseLike {
  reason: "agreed" | "finished" | "abandoned";
  subject: string[];
  pending: string[];
}

/** What the wasm module accepts: a shipped id, or a rules object. */
export type RulesRef = string | RulesModule<any, any>;

/** Every rules id an app can open: shipped ids and its own modules. */
export class RulesRegistry {
  private readonly modules = new Map<string, RulesModule<any, any>>();

  constructor(private readonly shipped: readonly string[], modules: RulesModule<any, any>[] = []) {
    for (const m of modules) this.add(m);
  }

  /** Register a module; its `id` may shadow a shipped id on purpose. */
  add(module: RulesModule<any, any>): this {
    this.modules.set(module.id, module);
    return this;
  }

  /** Every id this registry can run. */
  ids(): string[] {
    return [...new Set([...this.modules.keys(), ...this.shipped])];
  }

  has(id: string): boolean {
    return this.modules.has(id) || this.shipped.includes(id);
  }

  /** What to hand the wasm module for `id`, or `undefined` if nothing here runs it. */
  resolve(id: string): RulesRef | undefined {
    const m = this.modules.get(id);
    if (m) return m;
    return this.shipped.includes(id) ? id : undefined;
  }
}

/** The id of a rules reference. */
export function rulesId(rules: RulesRef): string {
  return typeof rules === "string" ? rules : rules.id;
}
