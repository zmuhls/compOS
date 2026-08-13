// The generic escape hatch that keeps the surface keyboard-complete: any
// registry command can be invoked with hand-written JSON input, and the
// result (or typed error) is shown in place. The palette offers this for
// commands the shell has no dedicated UI for.

import { useState } from "react";
import { Dialog } from "./Dialog";
import { RpcError } from "../rpc";
import type { RpcClient } from "../rpc";
import type { CommandSpec } from "../types";

interface RawInvokeProps {
  client: RpcClient;
  spec: CommandSpec;
  onClose: () => void;
}

export function RawInvoke({ client, spec, onClose }: RawInvokeProps) {
  const [input, setInput] = useState("{}");
  const [output, setOutput] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const run = async () => {
    let parsed: unknown;
    try {
      parsed = JSON.parse(input);
    } catch (e) {
      setError(`input is not valid JSON: ${String(e)}`);
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const result = await client.invoke<unknown>(spec.id, parsed);
      setOutput(JSON.stringify(result, null, 2));
    } catch (e) {
      if (e instanceof RpcError) {
        setError(`${e.type}: ${e.message}`);
      } else {
        setError(String(e));
      }
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog title={`Invoke ${spec.id}`} wide onClose={onClose}>
      <div className="dialog-form">
        <p className="muted">
          {spec.summary} <em>(effect: {spec.effect})</em>
        </p>
        <label>
          Input (JSON)
          <textarea
            data-autofocus
            rows={6}
            value={input}
            spellCheck={false}
            onChange={(e) => setInput(e.target.value)}
          />
        </label>
        <div className="dialog-actions">
          <button type="button" onClick={() => void run()} disabled={busy}>
            {busy ? "Running…" : "Invoke"}
          </button>
        </div>
        {error !== null && (
          <p className="error" role="alert">
            {error}
          </p>
        )}
        {output !== null && (
          <>
            <h3>Result</h3>
            <pre className="invoke-result" tabIndex={0} aria-label="Command result">
              {output}
            </pre>
          </>
        )}
      </div>
    </Dialog>
  );
}
