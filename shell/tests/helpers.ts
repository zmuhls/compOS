// Shared plumbing: connect the page to the suite's composd, assert axe
// cleanliness, and drive an agent-role JSON-RPC session from Node — the
// same boundary a real agentd will use, capability caps included.

import { AxeBuilder } from "@axe-core/playwright";
import { expect } from "@playwright/test";
import type { Page } from "@playwright/test";

export function env(name: string): string {
  const v = process.env[name];
  if (v === undefined || v === "") {
    throw new Error(`${name} not set — global setup did not run?`);
  }
  return v;
}

/** Load the shell pre-authenticated via the ?token= launch path. */
export async function gotoConnected(page: Page): Promise<void> {
  // Uncaught page errors are the difference between "element not found"
  // and knowing why — surface them in the runner output.
  page.on("pageerror", (e) => {
    console.error(`[pageerror] ${e.message}`);
  });
  const ws = encodeURIComponent(env("COMPOS_E2E_WS"));
  const token = encodeURIComponent(env("COMPOS_E2E_TOKEN"));
  await page.goto(`/?ws=${ws}&token=${token}`);
  await expect(page.getByRole("navigation", { name: "Modes" })).toBeVisible();
}

/** WCAG 2.2 AA sweep of the current page state. */
export async function expectAxeClean(page: Page): Promise<void> {
  const results = await new AxeBuilder({ page })
    .withTags(["wcag2a", "wcag2aa", "wcag21a", "wcag21aa", "wcag22aa"])
    .analyze();
  const readable = results.violations.map((v) => ({
    id: v.id,
    impact: v.impact,
    help: v.help,
    nodes: v.nodes.map((n) => n.html).slice(0, 5),
  }));
  expect(readable).toEqual([]);
}

interface WireResponse {
  id?: number;
  result?: unknown;
  error?: { message: string; data?: { type?: string } };
}

/** One-shot agent-role RPC session over the daemon's WebSocket. */
export async function agentSession(): Promise<{
  invoke: (command: string, input: unknown) => Promise<unknown>;
  close: () => void;
}> {
  const ws = new WebSocket(env("COMPOS_E2E_WS"));
  await new Promise<void>((resolve, reject) => {
    ws.onopen = () => resolve();
    ws.onerror = () => reject(new Error("agent websocket failed"));
  });
  let nextId = 1;
  const pending = new Map<
    number,
    { resolve: (v: unknown) => void; reject: (e: Error) => void }
  >();
  ws.onmessage = (e: MessageEvent) => {
    const msg = JSON.parse(String(e.data)) as WireResponse;
    if (typeof msg.id !== "number") {
      return;
    }
    const p = pending.get(msg.id);
    if (p === undefined) {
      return;
    }
    pending.delete(msg.id);
    if (msg.error !== undefined) {
      p.reject(
        new Error(`${msg.error.data?.type ?? "ERROR"}: ${msg.error.message}`),
      );
    } else {
      p.resolve(msg.result);
    }
  };
  const request = (method: string, params: unknown): Promise<unknown> => {
    const id = nextId++;
    ws.send(JSON.stringify({ jsonrpc: "2.0", id, method, params }));
    return new Promise((resolve, reject) => {
      pending.set(id, { resolve, reject });
    });
  };
  await request("hello", { role: "agent", token: env("COMPOS_E2E_TOKEN") });
  return {
    invoke: (command, input) =>
      request("commands.invoke", { command, input }),
    close: () => ws.close(),
  };
}

/** Agent proposes hunks against a path; returns the proposal id. */
export async function agentPropose(
  path: string,
  hunks: { start: number; del: number; ins: string }[],
): Promise<string> {
  const agent = await agentSession();
  try {
    const res = (await agent.invoke("proposal.create", {
      path,
      hunks,
      provenance: { provider: "e2e", model: "none" },
      evidence: ["playwright suite"],
    })) as { proposal: string };
    return res.proposal;
  } finally {
    agent.close();
  }
}
