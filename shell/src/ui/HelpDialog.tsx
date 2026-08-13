// Keyboard reference. Every listed operation is also reachable through the
// palette — the shortcuts are accelerators, not the only path.

import { Dialog } from "./Dialog";

const ROWS: [string, string][] = [
  ["Mod-P", "Command palette (every action lives here)"],
  ["Mod-K", "Search"],
  ["Mod-S", "Save the open document"],
  ["Alt-1 … Alt-4", "Write, Read, Search, Review"],
  ["Escape", "Close dialogs and menus"],
  ["Tab / Shift-Tab", "Move between controls (never trapped in the editor)"],
];

export function HelpDialog({ onClose }: { onClose: () => void }) {
  return (
    <Dialog title="Keyboard" onClose={onClose}>
      <table className="help-table">
        <caption className="visually-hidden">Keyboard shortcuts</caption>
        <thead>
          <tr>
            <th scope="col">Keys</th>
            <th scope="col">Action</th>
          </tr>
        </thead>
        <tbody>
          {ROWS.map(([keys, action]) => (
            <tr key={keys}>
              <td>
                <kbd>{keys}</kbd>
              </td>
              <td>{action}</td>
            </tr>
          ))}
        </tbody>
      </table>
      <p className="muted">
        “Mod” is Command on macOS, Control elsewhere.
      </p>
    </Dialog>
  );
}
