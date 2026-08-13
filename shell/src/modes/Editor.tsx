// CodeMirror 6 wrapper — greenfield per ARCHITECTURE.md §12 (no donor
// editor code exists). One view instance per mount; the document is swapped
// when `docKey` changes and refreshed in place when composd reports a new
// head. Tab is deliberately left unbound so keyboard focus is never trapped
// (WCAG 2.1.2); indentation is a palette concern later.

import { useEffect, useRef } from "react";
import { EditorState } from "@codemirror/state";
import { EditorView, keymap, lineNumbers } from "@codemirror/view";
import { defaultKeymap, history, historyKeymap } from "@codemirror/commands";
import { defaultHighlightStyle, syntaxHighlighting } from "@codemirror/language";
import { markdown } from "@codemirror/lang-markdown";

interface EditorProps {
  docKey: string;
  value: string;
  ariaLabel: string;
  /** Increment to move focus into the editor. Only user-initiated opens
   *  bump this — background refreshes must never steal focus. */
  focusTick: number;
  onChange: (text: string) => void;
  onSave: () => void;
}

export function Editor({
  docKey,
  value,
  ariaLabel,
  focusTick,
  onChange,
  onSave,
}: EditorProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  const viewRef = useRef<EditorView | null>(null);
  const onChangeRef = useRef(onChange);
  const onSaveRef = useRef(onSave);
  const labelRef = useRef(ariaLabel);
  onChangeRef.current = onChange;
  onSaveRef.current = onSave;
  labelRef.current = ariaLabel;

  // (Re)create the view when the document identity changes.
  useEffect(() => {
    const container = containerRef.current;
    if (container === null) {
      return;
    }
    const state = EditorState.create({
      doc: value,
      extensions: [
        lineNumbers(),
        history(),
        markdown(),
        syntaxHighlighting(defaultHighlightStyle),
        EditorView.lineWrapping,
        EditorView.contentAttributes.of({ "aria-label": labelRef.current }),
        keymap.of([
          {
            key: "Mod-s",
            preventDefault: true,
            run: () => {
              onSaveRef.current();
              return true;
            },
          },
          ...defaultKeymap,
          ...historyKeymap,
        ]),
        EditorView.updateListener.of((update) => {
          if (update.docChanged) {
            onChangeRef.current(update.state.doc.toString());
          }
        }),
      ],
    });
    const view = new EditorView({ state, parent: container });
    viewRef.current = view;
    return () => {
      view.destroy();
      viewRef.current = null;
    };
    // The initial text for a new docKey is read once; later external
    // refreshes go through the value-sync effect below.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [docKey]);

  // External refresh (head moved and the buffer was clean): replace the
  // text without recreating the view.
  useEffect(() => {
    const view = viewRef.current;
    if (view === null) {
      return;
    }
    const current = view.state.doc.toString();
    if (current !== value) {
      view.dispatch({
        changes: { from: 0, to: current.length, insert: value },
      });
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [value]);

  // Runs after the view-creation effect above (declaration order), so a
  // bump that lands together with a new docKey focuses the fresh view —
  // and after any dialog's focus restoration has already happened. docKey
  // is deliberately not a dependency: a background reload swaps the view
  // without re-running this, so it can never steal focus. A fresh mount
  // (entering Write mode) runs it once, which is the intended "mode switch
  // means edit" behavior.
  useEffect(() => {
    if (focusTick > 0) {
      viewRef.current?.focus();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [focusTick]);

  return <div ref={containerRef} className="editor-host" />;
}
