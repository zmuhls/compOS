// Review mode (§12): AI proposals with per-hunk acceptance. Staleness is
// server-derived and re-checked at accept time — this view only ever asks;
// composd decides. Diff lines carry +/− glyphs as text, so the meaning
// survives without color (WCAG 1.4.1).

import { useCallback, useEffect, useState } from "react";
import { RpcError } from "../rpc";
import type { RpcClient } from "../rpc";
import type { Hunk, Proposal, ProposalDetail } from "../types";

interface ReviewModeProps {
  client: RpcClient;
  version: number;
  onStatus: (kind: "info" | "error", text: string) => void;
}

const CONTEXT = 2;

interface DiffLine {
  kind: "ctx" | "del" | "add";
  no: number | null;
  text: string;
}

function hunkDiff(baseLines: string[], hunk: Hunk): DiffLine[] {
  const out: DiffLine[] = [];
  const from = Math.max(0, hunk.start - CONTEXT);
  for (let i = from; i < hunk.start; i++) {
    out.push({ kind: "ctx", no: i + 1, text: baseLines[i] ?? "" });
  }
  for (let i = hunk.start; i < hunk.start + hunk.del; i++) {
    out.push({ kind: "del", no: i + 1, text: baseLines[i] ?? "" });
  }
  if (hunk.ins !== "") {
    const inserted = hunk.ins.split("\n");
    // A trailing newline produces one empty tail segment; drop it.
    if (inserted.length > 0 && inserted[inserted.length - 1] === "") {
      inserted.pop();
    }
    for (const text of inserted) {
      out.push({ kind: "add", no: null, text });
    }
  }
  const tail = hunk.start + hunk.del;
  for (let i = tail; i < Math.min(baseLines.length, tail + CONTEXT); i++) {
    out.push({ kind: "ctx", no: i + 1, text: baseLines[i] ?? "" });
  }
  return out;
}

function splitBase(content: string): string[] {
  const lines = content.split("\n");
  if (lines.length > 0 && lines[lines.length - 1] === "") {
    lines.pop();
  }
  return lines;
}

