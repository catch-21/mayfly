// Following a chain as anyone: the module's reader lists, decodes and verifies (§9, §14);
// this file polls it while the chain is open and stops once it is final.

import { describeError } from "./errors.js";
import { loadWith, WasmReader, type Store } from "./mayfly.js";
import type { RulesRegistry } from "./rules.js";
import type { Loaded, LoadedFile } from "./types.js";

export const DEFAULT_POLL_MS = 4_000;

/** A stable key for one listed file. */
export function fileKey(f: LoadedFile): string {
  return `${f.folder.owner}${f.path}`;
}

/** Load a chain once, as anyone. `registry` says which rules can be run. */
export function loadChain<S = unknown>(store: Store, registry: RulesRegistry, url: string): Promise<Loaded<S>> {
  return loadWith<S>(new WasmReader(store, (id: string) => registry.resolve(id)), url);
}

export interface ReaderState<S = unknown> {
  url: string;
  loaded: (Loaded<S> & { loadedAt: Date }) | undefined;
  /** Why the chain could not be loaded at all (a bad link). */
  error: string | undefined;
  busy: boolean;
  /** Polling has stopped because the chain is final. */
  final: boolean;
}

export interface ChainReaderOptions {
  store: Store;
  registry: RulesRegistry;
  url: string;
  pollMs?: number;
}

/**
 * Follow a chain as a bystander: load it, poll while it is open or has no view yet, and stop
 * once the verifier says it is final. One module reader per `ChainReader`, so a poll
 * refetches only files whose content hash changed.
 */
export class ChainReader<S = unknown> {
  private listeners = new Set<(s: ReaderState<S>) => void>();
  private timer: ReturnType<typeof setInterval> | undefined;
  private inFlight = false;
  private stopped = false;
  private _state: ReaderState<S>;
  private readonly pollMs: number;
  private readonly reader: WasmReader;

  constructor(private readonly options: ChainReaderOptions) {
    this.pollMs = options.pollMs ?? DEFAULT_POLL_MS;
    this.reader = new WasmReader(options.store, (id: string) => options.registry.resolve(id));
    this._state = { url: options.url, loaded: undefined, error: undefined, busy: false, final: false };
  }

  get state(): ReaderState<S> {
    return this._state;
  }

  subscribe(listener: (s: ReaderState<S>) => void): () => void {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  }

  /** Load now and poll until the chain is final. `refresh()` alone works without this. */
  start(): void {
    if (this.timer) return;
    this.stopped = false;
    void this.refresh();
    this.timer = setInterval(() => {
      if (!this._state.final) void this.refresh();
    }, this.pollMs);
  }

  stop(): void {
    this.stopped = true;
    if (this.timer) clearInterval(this.timer);
    this.timer = undefined;
  }

  async refresh(): Promise<void> {
    if (this.inFlight) return;
    this.inFlight = true;
    this.set({ busy: true });
    try {
      const loaded = await loadWith<S>(this.reader, this.options.url);
      if (this.stopped) return;
      this.set({ loaded: { ...loaded, loadedAt: new Date() }, error: undefined, final: loaded.view?.is_final ?? false });
    } catch (e) {
      if (!this.stopped) this.set({ error: describeError(e) });
    } finally {
      this.inFlight = false;
      this.set({ busy: false });
    }
  }

  private set(patch: Partial<ReaderState<S>>): void {
    this._state = { ...this._state, ...patch };
    for (const l of this.listeners) l(this._state);
  }
}
