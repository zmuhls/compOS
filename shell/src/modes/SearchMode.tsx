// Search mode: FTS5 through `search.query` (§12). Snippets arrive with
// [bracket] markers around matches; they are rendered as real <mark>
// elements built from text nodes — no HTML round-trip.

import { useRef, useState } from "react";
import type { ReactNode } from "react";
import { RpcError } from "../rpc";
import type { RpcClient } from "../rpc";
import type { SearchHit } from "../types";

interface SearchModeProps {
  client: RpcClient;
  onOpen: (path: string) => void;
}

function snippetNodes(snippet: string): ReactNode[] {
  const out: ReactNode[] = [];
  let buf = "";
  let inMark = false;
  let key = 0;
  for (const ch of snippet) {
    if (!inMark && ch === "[") {
      if (buf !== "") {
        out.push(buf);
        buf = "";
      }
      inMark = true;
    } else if (inMark && ch === "]") {
      out.push(<mark key={key++}>{buf}</mark>);
      buf = "";
      inMark = false;
    } else {
      buf += ch;
    }
  }
  if (buf !== "") {
    out.push(buf);
  }
  return out;
}

export function SearchMode({ client, onOpen }: SearchModeProps) {
  const [query, setQuery] = useState("");
  const [hits, setHits] = useState<SearchHit[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);

  const run = async () => {
    const q = query.trim();
    if (q === "") {
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const res = await client.invoke<{ hits: SearchHit[] }>("search.query", {
        query: q,
        limit: 50,
      });
      setHits(res.hits);
    } catch (e) {
      setHits(null);
      setError(e instanceof RpcError ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="pane pane-single" aria-label="Search">
      <form
        role="search"
        className="search-form"
        onSubmit={(e) => {
          e.preventDefault();
          void run();
        }}
      >
        <label htmlFor="search-input">Search the vault (FTS5 syntax)</label>
        <div className="search-row">
          <input
            id="search-input"
            ref={inputRef}
            // Focus follows intent: entering Search (Mod-K or the tab) is a
            // request to type a query.
            autoFocus
            type="search"
            value={query}
            autoComplete="off"
            spellCheck={false}
            aria-describedby={error !== null ? "search-error" : undefined}
            onChange={(e) => setQuery(e.target.value)}
          />
          <button type="submit" disabled={busy}>
            {busy ? "Searching…" : "Search"}
          </button>
        </div>
      </form>
      {error !== null && (
        <p id="search-error" className="error" role="alert">
          {error}
        </p>
      )}
      {hits !== null && (
        <div aria-live="polite">
          <h2 className="results-head">
            {hits.length === 0
              ? "No results"
              : `${hits.length} result${hits.length === 1 ? "" : "s"}`}
          </h2>
          <ol className="hit-list">
            {hits.map((h) => (
              <li key={`${h.doc}`}>
                <button type="button" onClick={() => onOpen(h.path)}>
                  <span className="hit-path">{h.path}</span>
                  <span className="hit-snippet">{snippetNodes(h.snippet)}</span>
                </button>
              </li>
            ))}
          </ol>
        </div>
      )}
    </section>
  );
}
