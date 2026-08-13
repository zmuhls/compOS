// The front door: composd's WebSocket address and the startup token it
// wrote to <vault>/state/rpc-token (§6 — localhost bind + 0600 token file
// on every profile). The token can also arrive as ?token= in the URL.

import { useState } from "react";
import { RpcClient } from "./rpc";
import type { HelloResult } from "./types";

interface ConnectProps {
  initialUrl: string;
  initialToken: string;
  notice: string | null;
  onConnected: (client: RpcClient, hello: HelloResult, url: string, token: string) => void;
}

export function Connect({ initialUrl, initialToken, notice, onConnected }: ConnectProps) {
  const [url, setUrl] = useState(initialUrl);
  const [token, setToken] = useState(initialToken);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const submit = async () => {
    setBusy(true);
    setError(null);
    try {
      const { client, hello } = await RpcClient.connect(url.trim(), token.trim());
      onConnected(client, hello, url.trim(), token.trim());
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <main id="main" className="connect-page">
      <h1>CompOS</h1>
      <p className="rule" role="presentation" />
      <p>
        The shell talks to your vault only through <code>composd</code>. Start
        it (<code>composd --vault &lt;path&gt;</code>), then paste the token it
        wrote to <code>&lt;vault&gt;/state/rpc-token</code>.
      </p>
      {notice !== null && (
        <p className="error" role="alert">
          {notice}
        </p>
      )}
      <form
        className="connect-form"
        onSubmit={(e) => {
          e.preventDefault();
          void submit();
        }}
      >
        <label>
          composd WebSocket URL
          <input
            type="text"
            value={url}
            autoComplete="off"
            spellCheck={false}
            onChange={(e) => setUrl(e.target.value)}
          />
        </label>
        <label>
          Startup token
          <input
            type="password"
            value={token}
            autoComplete="off"
            onChange={(e) => setToken(e.target.value)}
          />
        </label>
        {error !== null && (
          <p className="error" role="alert">
            {error}
          </p>
        )}
        <div>
          <button type="submit" disabled={busy}>
            {busy ? "Connecting…" : "Connect"}
          </button>
        </div>
      </form>
    </main>
  );
}
