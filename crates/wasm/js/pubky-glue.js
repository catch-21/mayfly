// Adapters from the Pubky SDK (`@synonymdev/pubky`) to the Store and Signer shapes the Mayfly
// module expects. Plain JavaScript on purpose: this is the whole seam between the two wasm
// modules, and it should be readable in one screen.

/**
 * A Store over a Pubky facade and, for writes, a session.
 *
 * Reads go through `pubky.publicStorage` (anyone's `/pub/`), so a store with no session is a
 * read-only verifier's store (`me === ""`).
 *
 * @param {import("@synonymdev/pubky").Pubky} pubky
 * @param {import("@synonymdev/pubky").Session} [session]
 */
export function storeFromPubky(pubky, session) {
  const pub = pubky.publicStorage;
  const me = session ? session.info.publicKey.z32() : "";
  return {
    me,
    async list(owner, prefix) {
      const address = `pubky://${owner}${prefix}`;
      const out = [];
      let cursor = null;
      for (;;) {
        // list(address, cursor, reverse, limit, shallow) — one page at a time.
        const page = await pub.list(address, cursor, false, 1000, false);
        for (const link of page) {
          const path = link.replace(/^pubky:\/\/[^/]+/, "");
          out.push({ path });
        }
        if (page.length < 1000) break;
        cursor = page[page.length - 1];
      }
      // Content hashes come from HEAD requests; done concurrently in small batches so a
      // tampered mirror can be told from a fresh copy without fetching every file.
      for (let i = 0; i < out.length; i += 8) {
        await Promise.all(
          out.slice(i, i + 8).map(async (entry) => {
            try {
              const stats = await pub.stats(`pubky://${owner}${entry.path}`);
              if (stats && stats.etag) entry.contentHash = stats.etag;
            } catch {
              // Absent or racing; the client fetches and hashes instead.
            }
          }),
        );
      }
      return out;
    },
    async get(owner, path) {
      try {
        return await pub.getBytes(`pubky://${owner}${path}`);
      } catch (e) {
        if (e && (e.name === "RequestError") && e.data && e.data.statusCode === 404) return undefined;
        if (/404/.test(String(e && e.message))) return undefined;
        throw e;
      }
    },
    async put(path, bytes) {
      if (!session) throw new Error("read-only store");
      await session.storage.putBytes(path, bytes);
    },
    async delete(path) {
      if (!session) throw new Error("read-only store");
      await session.storage.delete(path);
    },
  };
}

/**
 * A Signer over a grant-backed Pubky session.
 *
 * @param {import("@synonymdev/pubky").Session} session
 */
export async function signerFromSession(session) {
  const grant = session.grant;
  if (!grant) throw new Error("Mayfly needs a grant-backed session (not a cookie session)");
  const client = await grant.clientPublicKey();
  return {
    pubky: session.info.publicKey.z32(),
    kid: client.z32(),
    clientId: session.info.clientId,
    grantJws: await grant.grantJws(),
    signJws: (typ, claims) => grant.signJws(typ, claims),
  };
}
