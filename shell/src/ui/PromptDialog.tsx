// One labeled text input in a dialog — used for "new document" paths and
// anywhere else a single value is needed without leaving the keyboard.

import { useState } from "react";
import { Dialog } from "./Dialog";

interface PromptDialogProps {
  title: string;
  label: string;
  initial?: string;
  submitLabel?: string;
  onSubmit: (value: string) => void;
  onClose: () => void;
}

export function PromptDialog({
  title,
  label,
  initial,
  submitLabel,
  onSubmit,
  onClose,
}: PromptDialogProps) {
  const [value, setValue] = useState(initial ?? "");

  return (
    <Dialog title={title} onClose={onClose}>
      <form
        className="dialog-form"
        onSubmit={(e) => {
          e.preventDefault();
          const v = value.trim();
          if (v !== "") {
            onClose();
            setTimeout(() => onSubmit(v), 0);
          }
        }}
      >
        <label>
          {label}
          <input
            data-autofocus
            type="text"
            value={value}
            autoComplete="off"
            spellCheck={false}
            onChange={(e) => setValue(e.target.value)}
          />
        </label>
        <div className="dialog-actions">
          <button type="submit">{submitLabel ?? "OK"}</button>
          <button type="button" onClick={onClose}>
            Cancel
          </button>
        </div>
      </form>
    </Dialog>
  );
}
