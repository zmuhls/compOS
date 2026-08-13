// Read mode, the Markdown slice of §12: canonical content rendered through
// micromark (raw HTML stays escaped — document content is data, not
// markup), with a generated heading outline for keyboard navigation.

import { useEffect, useMemo, useRef, useState } from "react";
import { micromark } from "micromark";
import type { OpenDoc } from "../App";

interface Heading {
  id: string;
  level: number;
  text: string;
}

interface ReadModeProps {
  current: OpenDoc | null;
}

export function ReadMode({ current }: ReadModeProps) {
  const articleRef = useRef<HTMLElement>(null);
  const [outline, setOutline] = useState<Heading[]>([]);

  // Safety contract for the dangerouslySetInnerHTML below: with these
  // options pinned (micromark's defaults, stated so they cannot drift), raw
  // HTML in the document is escaped to text and javascript:/data: link
  // protocols are refused — the emitted string contains only
  // micromark-generated markup. Do not flip these without adding a real
  // sanitizer between micromark and the DOM.
  const html = useMemo(
    () =>
      current === null
        ? ""
        : micromark(current.text, {
            allowDangerousHtml: false,
            allowDangerousProtocol: false,
          }),
    [current],
  );

  useEffect(() => {
    const article = articleRef.current;
    if (article === null) {
      setOutline([]);
      return;
    }
    const found: Heading[] = [];
    const nodes = article.querySelectorAll<HTMLElement>("h1, h2, h3, h4, h5, h6");
    nodes.forEach((h, i) => {
      const id = `hd-${i}`;
      h.id = id;
      h.tabIndex = -1;
      found.push({
        id,
        level: Number(h.tagName.slice(1)),
        text: h.textContent ?? "",
      });
    });
    setOutline(found);
  }, [html]);

  if (current === null) {
    return (
      <div className="empty-state">
        <p>Open a document in Write mode first — Read renders its head.</p>
      </div>
    );
  }

  return (
    <div className="split">
      <nav className="side" aria-label="Contents">
        <div className="side-head">
          <h2>Contents</h2>
        </div>
        {outline.length === 0 ? (
          <p className="muted">No headings.</p>
        ) : (
          <ul className="outline">
            {outline.map((h) => (
              <li key={h.id} style={{ paddingLeft: `${(h.level - 1) * 0.75}rem` }}>
                <button
                  type="button"
                  onClick={() => {
                    const el = document.getElementById(h.id);
                    el?.scrollIntoView();
                    el?.focus();
                  }}
                >
                  {h.text}
                </button>
              </li>
            ))}
          </ul>
        )}
      </nav>
      <section className="pane" aria-label="Reading view">
        <header className="pane-head">
          <h2 className="doc-title">{current.path}</h2>
          <p className="doc-meta">
            {current.dirty ? "rendering unsaved buffer" : "rendering saved head"}
          </p>
        </header>
        <article
          ref={articleRef}
          className="prose"
          aria-label={`Rendered ${current.path}`}
          // Safe by construction: micromark escapes raw HTML by default.
          dangerouslySetInnerHTML={{ __html: html }}
        />
      </section>
    </div>
  );
}
