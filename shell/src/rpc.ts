// The shell's only door into the vault: JSON-RPC 2.0 over the composd
// WebSocket (ARCHITECTURE.md §6). Auth is the startup token presented at
// `hello`. Event notifications carry per-topic sequence numbers; a gap
// means this client lagged and must resync its derived view.

import type { EventMsg, HelloResult } from "./types";

export class RpcError extends Error {
  readonly code: number;
  readonly type: string;

  constructor(code: number, type: string, message: string) {
    super(message);
    this.name = "RpcError";
    this.code = code;
    this.type = type;
  }
}

interface Pending {
  resolve: (value: unknown) => void;
  reject: (error: Error) => void;
}

interface WireResponse {
  id?: number | string | null;
  result?: unknown;
  error?: { code: number; message: string; data?: { type?: string } };
  method?: string;
  params?: unknown;
}

export class RpcClient {
  private readonly ws: WebSocket;
  private nextId = 1;
  private readonly pending = new Map<number, Pending>();
  private readonly eventHandlers = new Set<(ev: EventMsg) => void>();
  private readonly gapHandlers = new Set<(topic: string) => void>();
  private readonly closeHandlers = new Set<() => void>();
  private readonly lastSeq = new Map<string, number>();
  private closed = false;

  private constructor(ws: WebSocket) {
    this.ws = ws;
  }

  /** Open the socket, say hello, resolve once a role is granted. */
  static connect(
    url: string,
    token: string,
    role = "shell",
  ): Promise<{ client: RpcClient; hello: HelloResult }> {
    return new Promise((resolve, reject) => {
      let ws: WebSocket;
      try {
        ws = new WebSocket(url);
      } catch {
        reject(new Error(`invalid WebSocket URL: ${url}`));
        return;
      }
      let settled = false;
      const fail = (message: string) => {
        if (!settled) {
          settled = true;
          reject(new Error(message));
        }
      };
      ws.onerror = () => fail(`cannot reach composd at ${url}`);
      ws.onclose = () => fail(`connection closed before hello (${url})`);
      ws.onopen = () => {
        const client = new RpcClient(ws);
        ws.onmessage = (e) => client.onMessage(e);
        ws.onerror = () => client.handleClose();
        ws.onclose = () => client.handleClose();
        client.request<HelloResult>("hello", { role, token }).then(
          (hello) => {
            settled = true;
            resolve({ client, hello });
          },
          (err: Error) => {
            settled = true;
            ws.close();
            reject(err);
          },
        );
      };
    });
  }

  request<T>(method: string, params: unknown): Promise<T> {
    if (this.closed) {
      return Promise.reject(new Error("connection closed"));
    }
    const id = this.nextId++;
    const body = JSON.stringify({ jsonrpc: "2.0", id, method, params });
    return new Promise<T>((resolve, reject) => {
      this.pending.set(id, {
        resolve: (v) => resolve(v as T),
        reject,
      });
      this.ws.send(body);
    });
  }

  /** `commands.invoke` — the whole application surface (§7). */
  invoke<T>(command: string, input: unknown): Promise<T> {
    return this.request<T>("commands.invoke", { command, input });
  }

  /** Subscribe to topics and remember their baseline sequence numbers. */
  async subscribe(topics: string[]): Promise<void> {
    const res = await this.request<{
      subscribed: { topic: string; seq: number }[];
    }>("events.subscribe", { topics });
    for (const s of res.subscribed) {
      this.lastSeq.set(s.topic, s.seq);
    }
  }

  onEvent(handler: (ev: EventMsg) => void): void {
    this.eventHandlers.add(handler);
  }

  /** A sequence gap: this client missed events and must resync. */
  onGap(handler: (topic: string) => void): void {
    this.gapHandlers.add(handler);
  }

  onClose(handler: () => void): void {
    this.closeHandlers.add(handler);
  }

  close(): void {
    this.closed = true;
    this.ws.close();
  }

  private onMessage(e: MessageEvent): void {
    if (typeof e.data !== "string") {
      return;
    }
    let msg: WireResponse;
    try {
      msg = JSON.parse(e.data) as WireResponse;
    } catch {
      return;
    }
    if (typeof msg.id === "number") {
      const pending = this.pending.get(msg.id);
      if (pending === undefined) {
        return;
      }
      this.pending.delete(msg.id);
      if (msg.error !== undefined) {
        pending.reject(
          new RpcError(
            msg.error.code,
            msg.error.data?.type ?? "ERROR",
            msg.error.message,
          ),
        );
      } else {
        pending.resolve(msg.result);
      }
      return;
    }
    if (msg.method === "event" && msg.params !== undefined) {
      const ev = msg.params as EventMsg;
      const last = this.lastSeq.get(ev.topic);
      if (last !== undefined && ev.seq > last + 1) {
        for (const h of this.gapHandlers) {
          h(ev.topic);
        }
      }
      this.lastSeq.set(ev.topic, ev.seq);
      for (const h of this.eventHandlers) {
        h(ev);
      }
    }
  }

  private handleClose(): void {
    if (this.closed) {
      return;
    }
    this.closed = true;
    const err = new Error("connection to composd closed");
    for (const p of this.pending.values()) {
      p.reject(err);
    }
    this.pending.clear();
    for (const h of this.closeHandlers) {
      h();
    }
  }
}
