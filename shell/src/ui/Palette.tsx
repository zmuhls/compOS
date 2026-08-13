// The command palette (ARCHITECTURE.md §16 Phase 2): driven by
// `commands.list` plus the shell's own actions, so every operation is
// reachable from the keyboard through one door. Combobox + listbox pattern:
// the input filters, arrows move the active option, Enter runs it.

import { useMemo, useState } from "react";
import { Dialog } from "./Dialog";

export interface PaletteItem {
  id: string;
  label: string;
  hint?: string;
  keys?: string[];
  run: () => void;
}

interface PaletteProps {
  title: string;
  items: PaletteItem[];
  onClose: () => void;
}

export function Palette({ title, items, onClose }: PaletteProps) {
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (q === "") {
      return items;
    }
    return items.filter(
      (it) =>
        it.label.toLowerCase().includes(q) ||
        (it.hint ?? "").toLowerCase().includes(q),
    );
  }, [items, query]);

  const clamped = Math.min(active, Math.max(0, filtered.length - 1));

  const runItem = (item: PaletteItem | undefined) => {
    if (item === undefined) {
      return;
    }
    onClose();
    // Run after close so focus restoration happens first; the action may
    // move focus itself (e.g. into the search field).
    setTimeout(item.run, 0);
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setActive(Math.min(clamped + 1, filtered.length - 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setActive(Math.max(clamped - 1, 0));
    } else if (e.key === "Enter") {
      e.preventDefault();
      runItem(filtered[clamped]);
    }
  };

  return (
    <Dialog title={title} onClose={onClose}>
      <div className="palette">
        <input
          data-autofocus
          type="text"
          role="combobox"
          aria-expanded="true"
          aria-controls="palette-list"
          aria-activedescendant={
            filtered.length > 0 ? `palette-opt-${clamped}` : undefined
          }
          aria-label="Filter commands"
          autoComplete="off"
          spellCheck={false}
          placeholder="Type to filter…"
          value={query}
          onChange={(e) => {
            setQuery(e.target.value);
            setActive(0);
          }}
          onKeyDown={onKeyDown}
        />
        <ul id="palette-list" role="listbox" aria-label={title}>
          {filtered.length === 0 ? (
            <li className="palette-empty" role="presentation">
              No matching commands
            </li>
          ) : (
            filtered.map((it, i) => (
              // The option itself is the interactive surface: DOM focus
              // stays on the combobox input (aria-activedescendant points
              // here), so nesting a button would be a nested-interactive
              // violation.
              <li
                key={it.id}
                id={`palette-opt-${i}`}
                role="option"
                aria-selected={i === clamped}
                className={i === clamped ? "palette-item active" : "palette-item"}
                onClick={() => runItem(it)}
                onMouseMove={() => setActive(i)}
              >
                <span className="palette-label">{it.label}</span>
                {it.hint !== undefined && (
                  <span className="palette-hint">{it.hint}</span>
                )}
                {it.keys !== undefined && it.keys.length > 0 && (
                  <kbd>{it.keys.join(" ")}</kbd>
                )}
              </li>
            ))
          )}
        </ul>
      </div>
    </Dialog>
  );
}
