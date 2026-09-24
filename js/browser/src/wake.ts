// Live updates from a homeserver: one SSE stream per (owner, folder), through the Pubky SDK.

import { PublicKey, type Pubky } from "@synonymdev/pubky";

import type { Wake } from "./session.js";

/**
 * A `Wake` over `pubky.eventStreamForUser(...)`. Each subscription holds one HTTP connection
 * open; the caller opens few and unsubscribes promptly. A stream that fails to open (an old
 * homeserver, a network blip) is silent: the session's timer still drives it.
 */
export function pubkyWake(pubky: Pubky): Wake {
  return {
    subscribe(owner, path, onEvent) {
      let stopped = false;
      let reader: ReadableStreamDefaultReader<unknown> | undefined;
      void (async () => {
        try {
          const stream = await pubky.eventStreamForUser(PublicKey.from(owner), null).live().path(path).subscribe();
          const r = stream.getReader();
          if (stopped) {
            // Torn down while the subscription was opening: release the connection now.
            void r.cancel().catch(() => undefined);
            return;
          }
          reader = r;
          for (;;) {
            const { done } = await r.read();
            if (done || stopped) break;
            onEvent();
          }
        } catch {
          // No stream; the timer covers it.
        }
      })();
      return () => {
        stopped = true;
        if (reader) void reader.cancel().catch(() => undefined);
      };
    },
  };
}
