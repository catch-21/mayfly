import { useCallback, useEffect, useRef, useState } from "react";
import { DEFAULT_POLL_MS, describeError, parseChainUrl, type RulesRegistry, type Store } from "@synonymdev/mayfly-browser";
import { useChainReader } from "@synonymdev/mayfly-browser/react";

import { IS_TESTNET, reader } from "./mayfly";
import { formatClock } from "./format";
import { Anomalies } from "./components/Anomalies";
import { Files, fileKey } from "./components/Files";
import { Header } from "./components/Header";
import { RecordPanel } from "./components/RecordPanel";
import { RulesState } from "./components/RulesState";
import { Timeline } from "./components/Timeline";

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
          <em>seen</em> is when a watchman receipted the proposal. <em>think</em> is how long the
          author took after the previous quorum, and <em>confirm delay</em> is how long the other
          parties took to confirm — that delay is not part of the author's time. <em>author's
          clock</em> is the timestamp the signer wrote, which is not the watchman's time. A link
          with no receipt yet says it is not yet timed.
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
  const [parseError, setParseError] = useState<string | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [pieces, setPieces] = useState<{ store: Store; registry: RulesRegistry }>();
  const [fatal, setFatal] = useState<string | null>(null);
  const current = useRef(url);

  useEffect(() => {
    reader()
      .then(setPieces)
      .catch((e) => setFatal(describeError(e)));
  }, []);

  // Open a link: a record URL is cut back to its chain folder so the hash is canonical and
  // shareable. An unparseable link is shown as such and nothing is fetched.
  const open = useCallback((next: string) => {
    let target: string;
    try {
      target = parseChainUrl(next).url;
    } catch (e) {
      setParseError(describeError(e));
      return;
    }
    setParseError(null);
    if (target !== current.current) {
      current.current = target;
      setUrl(target);
      setSelected(null);
    }
    const hash = `#${target}`;
    if (window.location.hash !== hash) window.location.hash = hash;
  }, []);

  // A link in the hash on first load, and hash changes made in the address bar.
  const openRef = useRef(open);
  openRef.current = open;
  useEffect(() => {
    const onHash = () => {
      const h = hashUrl();
      if (h && h !== current.current) openRef.current(h);
    };
    window.addEventListener("hashchange", onHash);
    return () => window.removeEventListener("hashchange", onHash);
  }, []);

  // Follow the chain: the reader polls while it is open and stops once it is final.
  const chain = useChainReader(pieces && url ? { store: pieces.store, registry: pieces.registry, url } : undefined);
  const loaded = chain.loaded;
  const view = loaded?.view ?? null;
  const selectedFile = loaded?.files.find((f) => fileKey(f) === selected) ?? null;

  if (fatal) return <div className="app error">{fatal}</div>;

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
          <LinkForm initial="" onSubmit={open} busy={chain.busy} />
          {parseError ? <div className="error">{parseError}</div> : null}
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
        <LinkForm initial={url} onSubmit={open} busy={chain.busy} compact />
      </div>

      <div className="live-bar">
        {chain.final ? (
          <span>
            <span className="badge ok">final</span> chain is final; polling stopped
          </span>
        ) : (
          <span>
            <span className="live-dot" />
            live: refreshing every {DEFAULT_POLL_MS / 1000} s
          </span>
        )}
        {loaded ? <span>last updated {formatClock(loaded.loadedAt)}</span> : null}
        {chain.busy ? <span className="dim">loading…</span> : null}
        <button onClick={() => void chain.refresh()} disabled={chain.busy}>
          refresh now
        </button>
      </div>

      {parseError ? <div className="error">{parseError}</div> : null}
      {chain.error ? <div className="error">{chain.error}</div> : null}
      {loaded?.view_error ? <div className="error">{loaded.view_error}</div> : null}
      {loaded && loaded.rules && !loaded.rules_known ? (
        <div className="notice">
          Rules <code>{loaded.rules}</code> are not in this build, so the chain cannot be
          verified here. The files the initiator's folder lists are still shown below, decoded.
        </div>
      ) : null}
      {!loaded && !chain.error ? <p className="dim">Fetching the chain…</p> : null}

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
