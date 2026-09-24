// An in-memory Store shared between parties, the shape a browser app supplies over the Pubky
// SDK, plus a Wake that fires when anyone writes: the tests' stand-in for homeserver events.

export class Shared {
  constructor() {
    this.files = new Map();
    this.watchers = new Set();
  }
  paths(owner) {
    const prefix = `pubky://${owner}`;
    return [...this.files.keys()].filter((k) => k.startsWith(prefix)).map((k) => k.slice(prefix.length));
  }
  /** A Wake over this store: every write to a watched folder fires its subscribers. */
  wake() {
    return {
      subscribe: (owner, path, onEvent) => {
        const w = { owner, path, onEvent };
        this.watchers.add(w);
        return () => this.watchers.delete(w);
      },
    };
  }
  fire(key) {
    for (const w of this.watchers) {
      if (key.startsWith(`pubky://${w.owner}${w.path}`)) queueMicrotask(w.onEvent);
    }
  }
}

export function memoryStore(me, shared) {
  return {
    me,
    async list(owner, prefix) {
      const want = `pubky://${owner}${prefix}`;
      const out = [];
      for (const key of shared.files.keys()) {
        if (key.startsWith(want)) out.push({ path: key.slice(`pubky://${owner}`.length) });
      }
      out.sort((a, b) => (a.path < b.path ? -1 : a.path > b.path ? 1 : 0));
      return out;
    },
    async get(owner, path) {
      const bytes = shared.files.get(`pubky://${owner}${path}`);
      return bytes ? Uint8Array.from(bytes) : undefined;
    },
    async put(path, bytes) {
      const key = `pubky://${me}${path}`;
      shared.files.set(key, Uint8Array.from(bytes));
      shared.fire(key);
    },
    async delete(path) {
      const key = `pubky://${me}${path}`;
      shared.files.delete(key);
      shared.fire(key);
    },
  };
}
