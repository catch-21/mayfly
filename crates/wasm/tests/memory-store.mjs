// An in-memory Store shared between parties, the shape a browser app supplies over the Pubky
// SDK (see js/pubky-glue.js). Files are keyed `pubky://<owner><path>`.

export class Shared {
  constructor() {
    this.files = new Map();
  }
  /** Overwrite a file as if its owner did; for tests that tamper. */
  tamper(owner, path, mutate) {
    const key = `pubky://${owner}${path}`;
    const bytes = this.files.get(key);
    if (!bytes) throw new Error(`no such file ${key}`);
    this.files.set(key, mutate(Uint8Array.from(bytes)));
  }
  paths(owner) {
    const prefix = `pubky://${owner}`;
    return [...this.files.keys()].filter((k) => k.startsWith(prefix)).map((k) => k.slice(prefix.length));
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
      shared.files.set(`pubky://${me}${path}`, Uint8Array.from(bytes));
    },
    async delete(path) {
      shared.files.delete(`pubky://${me}${path}`);
    },
  };
}
