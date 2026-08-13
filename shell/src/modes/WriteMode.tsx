// Write mode (§12): document list plus the CodeMirror editor. Saving goes
// through `document.save` with the tracked base revision; a StaleBase
// answer surfaces as a conflict banner — the shell never overwrites and
// never bypasses composd.

import { Editor } from "./Editor";
import type { DocumentEntry } from "../types";
import type { OpenDoc } from "../App";

interface WriteModeProps {
  docs: DocumentEntry[];
  current: OpenDoc | null;
  focusTick: number;
  onOpen: (path: string) => void;
  onNew: () => void;
  onChange: (text: string) => void;
  onSave: () => void;
  onReloadHead: () => void;
}

export function WriteMode({
  docs,
  current,
  focusTick,
  onOpen,
  onNew,
  onChange,
  onSave,
  onReloadHead,
}: WriteModeProps) {
  return (
    <div className="split">
      <nav className="side" aria-label="Documents">
        <div className="side-head">
          <h2>Documents</h2>
          <button type="button" onClick={onNew}>
            New
          </button>
        </div>
        {docs.length === 0 ? (
          <p className="muted">Empty vault. Create a document to begin.</p>
        ) : (
          <ul className="doc-list">
            {docs.map((d) => (
              <li key={d.doc}>
                <button
                  type="button"
                  aria-current={current?.path === d.path ? "true" : undefined}
                  onClick={() => onOpen(d.path)}
                >
                  {d.path}
                </button>
              </li>
            ))}
          </ul>
        )}
      </nav>
      <section className="pane" aria-label="Editor">
        {current === null ? (
          <div className="empty-state">
            <p>
              No document open. Choose one from the list, or press{" "}
              <kbd>Mod-P</kbd> and run <em>New document</em>.
            </p>
          </div>
        ) : (
          <>
            <header className="pane-head">
              <h2 className="doc-title">{current.path}</h2>
              <p className="doc-meta">
                {current.baseRev === null
                  ? "new document"
                  : `base ${current.baseRev.slice(0, 10)}…`}
                {current.dirty ? " · unsaved changes" : " · saved"}
              </p>
            </header>
            {current.conflict !== null && (
              <div className="banner" role="alert">
                <p>
                  The document moved on to revision{" "}
                  <code>{current.conflict.slice(0, 10)}…</code> while you were
                  editing. Saving now would be refused (stale base).
                </p>
                <button type="button" onClick={onReloadHead}>
                  Reload head (discards this buffer)
                </button>
              </div>
            )}
            <Editor
              docKey={current.key}
              value={current.text}
              ariaLabel={`Editing ${current.path}`}
              focusTick={focusTick}
              onChange={onChange}
              onSave={onSave}
            />
          </>
        )}
      </section>
    </div>
  );
}
