import { useCallback, useEffect, useRef, useState } from "react";

import { IS_TESTNET, loadChain, normaliseChainUrl, type Loaded } from "./mayfly";
import { describeError, formatClock } from "./format";
import { Anomalies } from "./components/Anomalies";
import { Files, fileKey } from "./components/Files";
import { Header } from "./components/Header";
import { RecordPanel } from "./components/RecordPanel";
import { RulesState } from "./components/RulesState";
import { Timeline } from "./components/Timeline";

const POLL_MS = 4000;

const EXAMPLES = [
  "pubky://<owner>/pub/<client_id>/mayfly/chains/<CHAIN_ID>/",
  "pubky://<owner>/pub/<client_id>/mayfly/chains/<CHAIN_ID>/links/00000000-<h16>.jws",
  "pubky://<owner>/pub/<client_id>/mayfly/chains/<CHAIN_ID>/confirms/00000003-<h16>-<kid>.jws",
];

function hashUrl(): string {
  const h = decodeURIComponent(window.location.hash.replace(/^#/, ""));
  return h.startsWith("pubky://") ? h : "";
}

function LinkForm({
  initial,
  onSubmit,
  busy,
  compact,
}: {
  initial: string;
  onSubmit: (url: string) => void;
  busy: boolean;
  compact?: boolean;
}) {
  const [value, setValue] = useState(initial);
  useEffect(() => setValue(initial), [initial]);
  return (
    <form
      onSubmit={(e) => {
        e.preventDefault();
        if (value.trim()) onSubmit(value.trim());
      }}
    >
      <input
        type="text"
        value={value}
        placeholder="pubky://<owner>/pub/<client_id>/mayfly/chains/<CHAIN_ID>/"
        onChange={(e) => setValue(e.target.value)}
        spellCheck={false}
        autoFocus={!compact}
      />
      <button type="submit" className="primary" disabled={busy}>
        {compact ? "open" : "open chain"}
      </button>
    </form>
  );
}

function HowTo() {
  return (
    <footer className="howto">
      <h2>How to read this</h2>
      <ul>
        <li>
          A link is <em>provisional</em> while it is the head: it has a quorum of confirmations
          but no successor has embedded that quorum yet. It becomes <em>final</em> once the next
          link commits on top of it (§6.4).
        </li>
        <li>
          <em>witnessed m/k</em>: of the k witnesses engaged at that point, m have receipted the
          link. Receipts never gate a link; they are evidence of when it existed (§11).
        </li>
        <li>
          Every file is checked on its own: signature under the key it names, bytes against the
          homeserver ETag, bytes against the hash in its file name (§7).
        </li>
        <li>
          <span className="swatch bad" /> red: that file fails a check, or the verifier flagged
          it as a tampered mirror. The bytes in that folder are not what the name says.
        </li>
        <li>
          <span className="swatch warn" /> amber: the file is well-formed but is evidence of an
          anomaly attributed to a key (equivocation, a stale mirror, a vote out of turn, §9,
          §11.6).
        </li>
        <li>
          Everything shown comes from the same verifier the parties run; a bystander and a
          participant see identical facts (§14).
        </li>
      </ul>
    </footer>
  );
}

export function App() {
  const [url, setUrl] = useState<string>(hashUrl);
  const [loaded, setLoaded] = useState<Loaded | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [selected, setSelected] = useState<string | null>(null);
  const inFlight = useRef(false);
  const current = useRef(url);

  const refresh = useCallback(async (target: string) => {
    if (inFlight.current) return;
    inFlight.current = true;
    setBusy(true);
    try {
      const l = await loadChain(target);
      // Ignore a result for a chain the user has since navigated away from.
      if (current.current === target) {
        setLoaded(l);
        setError(null);
      }
    } catch (e) {
      if (current.current === target) setError(describeError(e));
    } finally {
      inFlight.current = false;
      setBusy(false);
    }
  }, []);

  // Open a link: a record URL is cut back to its chain folder so the hash is canonical and
  // shareable. An unparseable link is kept as typed so `refresh` can show the parse error.
  const open = useCallback(
    (next: string) => {
      let target = next;
      try {
        target = normaliseChainUrl(next).url;
      } catch {
        // Surfaced inline by refresh.
      }
      if (target === current.current) {
        void refresh(target);
        return;
      }
      current.current = target;
      setUrl(target);
      setLoaded(null);
      setError(null);
      setSelected(null);
      const hash = `#${target}`;
      if (window.location.hash !== hash) window.location.hash = hash;
      void refresh(target);
    },
    [refresh],
  );

  // A link in the hash on first load, and hash changes made in the address bar. Runs once:
  // `open` and `refresh` read the current target through refs, not through closed-over state.
  const openRef = useRef(open);
  openRef.current = open;
  useEffect(() => {
    if (current.current) void refresh(current.current);
    const onHash = () => {
      const h = hashUrl();
      if (h && h !== current.current) openRef.current(h);
    };
    window.addEventListener("hashchange", onHash);
    return () => window.removeEventListener("hashchange", onHash);
  }, [refresh]);

  // Live: poll while the chain is not final, or while there is no view yet (no genesis, a
  // transient store error). Stop once the verifier says the chain is final.
  const isFinal = loaded?.view?.is_final ?? false;
  useEffect(() => {
    if (!url || isFinal) return;
    const t = setInterval(() => void refresh(url), POLL_MS);
    return () => clearInterval(t);
  }, [url, isFinal, refresh]);

  const view = loaded?.view ?? null;
  const selectedFile = loaded?.files.find((f) => fileKey(f) === selected) ?? null;

  if (!url) {
    return (
      <div className="app">
        <div className="topbar">
          <h1>Mayfly chain viewer</h1>
          <span className="flavour">{IS_TESTNET ? "testnet" : "mainnet"}</span>
        </div>
        <div className="landing">
          <h2>Follow a chain from a link</h2>
          <p>
            Paste a <code>pubky://</code> link to a Mayfly chain folder, or to any record inside
            it. The viewer fetches every party's and witness's folder, runs the verifier, and
            shows each link with the checks on the bytes behind it. Read-only; no sign-in.
          </p>
          <LinkForm initial="" onSubmit={open} busy={busy} />
          <p>Accepted forms:</p>
          <ul>
            {EXAMPLES.map((e) => (
              <li key={e}>
                <code>{e}</code>
              </li>
            ))}
          </ul>
          <p>
            A link can also be opened directly as <code>#pubky://...</code> in the address bar.
          </p>
        </div>
        <HowTo />
      </div>
    );
  }

  return (
    <div className="app">
      <div className="topbar">
        <h1>Mayfly chain viewer</h1>
        <span className="flavour">{IS_TESTNET ? "testnet" : "mainnet"}</span>
        <LinkForm initial={url} onSubmit={open} busy={busy} compact />
      </div>

      <div className="live-bar">
        {isFinal ? (
          <span>
            <span className="badge ok">final</span> chain is final; polling stopped
          </span>
        ) : (
          <span>
            <span className="live-dot" />
            live: refreshing every {POLL_MS / 1000} s
          </span>
        )}
        {loaded ? <span>last updated {formatClock(loaded.loadedAt)}</span> : null}
        {busy ? <span className="dim">loading…</span> : null}
        <button onClick={() => void refresh(url)} disabled={busy}>
          refresh now
        </button>
      </div>

      {error ? <div className="error">{error}</div> : null}
      {loaded?.viewError ? <div className="error">{loaded.viewError}</div> : null}
      {loaded && loaded.rules && !loaded.rulesKnown ? (
        <div className="notice">
          Rules <code>{loaded.rules}</code> are not in this build, so the chain cannot be
          verified here. The files the initiator's folder lists are still shown below, decoded.
        </div>
      ) : null}
      {!loaded && !error ? <p className="dim">Fetching the chain…</p> : null}

      {view ? <Header view={view} rules={loaded?.rules ?? null} /> : null}
      {view ? <Timeline view={view} /> : null}
      {view ? <RulesState state={view.state} rules={view.rules ?? loaded?.rules ?? null} /> : null}
      {view && loaded ? <Anomalies view={view} files={loaded.files} onSelect={setSelected} /> : null}
      {loaded ? (
        <>
          <Files
            files={loaded.files}
            folders={loaded.folders}
            view={view}
            selected={selected}
            onSelect={(k) => setSelected(k === selected ? null : k)}
          />
          {selectedFile ? (
            <RecordPanel file={selectedFile} view={view} onClose={() => setSelected(null)} />
          ) : null}
        </>
      ) : null}
      <HowTo />
    </div>
  );
}