export function ReviewMode({ client, version, onStatus }: ReviewModeProps) {
  const [proposals, setProposals] = useState<Proposal[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [detail, setDetail] = useState<ProposalDetail | null>(null);
  const [checked, setChecked] = useState<ReadonlySet<number>>(new Set());
  const [busy, setBusy] = useState(false);

  const refreshList = useCallback(async () => {
    const res = await client.invoke<{ proposals: Proposal[] }>("proposal.list", {});
    // Newest first; open ones on top of the resolved tail.
    const sorted = [...res.proposals].reverse();
    sorted.sort((a, b) =>
      a.state === "open" && b.state !== "open"
        ? -1
        : a.state !== "open" && b.state === "open"
          ? 1
          : 0,
    );
    setProposals(sorted);
    setSelectedId((prev) => {
      if (prev !== null && sorted.some((p) => p.proposal === prev)) {
        return prev;
      }
      return sorted[0]?.proposal ?? null;
    });
  }, [client]);

  useEffect(() => {
    refreshList().catch((e: unknown) => onStatus("error", String(e)));
  }, [refreshList, version, onStatus]);

  useEffect(() => {
    if (selectedId === null) {
      setDetail(null);
      return;
    }
    let cancelled = false;
    client
      .invoke<ProposalDetail>("proposal.get", { proposal: selectedId })
      .then((d) => {
        if (!cancelled) {
          setDetail(d);
          setChecked(new Set(d.hunks.map((_, i) => i)));
        }
      })
      .catch((e: unknown) => onStatus("error", String(e)));
    return () => {
      cancelled = true;
    };
  }, [client, selectedId, version, onStatus]);

  const act = async (fn: () => Promise<void>) => {
    setBusy(true);
    try {
      await fn();
    } catch (e) {
      onStatus("error", e instanceof RpcError ? `${e.type}: ${e.message}` : String(e));
    } finally {
      setBusy(false);
      await refreshList().catch(() => undefined);
    }
  };

  const accept = () =>
    act(async () => {
      if (detail === null) {
        return;
      }
      const hunks = [...checked].sort((a, b) => a - b);
      const res = await client.invoke<{ rev: string; path: string }>(
        "proposal.accept.hunk",
        { proposal: detail.proposal, hunks },
      );
      onStatus(
        "info",
        `Accepted ${hunks.length} hunk${hunks.length === 1 ? "" : "s"} into ${res.path} (${res.rev.slice(0, 10)}…)`,
      );
    });

  const reject = () =>
    act(async () => {
      if (detail === null) {
        return;
      }
      await client.invoke("proposal.reject", { proposal: detail.proposal });
      onStatus("info", "Proposal rejected.");
    });

  const baseLines = detail === null ? [] : splitBase(detail.base_content);
  const open = detail !== null && detail.state === "open";

  return (
    <div className="split">
      <nav className="side" aria-label="Proposals">
        <div className="side-head">
          <h2>Proposals</h2>
        </div>
        {proposals.length === 0 ? (
          <p className="muted">
            No proposals. Agents open them through the propose-capped API;
            they land here for review.
          </p>
        ) : (
          <ul className="doc-list">
            {proposals.map((p) => (
              <li key={p.proposal}>
                <button
                  type="button"
                  aria-current={p.proposal === selectedId ? "true" : undefined}
                  onClick={() => setSelectedId(p.proposal)}
                >
                  <span className="hit-path">{p.path}</span>
                  <span className="prop-meta">
                    {p.state}
                    {p.state === "open" && p.stale ? " · stale" : ""}
                    {` · ${p.hunks.length} hunk${p.hunks.length === 1 ? "" : "s"}`}
                  </span>
                </button>
              </li>
            ))}
          </ul>
        )}
      </nav>
      <section className="pane" aria-label="Proposal review">
        {detail === null ? (
          <div className="empty-state">
            <p>Select a proposal to review it hunk by hunk.</p>
          </div>
        ) : (
          <>
            <header className="pane-head">
              <h2 className="doc-title">{detail.path}</h2>
              <p className="doc-meta">
                {detail.state}
                {detail.base !== null
                  ? ` · base ${detail.base.slice(0, 10)}…`
                  : " · proposes a new document"}
              </p>
            </header>
            {open && detail.stale && (
              <div className="banner" role="alert">
                <p>
                  Stale: the document moved past this proposal&apos;s base.
                  Accepting is refused by composd; reject it or ask for a
                  fresh proposal.
                </p>
              </div>
            )}
            <div className="hunks">
              {detail.hunks.map((h, i) => (
                <fieldset className="hunk" key={i}>
                  <legend>
                    <label>
                      <input
                        type="checkbox"
                        checked={checked.has(i)}
                        disabled={!open}
                        onChange={(e) => {
                          const next = new Set(checked);
                          if (e.target.checked) {
                            next.add(i);
                          } else {
                            next.delete(i);
                          }
                          setChecked(next);
                        }}
                      />{" "}
                      Hunk {i + 1}: line {h.start + 1}
                      {h.del > 0 ? `, replaces ${h.del} line${h.del === 1 ? "" : "s"}` : ", inserts"}
                    </label>
                  </legend>
                  <pre className="diff">
                    {hunkDiff(baseLines, h).map((l, j) => (
                      <span key={j} className={`line line-${l.kind}`}>
                        {l.kind === "add" ? "+" : l.kind === "del" ? "−" : " "}
                        {l.text}
                        {"\n"}
                      </span>
                    ))}
                  </pre>
                </fieldset>
              ))}
            </div>
            {open && (
              <div className="review-actions">
                <button
                  type="button"
                  onClick={() => void accept()}
                  disabled={busy || detail.stale || checked.size === 0}
                >
                  Accept {checked.size} of {detail.hunks.length}
                </button>
                <button type="button" onClick={() => void reject()} disabled={busy}>
                  Reject
                </button>
              </div>
            )}
            {detail.state === "accepted" && detail.rev !== undefined && (
              <p className="muted">
                Landed as <code>{detail.rev.slice(0, 12)}…</code> (origin
                proposal-accept).
              </p>
            )}
          </>
        )}
      </section>
    </div>
  );
}
